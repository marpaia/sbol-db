//! Answers the SBOLExplorer query shapes SynBioHub sends to its SPARQL endpoint
//! (see [`sbol_db_sparql::recognize_explorer`]) from the facade's sequence and
//! ranked-text search, so sbol-db is a drop-in for SBOLExplorer as well as a
//! native REST provider.
//!
//! The dedicated compatibility listener recognizes a shape and routes it here:
//! `# SIMILAR:<uri>` to the cluster-mate lookup, a `sbol2:elements "<seq>"`
//! literal to the aligner, and the `CONTAINS(lcase(?displayId), …)` free-text
//! shape to the ranked-text index. Every response carries SBOLExplorer's exact
//! `head.vars`. The main `/sparql` endpoint remains a literal triplestore
//! surface and never enters this adapter.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sbol_db_app::{AlignMode, AlignOptions, FacetedSearch, Hit, SequenceAlignment};
use sbol_db_core::SbolObjectRecord;
use sbol_db_sparql::{
    explorer_count_results, explorer_graph_scope, explorer_search_results, ExplorerQuery,
    GraphScope, Paging,
};
use sbol_db_storage::{EnqueueOutcome, ListJobsFilter, NewJob};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{ApiError, AppState};

/// Route a recognized SBOLExplorer query to the facade and render its
/// SPARQL-results JSON.
pub(crate) async fn route(
    state: &AppState,
    query: ExplorerQuery,
    scope: GraphScope,
) -> Result<Response, ApiError> {
    match query {
        ExplorerQuery::Browse { paging } => text(state, Vec::new(), paging, scope).await,
        ExplorerQuery::Similar { uri, paging } => similar(state, &uri, paging, &scope).await,
        ExplorerQuery::Sequence {
            sequence,
            exact,
            paging,
        } => sequence_search(state, &sequence, exact, paging, &scope).await,
        ExplorerQuery::Text { terms, paging } => text(state, terms, paging, scope).await,
    }
}

/// `# SIMILAR:<uri>`: the target's cluster mates ranked by PageRank, or their
/// count. Mates carry no alignment columns.
async fn similar(
    state: &AppState,
    uri: &str,
    paging: Paging,
    scope: &GraphScope,
) -> Result<Response, ApiError> {
    if paging.count {
        let count = state.app.sequence().similar_count(uri, scope).await?;
        return Ok(json_response(explorer_count_results(count)));
    }
    let hits = state.app.sequence().similar(uri, scope).await?;
    let iris: Vec<&str> = hits.iter().map(|h| h.iri.as_str()).collect();
    let records = state.service.get_objects_by_iris(&iris).await?;
    let by_iri = index_by_iri(&records);
    let bindings = hits
        .iter()
        .skip(paging.offset)
        .take(paging.limit)
        .map(|hit| {
            Value::Object(base_binding(
                &hit.iri,
                by_iri.get(hit.iri.as_str()).copied(),
            ))
        })
        .collect();
    Ok(json_response(explorer_search_results(bindings)))
}

/// `?seq sbol2:elements "<seq>"`: align the query against the indexed sequences,
/// or count the hits. Rows carry `percentMatch`/`strandAlignment`/`CIGAR`.
async fn sequence_search(
    state: &AppState,
    sequence: &str,
    exact: bool,
    paging: Paging,
    scope: &GraphScope,
) -> Result<Response, ApiError> {
    let options = AlignOptions {
        mode: if exact {
            AlignMode::Exact
        } else {
            AlignMode::GlobalAlign
        },
        ..AlignOptions::default()
    };
    let hits = state.app.sequence().align(sequence, options, scope).await?;
    if paging.count {
        return Ok(json_response(explorer_count_results(hits.len())));
    }
    let iris: Vec<&str> = hits.iter().map(|h| h.sequence_iri.as_str()).collect();
    let records = state.service.get_objects_by_iris(&iris).await?;
    let by_iri = index_by_iri(&records);
    let bindings = hits
        .iter()
        .skip(paging.offset)
        .take(paging.limit)
        .map(|hit| alignment_binding(hit, by_iri.get(hit.sequence_iri.as_str()).copied()))
        .collect();
    Ok(json_response(explorer_search_results(bindings)))
}

/// The ranked free-text shape: rank the keyword match through the tantivy index,
/// or count the matches. The ranked path applies paging internally.
async fn text(
    state: &AppState,
    terms: Vec<String>,
    paging: Paging,
    scope: GraphScope,
) -> Result<Response, ApiError> {
    let faceted = FacetedSearch {
        free_text: Some(terms.join(" ")),
        offset: paging.offset,
        limit: Some(paging.limit),
        ..FacetedSearch::default()
    };
    if paging.count {
        let count = state.app.ranked_search_count(&faceted, scope).await?;
        return Ok(json_response(explorer_count_results(count)));
    }
    let (hits, _total) = state.app.ranked_search(&faceted, scope).await?;
    let bindings = hits.iter().map(hit_binding).collect();
    Ok(json_response(explorer_search_results(bindings)))
}

const EXPLORER_CONFIG_KEY: &str = "sbolexplorer_compat";
const REINDEX_KIND: &str = "rebuild_search_index";

#[derive(Debug, Deserialize)]
pub(crate) struct ExplorerRequest {
    query: Option<String>,
    #[serde(rename = "default-graph-uri")]
    default_graph_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateRequest {
    subject: Option<String>,
}

/// `GET /` on the dedicated compatibility listener. This is deliberately
/// separate from the main `/sparql` route so classic SynBioHub can keep its
/// stock `http://explorer:13162/` configuration while both surfaces share one
/// process-local index and [`AppState`].
pub(crate) async fn endpoint(
    State(state): State<AppState>,
    Query(params): Query<ExplorerRequest>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Some(query) = params.query else {
        return Ok((
            [(CONTENT_TYPE, "text/plain; charset=utf-8")],
            "sbol-db SBOLExplorer compatibility listener\n",
        )
            .into_response());
    };
    if let Some(recognized) = sbol_db_sparql::recognize_explorer(&query) {
        let scope = explorer_graph_scope(&query, params.default_graph_uri.as_deref());
        return route(&state, recognized, scope).await;
    }

    // Stock SBOLExplorer delegates advanced facets, USES, and TWINS back to
    // its configured SPARQL store. This listener already shares sbol-db's
    // store, so run those shapes directly. Refuse a request with no protocol
    // default and no explicit FROM instead of accidentally evaluating it over
    // the union of every graph.
    if matches!(
        explorer_graph_scope(&query, params.default_graph_uri.as_deref()),
        GraphScope::Only(ref graphs) if graphs.is_empty()
    ) {
        return Err(ApiError::BadRequest(
            "Explorer query has no allowed dataset".to_owned(),
        ));
    }
    crate::routes::sparql_get(
        State(state),
        Query(crate::routes::SparqlGetParams {
            query: Some(query),
            format: None,
            default_graph_uri: params.default_graph_uri,
        }),
        headers,
    )
    .await
    .map(IntoResponse::into_response)
}

fn default_config() -> Value {
    json!({
        "engine": "sbol-db",
        "uclust_identity": "0.8",
        "elasticsearch_index_name": "native",
        "pagerank_tolerance": "0.0001",
        "elasticsearch_endpoint": "native://embedded/",
        "sparql_endpoint": "native://shared-store/sparql?",
        "last_update_start": "managed by sbol-db jobs",
        "last_update_end": "managed by sbol-db jobs",
        "distributed_search": false,
        "which_search": "vsearch",
        "autoUpdateIndex": false,
        "updateTimeInDays": "1"
    })
}

pub(crate) async fn get_config(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .app
            .config_service()
            .get(EXPLORER_CONFIG_KEY)
            .await?
            .unwrap_or_else(default_config),
    ))
}

pub(crate) async fn set_config(
    State(state): State<AppState>,
    Json(update): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let update = update
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("Explorer config must be a JSON object".to_owned()))?;
    let mut merged = state
        .app
        .config_service()
        .get(EXPLORER_CONFIG_KEY)
        .await?
        .unwrap_or_else(default_config)
        .as_object()
        .cloned()
        .unwrap_or_default();
    merged.extend(update.clone());
    let merged = Value::Object(merged);
    state
        .app
        .config_service()
        // The listener is intended for the same trusted internal network on
        // which classic SynBioHub talks to SBOLExplorer.
        .set(true, EXPLORER_CONFIG_KEY, &merged)
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    Ok(Json(merged))
}

pub(crate) async fn update(
    State(state): State<AppState>,
    Query(params): Query<UpdateRequest>,
) -> Response {
    let job = NewJob {
        kind: REINDEX_KIND.to_owned(),
        payload: json!({}),
        queue: None,
        priority: None,
        max_attempts: None,
        idempotency_key: None,
        available_at: None,
        parent_job_id: None,
        correlation_id: None,
    };
    match state.jobs.enqueue(job).await {
        Ok(EnqueueOutcome::Inserted(job)) | Ok(EnqueueOutcome::AlreadyExists(job)) => {
            let message = params
                .subject
                .map(|subject| format!("Successfully scheduled refresh for: {subject}"))
                .unwrap_or_else(|| "Successfully scheduled entire index update".to_owned());
            (
                StatusCode::ACCEPTED,
                [("X-SBOL-DB-Job-Id", job.id.to_string())],
                message,
            )
                .into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unable to schedule index update: {error}"),
        )
            .into_response(),
    }
}

/// Classic may send these notifications before its graph-store write commits.
/// The committed write surface is authoritative and already schedules native
/// reconciliation, so acknowledging the legacy signal without indexing its
/// partial payload avoids a race against the actual stored graph.
pub(crate) async fn incremental_update(_body: Bytes) -> impl IntoResponse {
    "Successfully acknowledged incremental update"
}

pub(crate) async fn incremental_remove() -> impl IntoResponse {
    "Successfully acknowledged incremental remove"
}

pub(crate) async fn incremental_remove_collection() -> impl IntoResponse {
    "Successfully acknowledged incremental collection remove"
}

pub(crate) async fn info() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/plain; charset=utf-8")],
        "sbol-db SBOLExplorer compatibility listener is ready\n",
    )
}

pub(crate) async fn indexing_info(State(state): State<AppState>) -> Response {
    let filter = ListJobsFilter {
        kind: Some(REINDEX_KIND.to_owned()),
        limit: 1,
        ..ListJobsFilter::default()
    };
    match state.jobs.list(&filter).await {
        Ok(jobs) => {
            let body = jobs.first().map_or_else(
                || "No index update has been scheduled\n".to_owned(),
                |job| {
                    format!(
                        "job_id={} status={} created_at={} finished_at={}\n",
                        job.id,
                        job.status.as_db_str(),
                        job.created_at.to_rfc3339(),
                        job.finished_at
                            .map(|timestamp| timestamp.to_rfc3339())
                            .unwrap_or_else(|| "-".to_owned())
                    )
                },
            );
            ([(CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unable to read index lifecycle: {error}"),
        )
            .into_response(),
    }
}

/// Map object records by IRI for per-hit metadata lookup.
fn index_by_iri(
    records: &[SbolObjectRecord],
) -> std::collections::HashMap<&str, &SbolObjectRecord> {
    records.iter().map(|r| (r.iri.as_str(), r)).collect()
}

/// A sequence-search binding: object metadata plus the alignment columns.
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

/// A ranked-text binding from a [`Hit`]: subject plus the metadata columns the
/// index carries.
fn hit_binding(hit: &Hit) -> Value {
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
}

/// The shared object-metadata columns of a binding: subject and, when the object
/// is known, its displayId/name/description/type.
fn base_binding(iri: &str, record: Option<&SbolObjectRecord>) -> Map<String, Value> {
    let mut binding = Map::new();
    binding.insert("subject".to_owned(), uri_node(iri));
    if let Some(record) = record {
        insert_literal(&mut binding, "displayId", record.display_id.as_deref());
        insert_literal(&mut binding, "name", record.name.as_deref());
        insert_literal(&mut binding, "description", record.description.as_deref());
        binding.insert("type".to_owned(), uri_node(&record.sbol_class));
    }
    binding
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

fn insert_literal(binding: &mut Map<String, Value>, var: &str, value: Option<&str>) {
    if let Some(value) = value {
        binding.insert(var.to_owned(), literal(value));
    }
}
