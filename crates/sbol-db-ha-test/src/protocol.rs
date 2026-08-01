use std::collections::BTreeMap;

use sbol_db_raft::CommandResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BootstrapRequest {
    pub members: BTreeMap<u64, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node_id: u64,
    pub cluster_id: uuid::Uuid,
    pub current_term: u64,
    pub current_leader: Option<u64>,
    pub last_log_index: Option<u64>,
    pub last_applied_index: Option<u64>,
    pub snapshot_index: Option<u64>,
    pub state: String,
    pub protocol_version: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandReply {
    Applied {
        log_index: u64,
        response: CommandResponse,
    },
    NotLeader {
        leader_id: Option<u64>,
        message: String,
    },
    Unavailable {
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigReadReply {
    pub key: String,
    pub value: Option<Value>,
    pub applied_index: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailpointAction {
    Arm,
    Release,
    Clear,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FailpointStatus {
    pub name: String,
    pub armed: bool,
    pub hit: bool,
}
