//! SynBioHub v1 sequence-search and `/similar` routes.
//!
//! Sequence search (`/search/sequence=…`, `globalsequence=…`, `exactsequence=…`)
//! aligns the query against the indexed sequences through the facade's
//! [`SequenceService`](sbol_db_app::SequenceService) and returns the classic
//! nine-column projection carrying `percentMatch`/`strandAlignment`/`CIGAR`,
//! ordered by `pagerank * percentMatch`. `<uri>/similar` and
//! `<uri>/similarCount` return the target's cluster mates ranked by PageRank
//! alone. Every read is scoped to the caller's authorized graphs.

use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_app::{AlignOptions, SequenceAlignment, SimilarHit};
use sbol_db_core::{SbolObjectRecord, User};
use serde_json::{json, Map, Value};

use super::routes::{public_uri, scope_for, user_uri, PublicObject, UserObject};
use super::search::SequenceQuery;
use super::CurrentUser;
use crate::{ApiError, AppState};

/// The ordered `head.vars` classic SynBioHub emits for the sequence-search and
/// `/similar` projections.
const VARS: [&str; 9] = [
    "subject",
    "displayId",
    "version",
    "name",
    "description",
    "type",
    "percentMatch",
    "strandAlignment",
    "CIGAR",
];

/// Run a sequence search extracted from the `/search` grammar under the caller's
/// scope and render the nine-column projection.
pub(super) async fn run_sequence_search(
    state: &AppState,
    user: &Option<User>,
    query: SequenceQuery,
) -> Result<Response, ApiError> {
    let scope = scope_for(state, user).await?;
    let options = AlignOptions {
        mode: query.mode,
        ..AlignOptions::default()
    };
    let hits = state
        .app
        .sequence()
        .align(&query.sequence, options, &scope)
        .await?;

    let iris: Vec<&str> = hits.iter().map(|h| h.sequence_iri.as_str()).collect();
    let records = state.service.get_objects_by_iris(&iris).await?;
    let by_iri = index_by_iri(&records);

    let bindings: Vec<Value> = hits
        .iter()
        .map(|hit| alignment_binding(hit, by_iri.get(hit.sequence_iri.as_str()).copied()))
        .collect();
    Ok(json_response(solutions(bindings)))
}

/// `GET /public/:collectionId/:displayId/:version/similar`.
pub async fn public_similar(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    similar_impl(state, user, public_uri(&object)).await
}

/// `GET /public/:collectionId/:displayId/:version/similarCount`.
pub async fn public_similar_count(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    similar_count_impl(state, user, public_uri(&object)).await
}

/// `GET /user/:userId/:collectionId/:displayId/:version/similar`.
pub async fn user_similar(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    similar_impl(state, user, user_uri(&object)).await
}

/// `GET /user/:userId/:collectionId/:displayId/:version/similarCount`.
pub async fn user_similar_count(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    similar_count_impl(state, user, user_uri(&object)).await
}

async fn similar_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    let hits: Vec<SimilarHit> = state.app.sequence().similar(&uri, &scope).await?;

    let iris: Vec<&str> = hits.iter().map(|h| h.iri.as_str()).collect();
    let records = state.service.get_objects_by_iris(&iris).await?;
    let by_iri = index_by_iri(&records);

    let bindings: Vec<Value> = hits
        .iter()
        .map(|hit| similar_binding(&hit.iri, by_iri.get(hit.iri.as_str()).copied()))
        .collect();
    Ok(json_response(solutions(bindings)))
}

async fn similar_count_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    let count = state.app.sequence().similar_count(&uri, &scope).await?;
    Ok(json_response(count_solutions(count)))
}

/// Map object records by their IRI for per-hit metadata lookup.
fn index_by_iri(
    records: &[SbolObjectRecord],
) -> std::collections::HashMap<&str, &SbolObjectRecord> {
    records.iter().map(|r| (r.iri.as_str(), r)).collect()
}

/// A sequence-search binding: the object metadata plus the alignment's
/// `percentMatch`/`strandAlignment`/`CIGAR`.
fn alignment_binding(hit: &SequenceAlignment, record: Option<&SbolObjectRecord>) -> Value {
    let mut binding = base_binding(&hit.sequence_iri, record);
    binding.insert(
        "percentMatch".to_owned(),
        literal(&hit.percent_match.to_string()),
    );
    binding.insert(
        "strandAlignment".to_owned(),
        literal(&hit.strand.to_string()),
    );
    binding.insert("CIGAR".to_owned(), literal(&hit.cigar));
    Value::Object(binding)
}

/// A `/similar` binding: object metadata only, no alignment columns (matching
/// SBOLExplorer's cluster-mate contract).
fn similar_binding(iri: &str, record: Option<&SbolObjectRecord>) -> Value {
    Value::Object(base_binding(iri, record))
}

/// The shared object-metadata columns of a binding: subject and, when the
/// object is known, its displayId/name/description/type.
fn base_binding(iri: &str, record: Option<&SbolObjectRecord>) -> Map<String, Value> {
    let mut binding = Map::new();
    binding.insert("subject".to_owned(), uri_node(iri));
    if let Some(record) = record {
        if let Some(display_id) = &record.display_id {
            binding.insert("displayId".to_owned(), literal(display_id));
        }
        if let Some(name) = &record.name {
            binding.insert("name".to_owned(), literal(name));
        }
        if let Some(description) = &record.description {
            binding.insert("description".to_owned(), literal(description));
        }
        binding.insert("type".to_owned(), uri_node(&record.sbol_class));
    }
    binding
}

fn solutions(bindings: Vec<Value>) -> Value {
    json!({
        "head": { "vars": VARS },
        "results": { "bindings": bindings },
    })
}

fn count_solutions(count: usize) -> Value {
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

fn json_response(value: Value) -> Response {
    (
        [(CONTENT_TYPE, "application/sparql-results+json")],
        Json(value),
    )
        .into_response()
}

fn uri_node(value: &str) -> Value {
    json!({ "type": "uri", "value": value })
}

fn literal(value: &str) -> Value {
    json!({ "type": "literal", "value": value })
}
