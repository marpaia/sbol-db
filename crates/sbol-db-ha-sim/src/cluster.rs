use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use openraft::{BasicNode, Config, Raft, SnapshotPolicy};
use sbol_db_core::ConfigEntry;
use sbol_db_raft::{
    CommandEnvelope, CommandResponse, NodeIdentity, RocksLogStore, RocksStateMachine, TypeConfig,
};
use tempfile::TempDir;
use uuid::Uuid;

use crate::network::SimulatedNetwork;

struct NodeRuntime {
    raft: Raft<TypeConfig>,
    state: RocksStateMachine,
}

#[derive(Clone, Debug)]
pub struct WriteAck {
    pub leader_id: u64,
    pub log_index: u64,
    pub response: CommandResponse,
}

pub struct SimulatedCluster {
    root: TempDir,
    cluster_id: Uuid,
    node_count: u64,
    config: Arc<Config>,
    network: SimulatedNetwork,
    nodes: BTreeMap<u64, NodeRuntime>,
}

impl SimulatedCluster {
    pub async fn start(node_count: u64, cluster_id: Uuid) -> Result<Self> {
        if node_count < 2 {
            bail!("HA simulation needs at least two nodes");
        }
        let root = tempfile::tempdir().context("creating simulation root")?;
        let config = Arc::new(
            Config {
                cluster_name: format!("sbol-db-ha-sim-{cluster_id}"),
                heartbeat_interval: 40,
                election_timeout_min: 140,
                election_timeout_max: 280,
                snapshot_policy: SnapshotPolicy::LogsSinceLast(32),
                replication_lag_threshold: 48,
                max_in_snapshot_log_to_keep: 8,
                purge_batch_size: 4,
                ..Default::default()
            }
            .validate()
            .context("validating simulation Raft configuration")?,
        );
        let mut cluster = Self {
            root,
            cluster_id,
            node_count,
            config,
            network: SimulatedNetwork::default(),
            nodes: BTreeMap::new(),
        };
        for node_id in 1..=node_count {
            cluster.start_node(node_id).await?;
        }
        let members = (1..=node_count)
            .map(|node_id| (node_id, BasicNode::new(format!("sim://{node_id}"))))
            .collect::<BTreeMap<_, _>>();
        cluster.nodes[&1]
            .raft
            .initialize(members)
            .await
            .context("initializing simulated Raft membership")?;
        cluster
            .wait_for_leader(None)
            .await
            .context("electing initial simulated leader")?;
        Ok(cluster)
    }

    pub fn cluster_id(&self) -> Uuid {
        self.cluster_id
    }

    pub fn live_node_ids(&self) -> BTreeSet<u64> {
        self.nodes.keys().copied().collect()
    }

    pub async fn wait_for_leader(&self, allowed: Option<&BTreeSet<u64>>) -> Result<u64> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                for (node_id, runtime) in &self.nodes {
                    if allowed.is_some_and(|allowed| !allowed.contains(node_id)) {
                        continue;
                    }
                    let metrics = runtime.raft.metrics().borrow().clone();
                    if metrics.current_leader != Some(*node_id) {
                        continue;
                    }
                    if matches!(
                        tokio::time::timeout(
                            Duration::from_millis(500),
                            runtime.raft.ensure_linearizable()
                        )
                        .await,
                        Ok(Ok(_))
                    ) {
                        return *node_id;
                    }
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .map_err(|_| anyhow!("cluster did not elect a quorum-confirmed leader"))
    }

    pub async fn write(&self, leader_id: u64, command: CommandEnvelope) -> Result<WriteAck> {
        let runtime = self
            .nodes
            .get(&leader_id)
            .ok_or_else(|| anyhow!("node {leader_id} is offline"))?;
        let response = runtime
            .raft
            .client_write(command)
            .await
            .with_context(|| format!("writing through node {leader_id}"))?;
        Ok(WriteAck {
            leader_id,
            log_index: response.log_id.index,
            response: response.data,
        })
    }

    pub async fn stop_node(&mut self, node_id: u64) -> Result<()> {
        self.network.remove(node_id);
        if let Some(runtime) = self.nodes.remove(&node_id) {
            runtime
                .raft
                .shutdown()
                .await
                .with_context(|| format!("shutting down node {node_id}"))?;
            drop(runtime);
        }
        Ok(())
    }

    pub async fn restart_node(&mut self, node_id: u64) -> Result<()> {
        if self.nodes.contains_key(&node_id) {
            bail!("node {node_id} is already running");
        }
        if !(1..=self.node_count).contains(&node_id) {
            bail!("node {node_id} does not belong to this cluster");
        }
        self.start_node(node_id).await
    }

    pub async fn full_restart(&mut self) -> Result<()> {
        let live = self.live_node_ids().into_iter().collect::<Vec<_>>();
        for node_id in live {
            self.stop_node(node_id).await?;
        }
        for node_id in 1..=self.node_count {
            self.start_node(node_id).await?;
        }
        self.wait_for_leader(None)
            .await
            .context("electing leader after full cluster restart")?;
        Ok(())
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

    pub fn set_delay(&self, source: u64, target: u64, delay: Duration) {
        self.network.set_delay(source, target, delay);
    }

    pub async fn wait_for_all_applied(&self, log_index: u64) -> Result<()> {
        for (node_id, runtime) in &self.nodes {
            runtime
                .raft
                .wait(Some(Duration::from_secs(30)))
                .applied_index_at_least(Some(log_index), format!("node {node_id} caught up"))
                .await
                .with_context(|| format!("waiting for node {node_id} to apply {log_index}"))?;
        }
        Ok(())
    }

    pub async fn trigger_snapshot(&self, node_id: u64) -> Result<()> {
        self.nodes
            .get(&node_id)
            .ok_or_else(|| anyhow!("node {node_id} is offline"))?
            .raft
            .trigger()
            .snapshot()
            .await
            .with_context(|| format!("triggering a snapshot on node {node_id}"))
    }

    pub fn config_entries(&self, node_id: u64) -> Result<Vec<ConfigEntry>> {
        self.nodes
            .get(&node_id)
            .ok_or_else(|| anyhow!("node {node_id} is offline"))?
            .state
            .read_all_config()
            .with_context(|| format!("reading configuration state from node {node_id}"))
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let live = self.live_node_ids().into_iter().collect::<Vec<_>>();
        for node_id in live {
            self.stop_node(node_id).await?;
        }
        Ok(())
    }

    async fn start_node(&mut self, node_id: u64) -> Result<()> {
        let started = Instant::now();
        loop {
            match self.try_start_node(node_id).await {
                Ok(()) => return Ok(()),
                Err(error)
                    if started.elapsed() < Duration::from_secs(5)
                        && is_transient_rocksdb_lock(&error) =>
                {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn try_start_node(&mut self, node_id: u64) -> Result<()> {
        let node_root = self.node_root(node_id);
        let log = RocksLogStore::open(
            node_root.join("raft-log"),
            NodeIdentity {
                cluster_id: self.cluster_id,
                node_id,
            },
        )
        .with_context(|| format!("opening Raft log for node {node_id}"))?;
        let state = RocksStateMachine::open(
            node_root.join("state"),
            node_root.join("snapshots"),
            self.cluster_id,
        )
        .with_context(|| format!("opening state machine for node {node_id}"))?;
        let raft = Raft::new(
            node_id,
            self.config.clone(),
            self.network.factory(node_id),
            log,
            state.clone(),
        )
        .await
        .with_context(|| format!("starting Raft node {node_id}"))?;
        self.network.register(node_id, raft.clone());
        self.nodes.insert(node_id, NodeRuntime { raft, state });
        Ok(())
    }

    fn node_root(&self, node_id: u64) -> PathBuf {
        self.root.path().join(format!("node-{node_id}"))
    }
}

fn is_transient_rocksdb_lock(error: &anyhow::Error) -> bool {
    let rendered = format!("{error:#}");
    rendered.contains("/LOCK") || rendered.contains("lock hold by current process")
}
