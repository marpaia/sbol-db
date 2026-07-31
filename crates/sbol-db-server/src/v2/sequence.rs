//! The V2 sequence resources: alignment search and cluster-mate `similar`.
//!
//! `GET /api/v2/sequences/search` aligns a query nucleotide string against the
//! indexed sequences and `GET /api/v2/objects/{iri}/similar` returns an object's
//! cluster mates, both through the same facade
//! [`SequenceService`](sbol_db_app::SequenceService) the V1 routes call. Unlike
//! V1, which emits the classic SPARQL-results projection, V2 returns idiomatic
//! JSON with a `total`. Every read is scoped to the caller's authorized graphs.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use sbol_db_app::{AlignMode, AlignOptions};
use sbol_db_core::{DomainError, SbolObjectRecord};
use serde::{Deserialize, Serialize};

use super::auth::{scope_for, Identity};
use crate::error::ApiError;
use crate::v2::error::V2Error;
use crate::AppState;

/// The default page size when the request names no `limit`.
const DEFAULT_LIMIT: usize = 50;
/// The largest page a single request may take.
const MAX_LIMIT: usize = 1000;

/// The typed query parameters for `GET /api/v2/sequences/search`.
#[derive(Debug, Default, Deserialize)]
pub struct SequenceSearchParams {
    /// The query nucleotide string. `sequence` is accepted as an alias.
    pub q: Option<String>,
    pub sequence: Option<String>,
    /// The alignment mode: `global` (banded aligner, the default) or `exact`
    /// (exact substring).
    pub mode: Option<String>,
    /// The maximum number of hits to return, clamped to `[1, MAX_LIMIT]`.
    /// Kept as text until the handler so malformed values receive the V2 JSON
    /// error envelope instead of Axum's extractor plain-text rejection.
    pub limit: Option<String>,
}

/// The shared object-metadata columns carried by a sequence or `similar` hit.
#[derive(Debug, Default, Serialize)]
pub struct ObjectMeta {
    pub display_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub object_type: Option<String>,
}

impl ObjectMeta {
    fn from_record(record: Option<&SbolObjectRecord>) -> Self {
        match record {
            Some(r) => Self {
                display_id: r.display_id.clone(),
                name: r.name.clone(),
                description: r.description.clone(),
                object_type: Some(r.sbol_class.clone()),
            },
            None => Self::default(),
        }
    }
}

/// One sequence-search hit: the aligned object plus the alignment measures.
#[derive(Debug, Serialize)]
pub struct SequenceHit {
    pub uri: String,
    pub percent_match: f64,
    pub strand: String,
    pub cigar: String,
    #[serde(flatten)]
    pub meta: ObjectMeta,
}

/// The paginated sequence-search response.
#[derive(Debug, Serialize)]
pub struct SequenceSearchResponse {
    pub items: Vec<SequenceHit>,
    pub total: usize,
}

/// `GET /api/v2/sequences/search` — align the query against the in-scope
/// indexed sequences, ordered by `pagerank * percentMatch`.
pub async fn search_sequences(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(params): Query<SequenceSearchParams>,
) -> Result<Json<SequenceSearchResponse>, V2Error> {
    let scope = scope_for(&state, &identity).await?;
    let query = params
        .q
        .or(params.sequence)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            V2Error::from(ApiError::BadRequest(
                "q (the query sequence) is required".to_owned(),
            ))
        })?;
    let mode = parse_mode(params.mode.as_deref())?;
    let max_accepts = parse_limit(params.limit.as_deref())? as u32;
    let options = AlignOptions {
        mode,
        max_accepts,
        ..AlignOptions::default()
    };

    let hits = state.app.sequence().align(&query, options, &scope).await?;
    let iris: Vec<&str> = hits.iter().map(|h| h.sequence_iri.as_str()).collect();
    let records = state
        .service
        .get_objects_by_iris(&iris)
        .await
        .map_err(V2Error::from)?;
    let by_iri = index_by_iri(&records);

    let items: Vec<SequenceHit> = hits
        .iter()
        .map(|hit| SequenceHit {
            uri: hit.sequence_iri.clone(),
            percent_match: hit.percent_match,
            strand: hit.strand.to_string(),
            cigar: hit.cigar.clone(),
            meta: ObjectMeta::from_record(by_iri.get(hit.sequence_iri.as_str()).copied()),
        })
        .collect();
    let total = items.len();
    Ok(Json(SequenceSearchResponse { items, total }))
}

/// One `similar` hit: a cluster mate and its PageRank score.
#[derive(Debug, Serialize)]
pub struct SimilarHitJson {
    pub uri: String,
    pub pagerank: f64,
    #[serde(flatten)]
    pub meta: ObjectMeta,
}

/// The paginated `similar` response.
#[derive(Debug, Serialize)]
pub struct SimilarResponse {
    pub items: Vec<SimilarHitJson>,
    pub total: usize,
}

/// `GET /api/v2/objects/{iri}/similar` — the object's in-scope cluster mates,
/// ranked by PageRank.
pub async fn similar(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
) -> Result<Json<SimilarResponse>, V2Error> {
    let scope = scope_for(&state, &identity).await?;
    let hits = state.app.sequence().similar(&iri, &scope).await?;
    let iris: Vec<&str> = hits.iter().map(|h| h.iri.as_str()).collect();
    let records = state
        .service
        .get_objects_by_iris(&iris)
        .await
        .map_err(V2Error::from)?;
    let by_iri = index_by_iri(&records);

    let items: Vec<SimilarHitJson> = hits
        .iter()
        .map(|hit| SimilarHitJson {
            uri: hit.iri.clone(),
            pagerank: hit.pagerank,
            meta: ObjectMeta::from_record(by_iri.get(hit.iri.as_str()).copied()),
        })
        .collect();
    let total = items.len();
    Ok(Json(SimilarResponse { items, total }))
}

/// Map object records by their IRI for per-hit metadata lookup.
fn index_by_iri(records: &[SbolObjectRecord]) -> HashMap<&str, &SbolObjectRecord> {
    records.iter().map(|r| (r.iri.as_str(), r)).collect()
}

/// Parse the alignment mode word. Absent or `global` runs the banded aligner;
/// `exact` takes the exact substring path.
fn parse_mode(mode: Option<&str>) -> Result<AlignMode, V2Error> {
    match mode.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("global") | Some("globalalign") => Ok(AlignMode::GlobalAlign),
        Some("exact") => Ok(AlignMode::Exact),
        Some(other) => Err(V2Error::from(ApiError::BadRequest(format!(
            "unknown alignment mode: {other}"
        )))),
    }
}

fn parse_limit(value: Option<&str>) -> Result<usize, V2Error> {
    let Some(value) = value else {
        return Ok(DEFAULT_LIMIT);
    };
    let limit = value.trim().parse::<usize>().map_err(|_| {
        V2Error::from(DomainError::InvalidInput(
            "limit must be a non-negative integer".to_owned(),
        ))
    })?;
    Ok(limit.clamp(1, MAX_LIMIT))
}
