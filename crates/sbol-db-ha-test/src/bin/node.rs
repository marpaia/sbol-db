use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use openraft::{BasicNode, Config, SnapshotPolicy};
use sbol_db_ha_test::{
    BootstrapRequest, CommandReply, ConfigReadReply, FailpointAction, FailpointStatus, NodeStatus,
};
use sbol_db_raft::{
    CommandEnvelope, NodeIdentity, RocksRaftNode, RocksRaftNodeConfig, RocksStateMachine,
    TypeConfig, RAFT_PROTOCOL_VERSION,
};
use tokio::sync::Notify;
use uuid::Uuid;

const MAX_TEST_COMMAND_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(about = "Run one test-only RocksDB/OpenRaft voter process")]
struct Cli {
    #[arg(long)]
    node_id: u64,
    #[arg(long)]
    cluster_id: Uuid,
    #[arg(long)]
    cluster_name: String,
    #[arg(long)]
    storage_root: PathBuf,
    #[arg(long)]
    peer_bind: SocketAddr,
    #[arg(long)]
    client_bind: SocketAddr,
    #[arg(long, env = "SBOL_DB_RAFT_TOKEN", hide_env_values = true)]
    bearer_token: String,
    #[arg(long, value_parser = parse_peer_route)]
    peer_route: Vec<(u64, String)>,
}

#[derive(Clone)]
struct AppState {
    identity: NodeIdentity,
    raft: openraft::Raft<TypeConfig>,
    state_machine: RocksStateMachine,
    response_failpoint: Arc<ResponseFailpoint>,
}

#[derive(Clone)]
struct AuthState {
    bearer_token: Arc<str>,
}

#[derive(Default)]
struct ResponseFailpoint {
    armed: AtomicBool,
    hit: AtomicBool,
    release: Notify,
}

impl ResponseFailpoint {
    async fn pause_if_armed(&self) {
        if self.armed.swap(false, Ordering::AcqRel) {
            self.hit.store(true, Ordering::Release);
            self.release.notified().await;
            self.hit.store(false, Ordering::Release);
        }
    }

    fn apply(&self, action: FailpointAction) {
        match action {
            FailpointAction::Arm => {
                self.hit.store(false, Ordering::Release);
                self.armed.store(true, Ordering::Release);
            }
            FailpointAction::Release => {
                self.armed.store(false, Ordering::Release);
                self.release.notify_waiters();
            }
            FailpointAction::Clear => {
                self.armed.store(false, Ordering::Release);
                self.hit.store(false, Ordering::Release);
                self.release.notify_waiters();
            }
        }
    }

    fn status(&self) -> FailpointStatus {
        FailpointStatus {
            name: "after_apply_before_response".to_owned(),
            armed: self.armed.load(Ordering::Acquire),
            hit: self.hit.load(Ordering::Acquire),
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.node_id == 0 {
        bail!("node id must be non-zero");
    }
    let peer_routes = cli.peer_route.into_iter().collect::<BTreeMap<_, _>>();
    let raft_config = Arc::new(
        Config {
            cluster_name: cli.cluster_name,
            // Debug builds can spend well over a second serializing and
            // applying the largest SBOLTestSuite documents. Keep elections
            // responsive to process/network faults without allowing a large
            // replicated entry to create artificial leader churn.
            heartbeat_interval: 250,
            election_timeout_min: 2_500,
            election_timeout_max: 5_000,
            install_snapshot_timeout: 120_000,
            snapshot_policy: SnapshotPolicy::LogsSinceLast(32),
            replication_lag_threshold: 48,
            max_in_snapshot_log_to_keep: 8,
            purge_batch_size: 4,
            ..Default::default()
        }
        .validate()
        .context("validating test Raft configuration")?,
    );
    let runtime = RocksRaftNode::open(RocksRaftNodeConfig {
        identity: NodeIdentity {
            cluster_id: cli.cluster_id,
            node_id: cli.node_id,
        },
        storage_root: cli.storage_root,
        bearer_token: cli.bearer_token.clone(),
        raft: raft_config,
        peer_routes,
    })
    .await
    .context("opening RocksDB Raft node")?;
    let state = AppState {
        identity: runtime.identity(),
        raft: runtime.raft().clone(),
        state_machine: runtime.state_machine().clone(),
        response_failpoint: Arc::new(ResponseFailpoint::default()),
    };
    let auth = AuthState {
        bearer_token: cli.bearer_token.into(),
    };
    let test_router = Router::new()
        .route("/test/v1/status", get(status))
        .route("/test/v1/bootstrap", post(bootstrap))
        .route("/test/v1/command", post(command))
        .route("/test/v1/config", get(read_config))
        .route("/test/v1/state-audit", get(state_audit))
        .route("/test/v1/snapshot", post(trigger_snapshot))
        .route(
            "/test/v1/failpoint/after-apply-before-response",
            get(failpoint_status).post(set_failpoint),
        )
        .route_layer(axum::middleware::from_fn_with_state(auth, require_auth))
        .layer(DefaultBodyLimit::max(MAX_TEST_COMMAND_BYTES))
        .with_state(state);

    let peer_listener = tokio::net::TcpListener::bind(cli.peer_bind)
        .await
        .context("binding peer listener")?;
    let client_listener = tokio::net::TcpListener::bind(cli.client_bind)
        .await
        .context("binding test client listener")?;
    let peer_server = axum::serve(peer_listener, runtime.rpc_router());
    let client_server = axum::serve(client_listener, test_router);
    tokio::select! {
        result = peer_server => result.context("peer server stopped")?,
        result = client_server => result.context("test client server stopped")?,
        _ = tokio::signal::ctrl_c() => {}
    }
    runtime
        .shutdown()
        .await
        .context("shutting down Raft node")?;
    Ok(())
}

async fn status(State(state): State<AppState>) -> Json<NodeStatus> {
    let metrics = state.raft.metrics().borrow().clone();
    Json(NodeStatus {
        node_id: state.identity.node_id,
        cluster_id: state.identity.cluster_id,
        current_term: metrics.current_term,
        current_leader: metrics.current_leader,
        last_log_index: metrics.last_log_index,
        last_applied_index: metrics.last_applied.map(|log_id| log_id.index),
        snapshot_index: metrics.snapshot.map(|log_id| log_id.index),
        state: format!("{:?}", metrics.state).to_lowercase(),
        protocol_version: RAFT_PROTOCOL_VERSION,
    })
}

async fn bootstrap(
    State(state): State<AppState>,
    Json(request): Json<BootstrapRequest>,
) -> Response {
    let members = request
        .members
        .into_iter()
        .map(|(node_id, address)| (node_id, BasicNode::new(address)))
        .collect::<BTreeMap<_, _>>();
    match state.raft.initialize(members).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, error),
    }
}

async fn command(State(state): State<AppState>, Json(request): Json<CommandEnvelope>) -> Response {
    match state.raft.client_write(request).await {
        Ok(response) => {
            state.response_failpoint.pause_if_armed().await;
            Json(CommandReply::Applied {
                log_index: response.log_id.index,
                response: response.data,
            })
            .into_response()
        }
        Err(error) => (
            StatusCode::CONFLICT,
            Json(CommandReply::NotLeader {
                leader_id: state.raft.metrics().borrow().current_leader,
                message: error.to_string(),
            }),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ConfigQuery {
    key: String,
}

async fn read_config(State(state): State<AppState>, Query(query): Query<ConfigQuery>) -> Response {
    if let Err(error) = state.raft.ensure_linearizable().await {
        return (
            StatusCode::CONFLICT,
            Json(CommandReply::NotLeader {
                leader_id: state.raft.metrics().borrow().current_leader,
                message: error.to_string(),
            }),
        )
            .into_response();
    }
    match state.state_machine.read_config(&query.key) {
        Ok(entry) => Json(ConfigReadReply {
            key: query.key,
            value: entry.map(|entry| entry.value),
            applied_index: state
                .raft
                .metrics()
                .borrow()
                .last_applied
                .map(|log_id| log_id.index),
        })
        .into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn state_audit(State(state): State<AppState>) -> Response {
    match state.state_machine.audit() {
        Ok(audit) => Json(audit).into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn trigger_snapshot(State(state): State<AppState>) -> Response {
    match state.raft.trigger().snapshot().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, error),
    }
}

async fn failpoint_status(State(state): State<AppState>) -> Json<FailpointStatus> {
    Json(state.response_failpoint.status())
}

async fn set_failpoint(
    State(state): State<AppState>,
    Json(action): Json<FailpointAction>,
) -> Json<FailpointStatus> {
    state.response_failpoint.apply(action);
    Json(state.response_failpoint.status())
}

async fn require_auth(State(state): State<AuthState>, request: Request, next: Next) -> Response {
    let accepted = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value.as_bytes() == state.bearer_token.as_bytes());
    if accepted {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn api_error(status: StatusCode, error: impl std::fmt::Display) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": error.to_string() })),
    )
        .into_response()
}

fn parse_peer_route(raw: &str) -> Result<(u64, String), String> {
    let (node_id, address) = raw
        .split_once('=')
        .ok_or_else(|| "peer route must be NODE_ID=URL".to_owned())?;
    let node_id = node_id.parse::<u64>().map_err(|error| error.to_string())?;
    if !(address.starts_with("http://") || address.starts_with("https://")) {
        return Err("peer route URL must start with http:// or https://".to_owned());
    }
    Ok((node_id, address.to_owned()))
}
