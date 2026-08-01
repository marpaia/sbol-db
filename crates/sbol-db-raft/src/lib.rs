//! Consensus primitives for RocksDB-backed sbol-db clusters.
//!
//! This crate deliberately does not depend on the HTTP server. It owns the
//! replicated-command wire format and the durable Raft log boundary so that
//! consensus can be tested independently from request routing and the SBOL
//! state machine.

mod http_transport;
mod log_store;
mod node;
mod protocol;
mod replicated_config;
mod replicated_token;
mod replicated_user;
mod state_machine;

use std::io::Cursor;

pub use log_store::{NodeIdentity, RocksLogStore};
pub use node::{NodeStorageLayout, RocksRaftNode, RocksRaftNodeConfig};
pub use protocol::{
    CommandEnvelope, CommandOutcome, CommandResponse, CommandResult, ReplicatedCommand,
    RAFT_PROTOCOL_VERSION,
};
pub use replicated_config::ReplicatedConfigStore;
pub use replicated_token::ReplicatedTokenStore;
pub use replicated_user::ReplicatedUserStore;
pub use state_machine::RocksStateMachine;

pub type NodeId = u64;

openraft::declare_raft_types!(
    pub TypeConfig:
        D = CommandEnvelope,
        R = CommandResponse,
        Node = openraft::BasicNode,
);
pub use http_transport::{http_raft_router, HttpNetworkFactory};
