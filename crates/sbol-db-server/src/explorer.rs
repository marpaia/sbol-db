//! Answers the SBOLExplorer query shapes SynBioHub sends to its SPARQL endpoint
//! (see [`sbol_db_sparql::recognize_explorer`]) from the facade's sequence and
//! ranked-text search, so sbol-db is a drop-in for SBOLExplorer as well as a
//! native REST provider.
//!
//! The public `/sparql` handler recognizes a shape before generic evaluation
//! and routes it here: `# SIMILAR:<uri>` to the cluster-mate lookup, a
//! `sbol2:elements "<seq>"` literal to the aligner, and the
//! `CONTAINS(lcase(?displayId), …)` free-text shape to the ranked-text index.
//! Every response carries SBOLExplorer's exact `head.vars`. The public endpoint
//! imposes no authorization ceiling, so reads run under the union scope.

use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sbol_db_app::{AlignMode, AlignOptions, FacetedSearch, Hit, SequenceAlignment};
use sbol_db_core::SbolObjectRecord;
use sbol_db_sparql::{
    explorer_count_results, explorer_search_results, ExplorerQuery, GraphScope, Paging,
};
use serde_json::{json, Map, Value};

use crate::{ApiError, AppState};

/// Route a recognized SBOLExplorer query to the facade and render its
/// SPARQL-results JSON.
pub(crate) async fn route(state: &AppState, query: ExplorerQuery) -> Result<Response, ApiError> {
    match query {
        ExplorerQuery::Similar { uri, paging } => similar(state, &uri, paging).await,
        ExplorerQuery::Sequence {
            sequence,
            exact,
            paging,
        } => sequence_search(state, &sequence, exact, paging).await,
        ExplorerQuery::Text { terms, paging } => text(state, terms, paging).await,
    }
}

/// `# SIMILAR:<uri>`: the target's cluster mates ranked by PageRank, or their
/// count. Mates carry no alignment columns.
async fn similar(state: &AppState, uri: &str, paging: Paging) -> Result<Response, ApiError> {
    let scope = GraphScope::Union;
    if paging.count {
        let count = state.app.sequence().similar_count(uri, &scope).await?;
        return Ok(json_response(explorer_count_results(count)));
    }
    let hits = state.app.sequence().similar(uri, &scope).await?;
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
) -> Result<Response, ApiError> {
    let scope = GraphScope::Union;
    let options = AlignOptions {
        mode: if exact {
            AlignMode::Exact
        } else {
            AlignMode::GlobalAlign
        },
        ..AlignOptions::default()
    };
    let hits = state
        .app
        .sequence()
        .align(sequence, options, &scope)
        .await?;
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
async fn text(state: &AppState, terms: Vec<String>, paging: Paging) -> Result<Response, ApiError> {
    let faceted = FacetedSearch {
        free_text: Some(terms.join(" ")),
        offset: paging.offset,
        limit: Some(paging.limit),
        ..FacetedSearch::default()
    };
    if paging.count {
        let count = state
            .app
            .ranked_search_count(&faceted, GraphScope::Union)
            .await?;
        return Ok(json_response(explorer_count_results(count)));
    }
    let (hits, _total) = state.app.ranked_search(&faceted, GraphScope::Union).await?;
    let bindings = hits.iter().map(hit_binding).collect();
    Ok(json_response(explorer_search_results(bindings)))
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
