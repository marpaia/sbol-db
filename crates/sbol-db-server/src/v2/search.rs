//! The V2 search resource.
//!
//! `GET /api/v2/search` runs the normalized native discovery contract over the
//! caller's in-scope corpus. It supports text, biological facets, collection
//! and ownership context, provenance/date narrowing, deterministic sorting,
//! and stable offset paging without inheriting V1's path grammar.

use axum::extract::{Query, State};
use axum::{Extension, Json};
use chrono::NaiveDate;
use sbol_db_app::{DiscoveryFacets, DiscoveryPage, DiscoveryQuery, DiscoverySort, SortDirection};
use sbol_db_core::DomainError;
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
    /// The free-text term. Absent browses the whole in-scope corpus.
    pub q: Option<String>,
    /// Restrict to one rdf:type, given as a full IRI. Carried on the wire as
    /// `type`, the idiomatic facet name.
    #[serde(rename = "type")]
    pub object_type: Option<String>,
    /// Restrict to objects carrying this full role IRI (SBOL 2 or SBOL 3).
    pub role: Option<String>,
    /// Restrict to direct members of this collection IRI.
    pub collection: Option<String>,
    /// Restrict to objects carrying `sbh:ownedBy` for this owner graph IRI.
    pub owner: Option<String>,
    /// Case-insensitive substring over `sbh:mutableProvenance`.
    pub provenance: Option<String>,
    pub created_after: Option<String>,
    pub created_before: Option<String>,
    pub modified_after: Option<String>,
    pub modified_before: Option<String>,
    /// `relevance`, `name`, `created`, `modified`, or `iri`.
    pub sort: Option<String>,
    /// `asc` or `desc`; omitted uses the natural direction for the sort.
    pub direction: Option<String>,
    /// Kept as strings until the handler so malformed values receive the V2
    /// JSON error envelope instead of Axum's extractor plain-text rejection.
    pub offset: Option<String>,
    pub limit: Option<String>,
}

pub type SearchResponse = DiscoveryPage;

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

/// `GET /api/v2/search/facets` — exact visible type/role counts with ontology
/// labels where the deployment has loaded them.
pub async fn facets(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<DiscoveryFacets>, V2Error> {
    let scope = scope_for(&state, &identity).await?;
    Ok(Json(state.app.discovery_facets(scope).await?))
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

/// Run the normalized native discovery query behind both `/search` and the
/// `/objects` list. The application facade owns filtering, ranking, sorting,
/// exact totals, and paging; this adapter only parses the wire representation.
pub(super) async fn run_search(
    state: &AppState,
    identity: &Identity,
    params: SearchParams,
) -> Result<SearchResponse, V2Error> {
    let scope = scope_for(state, identity).await?;
    let sort = parse_sort(params.sort.as_deref())?;
    let direction = parse_direction(params.direction.as_deref(), sort)?;
    let offset = parse_usize("offset", params.offset.as_deref(), 0)?;
    let limit = parse_usize("limit", params.limit.as_deref(), DEFAULT_LIMIT)?.clamp(1, MAX_LIMIT);
    let query = DiscoveryQuery {
        text: non_empty(params.q),
        object_type: non_empty(params.object_type),
        role: non_empty(params.role),
        collection: non_empty(params.collection),
        owner: non_empty(params.owner),
        provenance: non_empty(params.provenance),
        created_after: parse_date("created_after", params.created_after.as_deref())?,
        created_before: parse_date("created_before", params.created_before.as_deref())?,
        modified_after: parse_date("modified_after", params.modified_after.as_deref())?,
        modified_before: parse_date("modified_before", params.modified_before.as_deref())?,
        sort,
        direction,
        offset,
        limit,
    };
    state.app.discover(&query, scope).await.map_err(Into::into)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_usize(name: &str, value: Option<&str>, default: usize) -> Result<usize, V2Error> {
    value
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                V2Error::from(DomainError::InvalidInput(format!(
                    "{name} must be a non-negative integer"
                )))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_date(name: &str, value: Option<&str>) -> Result<Option<NaiveDate>, V2Error> {
    value
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
                V2Error::from(DomainError::InvalidInput(format!(
                    "{name} must use YYYY-MM-DD"
                )))
            })
        })
        .transpose()
}

fn parse_sort(value: Option<&str>) -> Result<DiscoverySort, V2Error> {
    match value.unwrap_or("relevance") {
        "relevance" => Ok(DiscoverySort::Relevance),
        "name" => Ok(DiscoverySort::Name),
        "created" => Ok(DiscoverySort::Created),
        "modified" => Ok(DiscoverySort::Modified),
        "iri" => Ok(DiscoverySort::Iri),
        other => {
            Err(DomainError::InvalidInput(format!("unsupported discovery sort: {other}")).into())
        }
    }
}

fn parse_direction(value: Option<&str>, sort: DiscoverySort) -> Result<SortDirection, V2Error> {
    match value {
        Some("asc") => Ok(SortDirection::Asc),
        Some("desc") => Ok(SortDirection::Desc),
        Some(other) => Err(DomainError::InvalidInput(format!(
            "unsupported discovery direction: {other}"
        ))
        .into()),
        None if matches!(sort, DiscoverySort::Name | DiscoverySort::Iri) => Ok(SortDirection::Asc),
        None => Ok(SortDirection::Desc),
    }
}
