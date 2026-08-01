use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::StatusCode;
use sbol_db_raft::{CommandEnvelope, ReplicatedCommand, StateAudit};
use serde_json::Value;
use tokio::process::{Child, Command};
use uuid::Uuid;

use crate::{
    BootstrapRequest, CommandReply, ConfigReadReply, FailpointAction, FailpointStatus,
    FaultNetwork, NodeStatus,
};

#[derive(Clone, Debug)]
pub struct ProcessClusterConfig {
    pub node_binary: PathBuf,
    pub root: PathBuf,
    pub cluster_id: Uuid,
    pub bearer_token: String,
    pub node_count: u64,
}

impl ProcessClusterConfig {
    pub fn three_nodes(node_binary: impl AsRef<Path>, root: impl AsRef<Path>) -> Self {
        Self {
            node_binary: node_binary.as_ref().to_path_buf(),
            root: root.as_ref().to_path_buf(),
            cluster_id: Uuid::new_v4(),
            bearer_token: format!("ha-test-{}", Uuid::new_v4()),
            node_count: 3,
        }
    }
}

struct NodeProcess {
    child: Child,
}

pub struct ProcessCluster {
    config: ProcessClusterConfig,
    peer_addresses: BTreeMap<u64, SocketAddr>,
    client_addresses: BTreeMap<u64, SocketAddr>,
    routes: BTreeMap<u64, BTreeMap<u64, String>>,
    network: FaultNetwork,
    processes: BTreeMap<u64, NodeProcess>,
    client: reqwest::Client,
}

#[derive(Clone)]
pub struct ProcessClient {
    client_addresses: BTreeMap<u64, SocketAddr>,
    bearer_token: String,
    client: reqwest::Client,
}

impl ProcessClient {
    pub async fn status(&self, node_id: u64) -> Result<NodeStatus> {
        self.client
            .get(self.url(node_id, "/test/v1/status")?)
            .bearer_auth(&self.bearer_token)
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("decoding node status")
    }

    pub async fn command(&self, node_id: u64, request: &CommandEnvelope) -> Result<CommandReply> {
        let response = self
            .client
            .post(self.url(node_id, "/test/v1/command")?)
            .bearer_auth(&self.bearer_token)
            .timeout(Duration::from_secs(5))
            .json(request)
            .send()
            .await?;
        if response.status().is_success() || response.status() == StatusCode::CONFLICT {
            response.json().await.context("decoding command reply")
        } else {
            let status = response.status();
            let message = response.text().await.unwrap_or_default();
            bail!("node {node_id} command failed with HTTP {status}: {message}")
        }
    }

    pub async fn read_config(
        &self,
        node_id: u64,
        key: &str,
    ) -> Result<Result<ConfigReadReply, CommandReply>> {
        let response = self
            .client
            .get(self.url(node_id, "/test/v1/config")?)
            .bearer_auth(&self.bearer_token)
            .query(&[("key", key)])
            .timeout(Duration::from_secs(5))
            .send()
            .await?;
        if response.status().is_success() {
            Ok(Ok(response.json().await?))
        } else if response.status() == StatusCode::CONFLICT {
            Ok(Err(response.json().await?))
        } else {
            let status = response.status();
            let message = response.text().await.unwrap_or_default();
            bail!("node {node_id} read failed with HTTP {status}: {message}")
        }
    }

    fn url(&self, node_id: u64, path: &str) -> Result<String> {
        self.client_addresses
            .get(&node_id)
            .map(|address| format!("http://{address}{path}"))
            .ok_or_else(|| anyhow!("unknown node {node_id}"))
    }
}

impl ProcessCluster {
    pub async fn start(config: ProcessClusterConfig) -> Result<Self> {
        if config.node_count != 3 {
            bail!("the single-host qualification stack requires exactly three voters");
        }
        if !config.node_binary.is_file() {
            bail!(
                "HA node binary does not exist: {}",
                config.node_binary.display()
            );
        }
        fs::create_dir_all(&config.root)
            .with_context(|| format!("creating cluster root {}", config.root.display()))?;
        let mut peer_addresses = BTreeMap::new();
        let mut client_addresses = BTreeMap::new();
        for node_id in 1..=config.node_count {
            peer_addresses.insert(node_id, reserve_address()?);
            client_addresses.insert(node_id, reserve_address()?);
        }
        let network = FaultNetwork::start(&peer_addresses).await?;
        let routes = (1..=config.node_count)
            .map(|node_id| (node_id, network.routes_for(node_id)))
            .collect();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .build()
            .context("building HA controller HTTP client")?;
        let mut cluster = Self {
            config,
            peer_addresses,
            client_addresses,
            routes,
            network,
            processes: BTreeMap::new(),
            client,
        };
        for node_id in 1..=cluster.config.node_count {
            cluster.start_node(node_id).await?;
        }
        cluster.wait_for_nodes(Duration::from_secs(15)).await?;
        cluster.bootstrap().await?;
        cluster
            .wait_for_writable_leader(None, Duration::from_secs(15))
            .await?;
        Ok(cluster)
    }

    pub fn cluster_id(&self) -> Uuid {
        self.config.cluster_id
    }

    pub fn root(&self) -> &Path {
        &self.config.root
    }

    pub fn client_handle(&self) -> ProcessClient {
        ProcessClient {
            client_addresses: self.client_addresses.clone(),
            bearer_token: self.config.bearer_token.clone(),
            client: self.client.clone(),
        }
    }

    pub fn node_ids(&self) -> BTreeSet<u64> {
        (1..=self.config.node_count).collect()
    }

    pub fn isolate(&self, node_id: u64) {
        self.network.isolate(node_id);
    }

    pub fn partition(&self, groups: &[BTreeSet<u64>]) {
        self.network.partition(groups);
    }

    pub fn heal(&self) {
        self.network.heal();
    }

    pub async fn kill(&mut self, node_id: u64) -> Result<()> {
        let mut process = self
            .processes
            .remove(&node_id)
            .ok_or_else(|| anyhow!("node {node_id} is not running"))?;
        process
            .child
            .kill()
            .await
            .with_context(|| format!("SIGKILL node {node_id}"))?;
        let _ = process.child.wait().await;
        Ok(())
    }

    pub async fn restart(&mut self, node_id: u64) -> Result<()> {
        if self.processes.contains_key(&node_id) {
            bail!("node {node_id} is already running");
        }
        self.start_node(node_id).await?;
        self.wait_for_node(node_id, Duration::from_secs(15)).await
    }

    pub async fn full_restart(&mut self) -> Result<()> {
        let running = self.processes.keys().copied().collect::<Vec<_>>();
        for node_id in running {
            self.kill(node_id).await?;
        }
        for node_id in 1..=self.config.node_count {
            self.start_node(node_id).await?;
        }
        self.wait_for_nodes(Duration::from_secs(15)).await?;
        self.wait_for_writable_leader(None, Duration::from_secs(15))
            .await?;
        Ok(())
    }

    pub async fn status(&self, node_id: u64) -> Result<NodeStatus> {
        self.get(node_id, "/test/v1/status").await
    }

    pub async fn audit(&self, node_id: u64) -> Result<StateAudit> {
        self.get(node_id, "/test/v1/state-audit").await
    }

    pub async fn read_config(&self, node_id: u64, key: &str) -> Result<ConfigReadReply> {
        self.client
            .get(self.url(node_id, "/test/v1/config")?)
            .bearer_auth(&self.config.bearer_token)
            .query(&[("key", key)])
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("decoding linearizable config read")
    }

    pub async fn trigger_snapshot(&self, node_id: u64) -> Result<()> {
        let response = self
            .post_json(node_id, "/test/v1/snapshot", &serde_json::json!({}))
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            bail!(
                "node {node_id} rejected snapshot trigger: {}",
                response.status()
            )
        }
    }

    pub async fn set_failpoint(
        &self,
        node_id: u64,
        action: FailpointAction,
    ) -> Result<FailpointStatus> {
        let response = self
            .post_json(
                node_id,
                "/test/v1/failpoint/after-apply-before-response",
                &action,
            )
            .await?;
        response
            .error_for_status()?
            .json()
            .await
            .context("decoding failpoint status")
    }

    pub async fn failpoint_status(&self, node_id: u64) -> Result<FailpointStatus> {
        self.get(node_id, "/test/v1/failpoint/after-apply-before-response")
            .await
    }

    pub async fn wait_for_failpoint_hit(&self, node_id: u64, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        loop {
            if self.failpoint_status(node_id).await?.hit {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                bail!("node {node_id} did not hit the response failpoint");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn command(&self, node_id: u64, request: &CommandEnvelope) -> Result<CommandReply> {
        let body = serde_json::to_vec(request).context("encoding replicated command")?;
        self.command_bytes(node_id, &body).await
    }

    pub async fn command_bytes(&self, node_id: u64, body: &[u8]) -> Result<CommandReply> {
        let response = self
            .client
            .post(self.url(node_id, "/test/v1/command")?)
            .bearer_auth(&self.config.bearer_token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(Duration::from_secs(60))
            .body(body.to_vec())
            .send()
            .await
            .with_context(|| format!("submitting command to node {node_id}"))?;
        if response.status().is_success() || response.status() == StatusCode::CONFLICT {
            return response
                .json()
                .await
                .context("decoding Raft command response");
        }
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        bail!("node {node_id} command failed with HTTP {status}: {message}")
    }

    pub fn spawn_command(
        &self,
        node_id: u64,
        request: CommandEnvelope,
    ) -> Result<tokio::task::JoinHandle<Result<CommandReply>>> {
        let url = self.url(node_id, "/test/v1/command")?;
        let client = self.client.clone();
        let bearer_token = self.config.bearer_token.clone();
        Ok(tokio::spawn(async move {
            let response = client
                .post(url)
                .bearer_auth(bearer_token)
                .timeout(Duration::from_secs(30))
                .json(&request)
                .send()
                .await
                .context("submitting asynchronous HA command")?;
            if response.status().is_success() || response.status() == StatusCode::CONFLICT {
                response
                    .json()
                    .await
                    .context("decoding asynchronous Raft command response")
            } else {
                let status = response.status();
                let message = response.text().await.unwrap_or_default();
                bail!("node {node_id} command failed with HTTP {status}: {message}")
            }
        }))
    }

    pub async fn wait_for_writable_leader(
        &self,
        allowed: Option<&BTreeSet<u64>>,
        timeout: Duration,
    ) -> Result<u64> {
        let started = Instant::now();
        loop {
            for node_id in self.node_ids() {
                if !self.processes.contains_key(&node_id)
                    || allowed.is_some_and(|allowed| !allowed.contains(&node_id))
                {
                    continue;
                }
                let Ok(status) = self.status(node_id).await else {
                    continue;
                };
                if status.current_leader != Some(node_id) {
                    continue;
                }
                let barrier = CommandEnvelope::new(
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    ReplicatedCommand::Barrier,
                );
                if matches!(
                    self.command(node_id, &barrier).await,
                    Ok(CommandReply::Applied { .. })
                ) {
                    return Ok(node_id);
                }
            }
            if started.elapsed() >= timeout {
                bail!("cluster did not elect a writable leader within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub async fn wait_for_applied(
        &self,
        node_ids: &BTreeSet<u64>,
        index: u64,
        timeout: Duration,
    ) -> Result<()> {
        let started = Instant::now();
        loop {
            let mut ready = true;
            let mut observed = BTreeMap::new();
            for node_id in node_ids {
                match self.status(*node_id).await {
                    Ok(status) => {
                        let applied = status.last_applied_index.unwrap_or(0);
                        observed.insert(*node_id, Some(applied));
                        if applied < index {
                            ready = false;
                        }
                    }
                    Err(_) => {
                        observed.insert(*node_id, None);
                        ready = false;
                    }
                }
            }
            if ready {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                bail!(
                    "nodes did not apply log index {index} within {timeout:?}; observed {observed:?}"
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let node_ids = self.processes.keys().copied().collect::<Vec<_>>();
        for node_id in node_ids {
            self.kill(node_id).await?;
        }
        Ok(())
    }

    async fn bootstrap(&self) -> Result<()> {
        let request = BootstrapRequest {
            members: self
                .peer_addresses
                .iter()
                .map(|(id, address)| (*id, format!("http://{address}")))
                .collect(),
        };
        let response = self
            .post_json(1, "/test/v1/bootstrap", &request)
            .await
            .context("bootstrapping process cluster")?;
        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("bootstrap failed with HTTP {status}: {body}")
        }
    }

    async fn start_node(&mut self, node_id: u64) -> Result<()> {
        let node_root = self.config.root.join(format!("node-{node_id}"));
        let log_root = self.config.root.join("nodes").join(node_id.to_string());
        fs::create_dir_all(&node_root)?;
        fs::create_dir_all(&log_root)?;
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_root.join("stdout.log"))?;
        let stderr = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_root.join("stderr.log"))?;
        let mut command = Command::new(&self.config.node_binary);
        command
            .arg("--node-id")
            .arg(node_id.to_string())
            .arg("--cluster-id")
            .arg(self.config.cluster_id.to_string())
            .arg("--cluster-name")
            .arg(format!("sbol-db-ha-process-{}", self.config.cluster_id))
            .arg("--storage-root")
            .arg(node_root)
            .arg("--peer-bind")
            .arg(self.peer_addresses[&node_id].to_string())
            .arg("--client-bind")
            .arg(self.client_addresses[&node_id].to_string())
            .arg("--bearer-token")
            .arg(&self.config.bearer_token)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        for (target, route) in &self.routes[&node_id] {
            command.arg("--peer-route").arg(format!("{target}={route}"));
        }
        let child = command
            .spawn()
            .with_context(|| format!("starting node {node_id}"))?;
        self.processes.insert(node_id, NodeProcess { child });
        Ok(())
    }

    async fn wait_for_nodes(&self, timeout: Duration) -> Result<()> {
        for node_id in self.node_ids() {
            self.wait_for_node(node_id, timeout).await?;
        }
        Ok(())
    }

    async fn wait_for_node(&self, node_id: u64, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        loop {
            if self.status(node_id).await.is_ok() {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                bail!("node {node_id} did not start within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, node_id: u64, path: &str) -> Result<T> {
        self.client
            .get(self.url(node_id, path)?)
            .bearer_auth(&self.config.bearer_token)
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("decoding HA test API response")
    }

    async fn post_json(
        &self,
        node_id: u64,
        path: &str,
        value: &impl serde::Serialize,
    ) -> Result<reqwest::Response> {
        self.client
            .post(self.url(node_id, path)?)
            .bearer_auth(&self.config.bearer_token)
            .timeout(Duration::from_secs(60))
            .json(value)
            .send()
            .await
            .context("calling HA test API")
    }

    fn url(&self, node_id: u64, path: &str) -> Result<String> {
        self.client_addresses
            .get(&node_id)
            .map(|address| format!("http://{address}{path}"))
            .ok_or_else(|| anyhow!("unknown node {node_id}"))
    }
}

impl Drop for ProcessCluster {
    fn drop(&mut self) {
        for process in self.processes.values_mut() {
            let _ = process.child.start_kill();
        }
    }
}

fn reserve_address() -> Result<SocketAddr> {
    let mut last_error = None;
    for _ in 0..20 {
        match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => return listener.local_addr().context("reading reserved local port"),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error).context("reserving local test port"),
        }
    }
    Err(last_error.expect("a failed bind records an error")).context("reserving local test port")
}

pub fn config_command(
    client_id: Uuid,
    request_id: Uuid,
    key: &str,
    value: Value,
) -> CommandEnvelope {
    CommandEnvelope::new(
        client_id,
        request_id,
        ReplicatedCommand::SetConfig {
            key: key.to_owned(),
            value,
            updated_at: chrono::Utc::now(),
        },
    )
}
