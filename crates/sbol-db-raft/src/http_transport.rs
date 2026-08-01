use std::collections::BTreeMap;
use std::error::Error;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use openraft::error::{InstallSnapshotError, RPCError, RaftError, RemoteError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Raft};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{NodeId, TypeConfig};

const APPEND_PATH: &str = "/raft/append";
const VOTE_PATH: &str = "/raft/vote";
const SNAPSHOT_PATH: &str = "/raft/snapshot";
/// OpenRaft's default snapshot chunk is 3 MiB and JSON represents each byte as
/// a number. Axum's 2 MiB default would reject normal snapshot traffic.
const MAX_RAFT_RPC_BODY_BYTES: usize = 64 * 1024 * 1024;

/// OpenRaft network factory over authenticated HTTP.
///
/// `BasicNode::addr` is the peer's internal base URL. The bearer token is kept
/// only in process memory and should be supplied from a secret manager. Raft
/// endpoints belong on a private listener, not the public sbol-db API router.
#[derive(Clone)]
pub struct HttpNetworkFactory {
    client: reqwest::Client,
    bearer_token: Arc<str>,
    route_overrides: Arc<BTreeMap<NodeId, String>>,
}

impl HttpNetworkFactory {
    pub fn new(bearer_token: impl Into<String>) -> io::Result<Self> {
        let bearer_token = bearer_token.into();
        if bearer_token.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Raft bearer token must not be empty",
            ));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .user_agent(concat!("sbol-db-raft/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(io::Error::other)?;
        Ok(Self {
            client,
            bearer_token: bearer_token.into(),
            route_overrides: Arc::new(BTreeMap::new()),
        })
    }

    /// Override the address used to reach selected peers from this node.
    ///
    /// Raft membership continues to contain the peer's canonical address. The
    /// override is a transport concern, primarily useful for routing each
    /// directed source-to-target link through a fault-injection proxy. Because
    /// the map belongs to one node's factory, node 1 can route node 2 through a
    /// different proxy than node 3 uses to reach node 2.
    pub fn with_route_overrides(mut self, routes: BTreeMap<NodeId, String>) -> Self {
        self.route_overrides = Arc::new(routes);
        self
    }
}

impl RaftNetworkFactory<TypeConfig> for HttpNetworkFactory {
    type Network = HttpNetworkConnection;

    async fn new_client(&mut self, target: NodeId, node: &BasicNode) -> Self::Network {
        let mut node = node.clone();
        if let Some(address) = self.route_overrides.get(&target) {
            node.addr.clone_from(address);
        }
        HttpNetworkConnection {
            client: self.client.clone(),
            bearer_token: self.bearer_token.clone(),
            target,
            node,
        }
    }
}

pub struct HttpNetworkConnection {
    client: reqwest::Client,
    bearer_token: Arc<str>,
    target: NodeId,
    node: BasicNode,
}

impl HttpNetworkConnection {
    async fn send<Req, Resp, ApiError>(
        &self,
        path: &str,
        request: &Req,
        option: RPCOption,
    ) -> Result<Resp, RPCError<NodeId, BasicNode, RaftError<NodeId, ApiError>>>
    where
        Req: Serialize + ?Sized,
        Resp: DeserializeOwned,
        ApiError: Error + DeserializeOwned,
    {
        let endpoint = format!("{}{}", self.node.addr.trim_end_matches('/'), path);
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(self.bearer_token.as_ref())
            .timeout(option.hard_ttl())
            .json(request)
            .send()
            .await
            .map_err(network_error)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let error = io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("Raft authentication rejected by node {}", self.target),
            );
            return Err(Unreachable::new(&error).into());
        }
        if !response.status().is_success() {
            let error = io::Error::other(format!(
                "Raft node {} returned HTTP {}",
                self.target,
                response.status()
            ));
            return Err(network_error(error));
        }

        match response
            .json::<Result<Resp, RaftError<NodeId, ApiError>>>()
            .await
            .map_err(network_error)?
        {
            Ok(response) => Ok(response),
            Err(error) => {
                Err(RemoteError::new_with_node(self.target, self.node.clone(), error).into())
            }
        }
    }
}

impl RaftNetwork<TypeConfig> for HttpNetworkConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        self.send(APPEND_PATH, &rpc, option).await
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        self.send(SNAPSHOT_PATH, &rpc, option).await
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        self.send(VOTE_PATH, &rpc, option).await
    }
}

#[derive(Clone)]
struct HttpServerState {
    raft: Raft<TypeConfig>,
}

#[derive(Clone)]
struct HttpAuthState {
    token_digest: [u8; 32],
}

/// Build the authenticated internal RPC router for one Raft node.
///
/// The caller owns the listener and must bind it to an internal interface. An
/// empty shared secret is rejected; only its SHA-256 digest is retained by the
/// server state.
pub fn http_raft_router(raft: Raft<TypeConfig>, bearer_token: &str) -> io::Result<Router> {
    if bearer_token.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Raft bearer token must not be empty",
        ));
    }
    let state = HttpServerState { raft };
    let auth = HttpAuthState {
        token_digest: Sha256::digest(bearer_token.as_bytes()).into(),
    };
    Ok(Router::new()
        .route(APPEND_PATH, post(append_entries))
        .route(VOTE_PATH, post(vote))
        .route(SNAPSHOT_PATH, post(install_snapshot))
        .route_layer(axum::middleware::from_fn_with_state(auth, require_auth))
        .layer(DefaultBodyLimit::max(MAX_RAFT_RPC_BODY_BYTES))
        .with_state(state))
}

async fn append_entries(
    State(state): State<HttpServerState>,
    Json(request): Json<AppendEntriesRequest<TypeConfig>>,
) -> Json<Result<AppendEntriesResponse<NodeId>, RaftError<NodeId>>> {
    Json(state.raft.append_entries(request).await)
}

async fn vote(
    State(state): State<HttpServerState>,
    Json(request): Json<VoteRequest<NodeId>>,
) -> Json<Result<VoteResponse<NodeId>, RaftError<NodeId>>> {
    Json(state.raft.vote(request).await)
}

async fn install_snapshot(
    State(state): State<HttpServerState>,
    Json(request): Json<InstallSnapshotRequest<TypeConfig>>,
) -> Json<Result<InstallSnapshotResponse<NodeId>, RaftError<NodeId, InstallSnapshotError>>> {
    Json(state.raft.install_snapshot(request).await)
}

async fn require_auth(
    State(state): State<HttpAuthState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(value) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let candidate: [u8; 32] = Sha256::digest(value.as_bytes()).into();
    if constant_time_equal(&candidate, &state.token_digest) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn network_error<Node, ApiError>(
    error: impl Error + 'static,
) -> RPCError<NodeId, Node, RaftError<NodeId, ApiError>>
where
    Node: openraft::Node,
    ApiError: Error,
{
    openraft::error::NetworkError::new(&error).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_accepts_only_the_same_digest() {
        let left: [u8; 32] = Sha256::digest(b"correct").into();
        let same: [u8; 32] = Sha256::digest(b"correct").into();
        let other: [u8; 32] = Sha256::digest(b"incorrect").into();
        assert!(constant_time_equal(&left, &same));
        assert!(!constant_time_equal(&left, &other));
    }

    #[test]
    fn empty_tokens_are_rejected() {
        assert!(HttpNetworkFactory::new("  ").is_err());
    }
}
