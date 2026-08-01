//! Real-process systems-test infrastructure for the RocksDB HA stack.
//!
//! Workloads and evidence are deliberately independent of the process driver
//! so a future VM or Kubernetes driver can preserve the same test contract.

mod artifacts;
mod history;
mod network;
mod process;
mod protocol;
mod scenario;

pub use artifacts::{ArtifactBundle, RunManifest};
pub use history::{
    check_register_linearizable, Completion, History, HistoryEvent, HistoryRecorder,
    LinearizabilityReport, Operation,
};
pub use network::FaultNetwork;
pub use process::{ProcessClient, ProcessCluster, ProcessClusterConfig};
pub use protocol::{
    BootstrapRequest, CommandReply, ConfigReadReply, FailpointAction, FailpointStatus, NodeStatus,
};
pub use scenario::{run_process_chaos, ProcessChaosConfig, ProcessChaosReport};
