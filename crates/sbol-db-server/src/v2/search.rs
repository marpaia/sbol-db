//! The V2 search resource.
//!
//! `GET /api/v2/search` runs a ranked free-text query over the in-scope corpus
//! and returns a paginated JSON envelope with a total. The query and paging
//! arrive as typed query parameters (`q`, `object_type`, `offset`, `limit`),
//! not the V1 `/search/key=value` path grammar. The handler delegates to the
//! same [`ranked_search`](sbol_db_app::AppServices::ranked_search) facade verb
//! the V1 adapter calls; the caller's [`GraphScope`](sbol_db_sparql::GraphScope)
//! is the read ceiling.

use axum::extract::{Query, State};
use axum::{Extension, Json};
use sbol_db_app::{FacetedSearch, Hit};
use sbol_db_search_sdk::{SearchPage as StructuredSearchPage, SearchRequest as StructuredRequest};
use serde::{Deserialize, Serialize};

use super::auth::{scope_for, Identity};
use crate::v2::error::V2Error;
use crate::AppState;

/// The default page size when the request names no `limit`.
const DEFAULT_LIMIT: usize = 50;
/// The largest page a single request may take.
const MAX_LIMIT: usize = 1000;

/// The typed query parameters shared by `GET /api/v2/search` and the
/// `GET /api/v2/objects` list, which is the same ranked query without a
/// dedicated free-text emphasis.
#[derive(Debug, Default, Deserialize)]
pub struct SearchParams {
    /// The free-text term. Absent ranks the whole in-scope corpus.
    pub q: Option<String>,
    /// Restrict to one rdf:type, given as a full IRI. Carried on the wire as
    /// `type`, the idiomatic facet name.
    #[serde(rename = "type")]
    pub object_type: Option<String>,
    /// The paging offset into the ranked results.
    #[serde(default)]
    pub offset: usize,
    /// The page size, clamped to `[1, MAX_LIMIT]`.
    pub limit: Option<usize>,
}

/// One ranked search hit as idiomatic JSON.
#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub uri: String,
    pub display_id: Option<String>,
    pub version: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub object_type: Option<String>,
}

impl From<Hit> for SearchHit {
    fn from(hit: Hit) -> Self {
        Self {
            uri: hit.subject,
            display_id: hit.display_id,
            version: hit.version,
            name: hit.name,
            description: hit.description,
            object_type: hit.type_iri,
        }
    }
}

/// The paginated search response: the window of hits plus the total number of
/// in-scope matches and the applied paging.
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub items: Vec<SearchHit>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

/// Discovery envelope for the immutable runtime assembled at startup.
#[derive(Debug, Serialize)]
pub struct StrategiesResponse {
    pub default_strategy: String,
    pub items: Vec<sbol_db_search_sdk::StrategyDescriptor>,
}

/// `GET /api/v2/search` — ranked, ACL-scoped, paginated free-text search.
pub async fn search(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, V2Error> {
    Ok(Json(run_search(&state, &identity, params).await?))
}

/// `POST /api/v2/search` — execute an explicitly structured, capability-checked
/// search strategy without changing the compatibility GET route.
pub async fn structured_search(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<StructuredRequest>,
) -> Result<Json<StructuredSearchPage>, V2Error> {
    let scope = scope_for(&state, &identity).await?;
    Ok(Json(state.app.structured_search(request, scope).await?))
}

/// `GET /api/v2/search/strategies` — discover the default strategy and the
/// declared capabilities of every registered implementation.
pub async fn strategies(State(state): State<AppState>) -> Json<StrategiesResponse> {
    let runtime = state.app.search_runtime();
    Json(StrategiesResponse {
        default_strategy: runtime.default_strategy().to_owned(),
        items: runtime.descriptors(),
    })
}

/// Run the ranked, ACL-scoped, paginated query behind both `/search` and the
/// `/objects` list. Delegates to the same
/// [`ranked_search`](sbol_db_app::AppServices::ranked_search) facade verb the V1
/// adapter calls; the caller's [`GraphScope`](sbol_db_sparql::GraphScope) is the
/// read ceiling.
pub(super) async fn run_search(
    state: &AppState,
    identity: &Identity,
    params: SearchParams,
) -> Result<SearchResponse, V2Error> {
    let scope = scope_for(state, identity).await?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let faceted = FacetedSearch {
        class: params.object_type,
        free_text: params.q,
        offset: params.offset,
        limit: Some(limit),
        ..FacetedSearch::default()
    };
    let (hits, total) = state.app.ranked_search(&faceted, scope).await?;
    Ok(SearchResponse {
        items: hits.into_iter().map(SearchHit::from).collect(),
        total,
        offset: params.offset,
        limit,
    })
}
