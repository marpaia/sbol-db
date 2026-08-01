use std::collections::{BTreeSet, HashMap};
use std::io;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use openraft::error::{InstallSnapshotError, RPCError, RaftError, RemoteError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Raft};
use sbol_db_raft::TypeConfig;

type NetworkError<E> = RPCError<u64, BasicNode, RaftError<u64, E>>;
type RouteResult<E> = Result<(Raft<TypeConfig>, Duration), NetworkError<E>>;

#[derive(Clone, Copy, Debug)]
struct LinkState {
    enabled: bool,
    delay: Duration,
}

impl Default for LinkState {
    fn default() -> Self {
        Self {
            enabled: true,
            delay: Duration::ZERO,
        }
    }
}

#[derive(Clone, Default)]
pub struct SimulatedNetwork {
    peers: Arc<RwLock<HashMap<u64, Raft<TypeConfig>>>>,
    links: Arc<RwLock<HashMap<(u64, u64), LinkState>>>,
}

impl SimulatedNetwork {
    pub fn factory(&self, source: u64) -> NetworkFactory {
        NetworkFactory {
            source,
            network: self.clone(),
        }
    }

    pub fn register(&self, node_id: u64, raft: Raft<TypeConfig>) {
        self.peers.write().unwrap().insert(node_id, raft);
    }

    pub fn remove(&self, node_id: u64) {
        self.peers.write().unwrap().remove(&node_id);
    }

    pub fn heal(&self) {
        self.links.write().unwrap().clear();
    }

    pub fn isolate(&self, node_id: u64) {
        let node_ids = self.node_ids();
        let mut links = self.links.write().unwrap();
        for other in node_ids {
            if other != node_id {
                links.entry((node_id, other)).or_default().enabled = false;
                links.entry((other, node_id)).or_default().enabled = false;
            }
        }
    }

    pub fn partition(&self, groups: &[BTreeSet<u64>]) {
        let node_ids = self.node_ids();
        let mut group_by_node = HashMap::new();
        for (group_index, group) in groups.iter().enumerate() {
            for node_id in group {
                group_by_node.insert(*node_id, group_index);
            }
        }
        let mut links = self.links.write().unwrap();
        for source in &node_ids {
            for target in &node_ids {
                if source == target {
                    continue;
                }
                let enabled = group_by_node.contains_key(source)
                    && group_by_node.get(source) == group_by_node.get(target);
                links.entry((*source, *target)).or_default().enabled = enabled;
            }
        }
    }

    pub fn set_delay(&self, source: u64, target: u64, delay: Duration) {
        self.links
            .write()
            .unwrap()
            .entry((source, target))
            .or_default()
            .delay = delay;
    }

    fn node_ids(&self) -> Vec<u64> {
        self.peers.read().unwrap().keys().copied().collect()
    }

    fn route<E: std::error::Error>(&self, source: u64, target: u64) -> RouteResult<E> {
        let link = self
            .links
            .read()
            .unwrap()
            .get(&(source, target))
            .copied()
            .unwrap_or_default();
        if !link.enabled {
            let error = io::Error::new(
                io::ErrorKind::NotConnected,
                format!("simulated partition blocks {source} -> {target}"),
            );
            return Err(RPCError::Unreachable(Unreachable::new(&error)));
        }
        let peer = self
            .peers
            .read()
            .unwrap()
            .get(&target)
            .cloned()
            .ok_or_else(|| {
                let error = io::Error::new(
                    io::ErrorKind::NotConnected,
                    format!("node {target} is offline"),
                );
                RPCError::Unreachable(Unreachable::new(&error))
            })?;
        Ok((peer, link.delay))
    }
}

#[derive(Clone)]
pub struct NetworkFactory {
    source: u64,
    network: SimulatedNetwork,
}

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = SimulatedConnection;

    async fn new_client(&mut self, target: u64, node: &BasicNode) -> Self::Network {
        SimulatedConnection {
            source: self.source,
            target,
            target_node: node.clone(),
            network: self.network.clone(),
        }
    }
}

pub struct SimulatedConnection {
    source: u64,
    target: u64,
    target_node: BasicNode,
    network: SimulatedNetwork,
}

impl RaftNetwork<TypeConfig> for SimulatedConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let (peer, delay) = self.network.route(self.source, self.target)?;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        peer.append_entries(rpc).await.map_err(|error| {
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
        let (peer, delay) = self.network.route(self.source, self.target)?;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        peer.install_snapshot(rpc).await.map_err(|error| {
            RemoteError::new_with_node(self.target, self.target_node.clone(), error).into()
        })
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let (peer, delay) = self.network.route(self.source, self.target)?;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        peer.vote(rpc).await.map_err(|error| {
            RemoteError::new_with_node(self.target, self.target_node.clone(), error).into()
        })
    }
}
