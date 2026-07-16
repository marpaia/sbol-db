//! SynBioHub v1 read/query routes.
//!
//! Every handler resolves the caller's authorized
//! [`GraphScope`](sbol_db_sparql::GraphScope) from the `X-authorization`
//! identity ([`CurrentUser`]) and enforces it on the read: free-text relevance
//! goes through the facade's ranked search over tantivy, and everything else
//! (facets, counts, members, uses, twins, root/sub collections, metadata) is
//! SPARQL over the shared engine and its accelerator. The scope is the ceiling,
//! so a caller never reads a graph they are not entitled to.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_app::Hit;
use sbol_db_core::{DomainError, User};
use sbol_db_sparql::{GraphScope, ResultFormat, SparqlOptions};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::search::parse_search_path;
use super::{queries, CurrentUser};
use crate::{ApiError, AppState};

/// The instance base IRI classic SynBioHub mints objects under.
const BASE: &str = "http://synbiohub.org/";

/// The `?offset=&limit=` paging classic honors on the search family.
#[derive(Debug, Default, Deserialize)]
pub struct Paging {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// A public object path, `/public/<collectionId>/<displayId>/<version>/…`.
#[derive(Debug, Deserialize)]
pub struct PublicObject {
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
}

/// A user object path, `/user/<userId>/<collectionId>/<displayId>/<version>/…`.
#[derive(Debug, Deserialize)]
pub struct UserObject {
    pub user_id: String,
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
}

fn public_uri(object: &PublicObject) -> String {
    format!(
        "{BASE}public/{}/{}/{}",
        object.collection_id, object.display_id, object.version
    )
}

fn user_uri(object: &UserObject) -> String {
    format!(
        "{BASE}user/{}/{}/{}/{}",
        object.user_id, object.collection_id, object.display_id, object.version
    )
}

// --- /search and /searchCount ------------------------------------------------

/// The query parameters the bare `GET /search` route accepts. This path is
/// shared by two surfaces distinguished by the `q` parameter: the native
/// object-text API (the sbol-db REST client and its Python binding key on `q`),
/// and the SynBioHub V1 relevance search, which ranks the whole in-scope corpus
/// when no `q` is given.
#[derive(Debug, Default, Deserialize)]
pub struct SearchRootParams {
    pub q: Option<String>,
    pub object_type: Option<String>,
    pub property_uri: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

/// `GET /search` with no path grammar. A `q` parameter routes to the native
/// object-text search; its absence ranks the whole in-scope corpus through the
/// SynBioHub relevance path.
pub async fn search_root(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Query(params): Query<SearchRootParams>,
) -> Result<Response, ApiError> {
    if let Some(q) = params.q {
        let native = crate::routes::TextSearchParams {
            q,
            object_type: params.object_type,
            property_uri: params.property_uri,
            offset: params.offset.unwrap_or(0),
            limit: params.limit.unwrap_or(crate::routes::SEARCH_DEFAULT_LIMIT),
        };
        return Ok(crate::routes::text_search(State(state), Query(native))
            .await
            .into_response());
    }
    let paging = Paging {
        offset: params.offset.map(|o| o.max(0) as usize),
        limit: params.limit.map(|l| l.max(0) as usize),
    };
    run_search(state, user, String::new(), paging).await
}

/// `GET /search/<grammar>`: parse the classic path grammar and answer.
pub async fn search(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(query): Path<String>,
    Query(paging): Query<Paging>,
) -> Result<Response, ApiError> {
    run_search(state, user, query, paging).await
}

/// `GET /searchCount` with no query segment.
pub async fn search_count_root(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> Result<Response, ApiError> {
    run_search_count(state, user, String::new()).await
}

/// `GET /searchCount/<grammar>`.
pub async fn search_count(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(query): Path<String>,
) -> Result<Response, ApiError> {
    run_search_count(state, user, query).await
}

async fn run_search(
    state: AppState,
    user: Option<User>,
    grammar: String,
    paging: Paging,
) -> Result<Response, ApiError> {
    let mut faceted = parse_search_path(&grammar)?;
    faceted.offset = paging.offset.unwrap_or(0);
    faceted.limit = paging.limit;
    let scope = scope_for(&state, &user).await?;

    // Free text drives the tantivy relevance path; a purely faceted query
    // falls back to the accelerated SPARQL object list so counts and members
    // match the engine exactly.
    if faceted.free_text.is_some() {
        let (hits, _total) = state.app.ranked_search(&faceted, scope).await?;
        Ok(json_response(hits_to_solutions(&hits)))
    } else {
        run_scoped(&state, &queries::faceted(&faceted, false), scope).await
    }
}

async fn run_search_count(
    state: AppState,
    user: Option<User>,
    grammar: String,
) -> Result<Response, ApiError> {
    let faceted = parse_search_path(&grammar)?;
    let scope = scope_for(&state, &user).await?;
    if faceted.free_text.is_some() {
        let count = state.app.ranked_search_count(&faceted, scope).await?;
        Ok(json_response(count_to_solutions(count)))
    } else {
        run_scoped(&state, &queries::faceted(&faceted, true), scope).await
    }
}

// --- /:type/count ------------------------------------------------------------

/// `GET /:type/count`: the count of top-level objects of one SBOL2 type.
pub async fn type_count(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(type_name): Path<String>,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    run_scoped(&state, &queries::count(&type_name), scope).await
}

// --- /rootCollections --------------------------------------------------------

/// `GET /rootCollections`: Collections that are not members of any Collection.
pub async fn root_collections(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    run_scoped(&state, &queries::root_collections(), scope).await
}

// --- object-scoped: /uses /twins /subCollections /metadata -------------------

pub async fn public_uses(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    uses_impl(state.0, user.0 .0, public_uri(&object), false).await
}

pub async fn public_uses_count(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    uses_impl(state.0, user.0 .0, public_uri(&object), true).await
}

pub async fn public_twins(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    twins_impl(state.0, user.0 .0, public_uri(&object), false).await
}

pub async fn public_twins_count(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    twins_impl(state.0, user.0 .0, public_uri(&object), true).await
}

pub async fn public_sub_collections(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    sub_collections_impl(state.0, user.0 .0, public_uri(&object)).await
}

pub async fn public_metadata(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    metadata_impl(state.0, user.0 .0, public_uri(&object)).await
}

pub async fn user_uses(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    uses_impl(state.0, user.0 .0, user_uri(&object), false).await
}

pub async fn user_uses_count(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    uses_impl(state.0, user.0 .0, user_uri(&object), true).await
}

pub async fn user_twins(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    twins_impl(state.0, user.0 .0, user_uri(&object), false).await
}

pub async fn user_twins_count(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    twins_impl(state.0, user.0 .0, user_uri(&object), true).await
}

pub async fn user_sub_collections(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    sub_collections_impl(state.0, user.0 .0, user_uri(&object)).await
}

pub async fn user_metadata(
    state: State<AppState>,
    user: Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    metadata_impl(state.0, user.0 .0, user_uri(&object)).await
}

async fn uses_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
    count_only: bool,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    run_scoped(&state, &queries::uses(&uri, count_only), scope).await
}

async fn twins_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
    count_only: bool,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    run_scoped(&state, &queries::twins(&uri, count_only), scope).await
}

async fn sub_collections_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    run_scoped(&state, &queries::sub_collections(&uri), scope).await
}

async fn metadata_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    run_scoped(&state, &queries::metadata(&uri), scope).await
}

// --- /manage and /shared (identity-required) ---------------------------------

/// `GET /manage`: the top-level objects the caller owns (`sbh:ownedBy`).
/// Anonymous callers are rejected, matching classic's `requireUser`.
pub async fn manage(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> Result<Response, ApiError> {
    let Some(user) = user else {
        return Ok(unauthorized());
    };
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await?;
    run_scoped(&state, &queries::owned_by(&user.graph_uri), scope).await
}

/// `GET /shared`: the objects shared with the caller (`sbh:canView`).
pub async fn shared(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> Result<Response, ApiError> {
    let Some(user) = user else {
        return Ok(unauthorized());
    };
    let scope = state
        .app
        .acl_service
        .compute_scope(Some(&user.graph_uri))
        .await?;
    let objects = state.app.acl.viewable_objects(&user.graph_uri).await?;
    run_scoped(&state, &queries::metadata_of(&objects), scope).await
}

// --- shared helpers ----------------------------------------------------------

/// The caller's authorized graph scope, from the `X-authorization` identity.
async fn scope_for(state: &AppState, user: &Option<User>) -> Result<GraphScope, ApiError> {
    let user_graph = user.as_ref().map(|u| u.graph_uri.clone());
    let scope = state
        .app
        .acl_service
        .compute_scope(user_graph.as_deref())
        .await?;
    Ok(scope)
}

/// Run a SPARQL query under the caller's scope and return the engine's
/// SPARQL-results JSON verbatim.
async fn run_scoped(
    state: &AppState,
    query: &str,
    scope: GraphScope,
) -> Result<Response, ApiError> {
    let options = SparqlOptions {
        authorized_graphs: scope,
        ..SparqlOptions::default()
    };
    let outcome = state
        .app
        .sparql
        .execute(query, Some(ResultFormat::Json), None, &options)
        .await?;
    Response::builder()
        .header(CONTENT_TYPE, outcome.payload.content_type)
        .body(Body::from(outcome.payload.body))
        .map_err(|e| ApiError::Domain(DomainError::Serialization(e.to_string())))
}

/// A `401` for an endpoint that requires an authenticated caller.
fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "authentication required").into_response()
}

/// Wrap a SPARQL-results JSON value in a `application/sparql-results+json`
/// response, matching the engine's content type.
fn json_response(value: Value) -> Response {
    (
        [(CONTENT_TYPE, "application/sparql-results+json")],
        Json(value),
    )
        .into_response()
}

/// Project ranked hits into the SPARQL-results JSON shape `/search` emits, with
/// `head.vars` exactly `[subject, displayId, version, name, description, type]`.
fn hits_to_solutions(hits: &[Hit]) -> Value {
    let bindings: Vec<Value> = hits
        .iter()
        .map(|hit| {
            let mut binding = Map::new();
            binding.insert("subject".to_owned(), uri_node(&hit.subject));
            insert_literal(&mut binding, "displayId", hit.display_id.as_deref());
            insert_literal(&mut binding, "version", hit.version.as_deref());
            insert_literal(&mut binding, "name", hit.name.as_deref());
            insert_literal(&mut binding, "description", hit.description.as_deref());
            if let Some(type_iri) = &hit.type_iri {
                binding.insert("type".to_owned(), uri_node(type_iri));
            }
            Value::Object(binding)
        })
        .collect();
    json!({
        "head": { "vars": ["subject", "displayId", "version", "name", "description", "type"] },
        "results": { "bindings": bindings },
    })
}

/// The single-row `[{count}]` SPARQL-results JSON `/searchCount` emits.
fn count_to_solutions(count: usize) -> Value {
    json!({
        "head": { "vars": ["count"] },
        "results": {
            "bindings": [{
                "count": {
                    "type": "literal",
                    "value": count.to_string(),
                    "datatype": "http://www.w3.org/2001/XMLSchema#integer",
                },
            }],
        },
    })
}

fn uri_node(value: &str) -> Value {
    json!({ "type": "uri", "value": value })
}

fn insert_literal(binding: &mut Map<String, Value>, var: &str, value: Option<&str>) {
    if let Some(value) = value {
        binding.insert(var.to_owned(), json!({ "type": "literal", "value": value }));
    }
}
