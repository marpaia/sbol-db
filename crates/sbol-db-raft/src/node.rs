use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use openraft::{Config, Raft};
use uuid::Uuid;

use crate::{
    http_raft_router, HttpNetworkFactory, NodeIdentity, ReplicatedConfigStore,
    ReplicatedTokenStore, ReplicatedUserStore, RocksLogStore, RocksStateMachine, TypeConfig,
};

/// The independent durable directories owned by one RocksDB Raft node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeStorageLayout {
    pub root: PathBuf,
    pub raft_log: PathBuf,
    pub state: PathBuf,
    pub snapshots: PathBuf,
}

impl NodeStorageLayout {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            raft_log: root.join("raft-log.rocksdb"),
            state: root.join("state.rocksdb"),
            snapshots: root.join("snapshots"),
            root,
        }
    }
}

/// Inputs required to open one data-bearing Raft voter or learner.
///
/// The shared secret is intentionally omitted from `Debug`; callers should
/// source it from a secret manager and bind the returned RPC router only to an
/// internal interface.
#[derive(Clone)]
pub struct RocksRaftNodeConfig {
    pub identity: NodeIdentity,
    pub storage_root: PathBuf,
    pub bearer_token: String,
    pub raft: Arc<Config>,
    /// Optional node-local routes used instead of canonical membership
    /// addresses for outbound peer RPCs.
    pub peer_routes: BTreeMap<u64, String>,
}

impl fmt::Debug for RocksRaftNodeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RocksRaftNodeConfig")
            .field("identity", &self.identity)
            .field("storage_root", &self.storage_root)
            .field("bearer_token", &"[redacted]")
            .field("peer_route_count", &self.peer_routes.len())
            .field("raft", &self.raft)
            .finish()
    }
}

/// An opened RocksDB Raft node and the narrow replicated application seam.
///
/// Constructing this type does not initialize cluster membership and does not
/// expose an HA application server. Bootstrap/join policy belongs to the
/// orchestration layer; all application mutation traits must be replicated
/// before the public API can advertise HA mode.
pub struct RocksRaftNode {
    identity: NodeIdentity,
    layout: NodeStorageLayout,
    raft: Raft<TypeConfig>,
    state_machine: RocksStateMachine,
    rpc_router: Router,
}

impl RocksRaftNode {
    pub async fn open(config: RocksRaftNodeConfig) -> io::Result<Self> {
        validate_config(&config)?;
        let layout = NodeStorageLayout::new(&config.storage_root);
        std::fs::create_dir_all(&layout.root)?;

        let log_store = RocksLogStore::open(&layout.raft_log, config.identity)?;
        let state_machine =
            RocksStateMachine::open(&layout.state, &layout.snapshots, config.identity.cluster_id)
                .map_err(io::Error::other)?;
        let network = HttpNetworkFactory::new(config.bearer_token.clone())?
            .with_route_overrides(config.peer_routes.clone());
        let raft = Raft::new(
            config.identity.node_id,
            config.raft,
            network,
            log_store,
            state_machine.clone(),
        )
        .await
        .map_err(io::Error::other)?;
        let rpc_router = http_raft_router(raft.clone(), &config.bearer_token)?;

        Ok(Self {
            identity: config.identity,
            layout,
            raft,
            state_machine,
            rpc_router,
        })
    }

    pub fn identity(&self) -> NodeIdentity {
        self.identity
    }

    pub fn layout(&self) -> &NodeStorageLayout {
        &self.layout
    }

    pub fn raft(&self) -> &Raft<TypeConfig> {
        &self.raft
    }

    pub fn state_machine(&self) -> &RocksStateMachine {
        &self.state_machine
    }

    pub fn rpc_router(&self) -> Router {
        self.rpc_router.clone()
    }

    pub fn config_store(&self, client_id: Uuid) -> ReplicatedConfigStore {
        ReplicatedConfigStore::new(self.raft.clone(), self.state_machine.clone(), client_id)
    }

    pub fn token_store(&self, client_id: Uuid) -> ReplicatedTokenStore {
        ReplicatedTokenStore::new(self.raft.clone(), self.state_machine.clone(), client_id)
    }

    pub fn user_store(&self, client_id: Uuid) -> ReplicatedUserStore {
        ReplicatedUserStore::new(self.raft.clone(), self.state_machine.clone(), client_id)
    }

    pub async fn shutdown(&self) -> io::Result<()> {
        self.raft.shutdown().await.map_err(io::Error::other)
    }
}

fn validate_config(config: &RocksRaftNodeConfig) -> io::Result<()> {
    if config.identity.cluster_id.is_nil() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Raft cluster id must not be nil",
        ));
    }
    if config.bearer_token.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Raft bearer token must not be empty",
        ));
    }
    if config.storage_root.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Raft storage root must not be empty",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;

    fn raft_config() -> Arc<Config> {
        Arc::new(
            Config {
                cluster_name: "node-open-test".to_owned(),
                heartbeat_interval: 100,
                election_timeout_min: 300,
                election_timeout_max: 600,
                ..Default::default()
            }
            .validate()
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn node_open_creates_and_reuses_the_documented_layout() {
        let directory = tempdir().unwrap();
        let config = RocksRaftNodeConfig {
            identity: NodeIdentity {
                cluster_id: Uuid::from_u128(900),
                node_id: 1,
            },
            storage_root: directory.path().join("node"),
            bearer_token: "test-secret".to_owned(),
            raft: raft_config(),
            peer_routes: BTreeMap::new(),
        };

        let node = RocksRaftNode::open(config.clone()).await.unwrap();
        assert!(node.layout().raft_log.is_dir());
        assert!(node.layout().state.is_dir());
        assert!(node.layout().snapshots.is_dir());
        node.shutdown().await.unwrap();
        drop(node);

        // Shutdown completion is normally enough to release the RocksDB
        // handles; the bounded retry keeps this assertion insensitive to task
        // teardown scheduling on slower CI hosts.
        let reopened = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match RocksRaftNode::open(config.clone()).await {
                    Ok(node) => break node,
                    Err(error) if error.kind() == io::ErrorKind::Other => {
                        tokio::task::yield_now().await;
                    }
                    Err(error) => panic!("unexpected reopen error: {error}"),
                }
            }
        })
        .await
        .expect("node storage lock should be released after shutdown");
        reopened.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn node_open_rejects_nil_cluster_identity_before_touching_disk() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("must-not-exist");
        let result = RocksRaftNode::open(RocksRaftNodeConfig {
            identity: NodeIdentity {
                cluster_id: Uuid::nil(),
                node_id: 1,
            },
            storage_root: root.clone(),
            bearer_token: "test-secret".to_owned(),
            raft: raft_config(),
            peer_routes: BTreeMap::new(),
        })
        .await;
        assert!(result.is_err());
        assert!(!root.exists());
    }
}
