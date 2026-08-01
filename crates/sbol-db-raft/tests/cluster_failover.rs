#![allow(clippy::result_large_err)] // OpenRaft's RPC error is fixed by its network trait.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use openraft::error::{InstallSnapshotError, RPCError, RaftError, RemoteError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Config, Raft};
use sbol_db_raft::{
    CommandEnvelope, CommandOutcome, NodeIdentity, ReplicatedCommand, ReplicatedConfigStore,
    RocksLogStore, RocksStateMachine, TypeConfig,
};
use sbol_db_storage::ConfigStore;
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

#[derive(Clone, Default)]
struct InProcessRouter {
    peers: Arc<RwLock<HashMap<u64, Raft<TypeConfig>>>>,
}

impl InProcessRouter {
    fn insert(&self, id: u64, raft: Raft<TypeConfig>) {
        self.peers.write().unwrap().insert(id, raft);
    }

    fn remove(&self, id: u64) {
        self.peers.write().unwrap().remove(&id);
    }
}

impl RaftNetworkFactory<TypeConfig> for InProcessRouter {
    type Network = InProcessConnection;

    async fn new_client(&mut self, target: u64, node: &BasicNode) -> Self::Network {
        InProcessConnection {
            target,
            target_node: node.clone(),
            peers: self.peers.clone(),
        }
    }
}

struct InProcessConnection {
    target: u64,
    target_node: BasicNode,
    peers: Arc<RwLock<HashMap<u64, Raft<TypeConfig>>>>,
}

impl InProcessConnection {
    fn peer<E: std::error::Error>(
        &self,
    ) -> Result<Raft<TypeConfig>, RPCError<u64, BasicNode, RaftError<u64, E>>> {
        self.peers
            .read()
            .unwrap()
            .get(&self.target)
            .cloned()
            .ok_or_else(|| {
                let error = io::Error::new(
                    io::ErrorKind::NotConnected,
                    format!("node {} is offline", self.target),
                );
                RPCError::Unreachable(Unreachable::new(&error))
            })
    }
}

impl RaftNetwork<TypeConfig> for InProcessConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        self.peer()?.append_entries(rpc).await.map_err(|error| {
            RemoteError::new_with_node(self.target, self.target_node.clone(), error).into()
        })
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        self.peer()?.install_snapshot(rpc).await.map_err(|error| {
            RemoteError::new_with_node(self.target, self.target_node.clone(), error).into()
        })
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        self.peer()?.vote(rpc).await.map_err(|error| {
            RemoteError::new_with_node(self.target, self.target_node.clone(), error).into()
        })
    }
}

async fn wait_for_linearizable_leader(nodes: &BTreeMap<u64, Raft<TypeConfig>>) -> u64 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            for (id, node) in nodes {
                if node.ensure_linearizable().await.is_ok() {
                    return *id;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("cluster should elect a quorum-confirmed leader")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acknowledged_writes_survive_failover_and_full_cluster_restart() {
    let router = InProcessRouter::default();
    let config = Arc::new(
        Config {
            cluster_name: "sbol-db-ha-test".to_owned(),
            heartbeat_interval: 50,
            election_timeout_min: 150,
            election_timeout_max: 300,
            ..Default::default()
        }
        .validate()
        .unwrap(),
    );
    let mut directories: Vec<TempDir> = Vec::new();
    let mut nodes = BTreeMap::new();
    let mut states = BTreeMap::new();
    let cluster_id = Uuid::from_u128(500);

    for id in 1..=3 {
        let directory = tempfile::tempdir().unwrap();
        let log = RocksLogStore::open(
            directory.path().join("raft-log"),
            NodeIdentity {
                cluster_id,
                node_id: id,
            },
        )
        .unwrap();
        let state = RocksStateMachine::open(
            directory.path().join("state"),
            directory.path().join("snapshots"),
            cluster_id,
        )
        .unwrap();
        let raft = Raft::new(id, config.clone(), router.clone(), log, state.clone())
            .await
            .unwrap();
        router.insert(id, raft.clone());
        directories.push(directory);
        states.insert(id, state);
        nodes.insert(id, raft);
    }

    let members = (1..=3)
        .map(|id| (id, BasicNode::new(format!("in-process://{id}"))))
        .collect::<BTreeMap<_, _>>();
    nodes[&1].initialize(members).await.unwrap();

    let metrics = nodes[&1]
        .wait(Some(Duration::from_secs(5)))
        .metrics(|metrics| metrics.current_leader.is_some(), "initial leader")
        .await
        .unwrap();
    let first_leader = metrics.current_leader.unwrap();
    let first_request = CommandEnvelope::new(
        Uuid::from_u128(100),
        Uuid::from_u128(101),
        ReplicatedCommand::SetConfig {
            key: "theme".to_owned(),
            value: json!({"dark": true}),
            updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        },
    );
    let committed = nodes[&first_leader]
        .client_write(first_request)
        .await
        .unwrap();
    assert_eq!(committed.data.outcome, CommandOutcome::Applied);
    let committed_index = committed.log_id.index;

    for (id, node) in &nodes {
        node.wait(Some(Duration::from_secs(5)))
            .applied_index_at_least(Some(committed_index), format!("node {id} caught up"))
            .await
            .unwrap();
    }

    router.remove(first_leader);
    nodes[&first_leader].shutdown().await.unwrap();

    let survivor = *nodes.keys().find(|id| **id != first_leader).unwrap();
    let metrics = nodes[&survivor]
        .wait(Some(Duration::from_secs(5)))
        .metrics(
            |metrics| metrics.current_leader.is_some_and(|id| id != first_leader),
            "replacement leader",
        )
        .await
        .unwrap();
    let replacement_leader = metrics.current_leader.unwrap();
    nodes[&replacement_leader]
        .ensure_linearizable()
        .await
        .unwrap();

    let replicated_config = ReplicatedConfigStore::new(
        nodes[&replacement_leader].clone(),
        states[&replacement_leader].clone(),
        Uuid::from_u128(100),
    );
    assert_eq!(
        replicated_config.get("theme").await.unwrap(),
        Some(json!({"dark": true}))
    );
    replicated_config
        .set("mail", &json!({"enabled": true}))
        .await
        .unwrap();
    assert_eq!(
        replicated_config.get("mail").await.unwrap(),
        Some(json!({"enabled": true}))
    );

    for (id, node) in &nodes {
        if *id != first_leader {
            node.shutdown().await.unwrap();
        }
    }

    // Release every live storage handle while retaining the durable node
    // directories, then reconstruct the entire cluster from disk. No member is
    // initialized again: membership, votes, log entries, applied indexes, and
    // application values must all come from persisted state.
    drop(replicated_config);
    router.peers.write().unwrap().clear();
    drop(nodes);
    drop(states);

    let restarted_router = InProcessRouter::default();
    let mut restarted_nodes = BTreeMap::new();
    let mut restarted_states = BTreeMap::new();
    for id in 1..=3 {
        let root = directories[(id - 1) as usize].path();
        let log = RocksLogStore::open(
            root.join("raft-log"),
            NodeIdentity {
                cluster_id,
                node_id: id,
            },
        )
        .unwrap();
        let state = RocksStateMachine::open(root.join("state"), root.join("snapshots"), cluster_id)
            .unwrap();
        let raft = Raft::new(
            id,
            config.clone(),
            restarted_router.clone(),
            log,
            state.clone(),
        )
        .await
        .unwrap();
        restarted_router.insert(id, raft.clone());
        restarted_states.insert(id, state);
        restarted_nodes.insert(id, raft);
    }

    let restarted_leader = wait_for_linearizable_leader(&restarted_nodes).await;
    let restarted_config = ReplicatedConfigStore::new(
        restarted_nodes[&restarted_leader].clone(),
        restarted_states[&restarted_leader].clone(),
        Uuid::from_u128(102),
    );
    assert_eq!(
        restarted_config.get("theme").await.unwrap(),
        Some(json!({"dark": true}))
    );
    assert_eq!(
        restarted_config.get("mail").await.unwrap(),
        Some(json!({"enabled": true}))
    );
    restarted_config
        .set("post_restart", &json!(true))
        .await
        .unwrap();

    for node in restarted_nodes.values() {
        node.shutdown().await.unwrap();
    }

    // Keep every RocksDB directory alive until all Raft tasks have stopped.
    drop(directories);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_voters_fail_closed_when_either_node_is_lost() {
    let router = InProcessRouter::default();
    let config = Arc::new(
        Config {
            cluster_name: "sbol-db-two-node-test".to_owned(),
            heartbeat_interval: 50,
            election_timeout_min: 150,
            election_timeout_max: 300,
            ..Default::default()
        }
        .validate()
        .unwrap(),
    );
    let mut directories = Vec::new();
    let mut nodes = BTreeMap::new();
    let mut states = BTreeMap::new();
    let cluster_id = Uuid::from_u128(600);

    for id in 1..=2 {
        let directory = tempfile::tempdir().unwrap();
        let log = RocksLogStore::open(
            directory.path().join("raft-log"),
            NodeIdentity {
                cluster_id,
                node_id: id,
            },
        )
        .unwrap();
        let state = RocksStateMachine::open(
            directory.path().join("state"),
            directory.path().join("snapshots"),
            cluster_id,
        )
        .unwrap();
        let raft = Raft::new(id, config.clone(), router.clone(), log, state.clone())
            .await
            .unwrap();
        router.insert(id, raft.clone());
        directories.push(directory);
        states.insert(id, state);
        nodes.insert(id, raft);
    }

    nodes[&1]
        .initialize(BTreeMap::from([
            (1, BasicNode::new("in-process://1")),
            (2, BasicNode::new("in-process://2")),
        ]))
        .await
        .unwrap();
    let initial = nodes[&1]
        .wait(Some(Duration::from_secs(5)))
        .metrics(|metrics| metrics.current_leader.is_some(), "initial leader")
        .await
        .unwrap();
    let first_leader = initial.current_leader.unwrap();
    let survivor = if first_leader == 1 { 2 } else { 1 };

    router.remove(first_leader);
    nodes[&first_leader].shutdown().await.unwrap();
    nodes[&survivor]
        .wait(Some(Duration::from_secs(5)))
        .metrics(
            |metrics| metrics.current_leader != Some(first_leader),
            "lost leader lease",
        )
        .await
        .unwrap();

    let unavailable_write = tokio::time::timeout(
        Duration::from_secs(1),
        nodes[&survivor].client_write(CommandEnvelope::new(
            Uuid::from_u128(200),
            Uuid::from_u128(201),
            ReplicatedCommand::SetConfig {
                key: "must_not_commit".to_owned(),
                value: json!(true),
                updated_at: Utc.timestamp_opt(1_700_000_002, 0).unwrap(),
            },
        )),
    )
    .await;
    assert!(
        !matches!(unavailable_write, Ok(Ok(_))),
        "a lone voter must not acknowledge a write"
    );
    assert!(states[&survivor]
        .read_config("must_not_commit")
        .unwrap()
        .is_none());

    nodes[&survivor].shutdown().await.unwrap();
    drop(directories);
}
