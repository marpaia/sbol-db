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
use axum::response::Response;
use axum::Extension;
use sbol_db_app::{AlignOptions, SequenceAlignment, SimilarHit};
use sbol_db_core::{ObjectTerm, Triple, User};
use serde_json::{json, Map, Value};

use super::routes::{
    public_pi_uri, public_uri, resolve_pi, scope_for, user_pi_uri, user_uri, PublicObject,
    PublicObjectPi, UserObject, UserObjectPi,
};
use super::search::SequenceQuery;
use super::{render, CurrentUser};
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

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
const VERSION: &str = "http://sbols.org/v2#version";
const TITLE: &str = "http://purl.org/dc/terms/title";
const DESCRIPTION: &str = "http://purl.org/dc/terms/description";

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

    let mut bindings: Vec<Value> = Vec::with_capacity(hits.len());
    for hit in &hits {
        let meta = fetch_metadata(state, &hit.sequence_iri).await?;
        bindings.push(alignment_binding(hit, &meta));
    }
    Ok(render::search_response(&solutions(bindings)))
}

/// Count the matches of a sequence search, for `/searchCount/<sequence facet>`.
pub(super) async fn run_sequence_count(
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
    Ok(render::count_response(&count_solutions(hits.len())))
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

/// `GET /public/:collectionId/:displayId/similar` (version-less): resolve the
/// persistent identity to the latest version, then list similar objects.
pub async fn public_similar_pi(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObjectPi>,
) -> Result<Response, ApiError> {
    let uri = resolve_pi(&state, &user, public_pi_uri(&object)).await?;
    similar_impl(state, user, uri).await
}

pub async fn public_similar_count_pi(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObjectPi>,
) -> Result<Response, ApiError> {
    let uri = resolve_pi(&state, &user, public_pi_uri(&object)).await?;
    similar_count_impl(state, user, uri).await
}

pub async fn user_similar_pi(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObjectPi>,
) -> Result<Response, ApiError> {
    let uri = resolve_pi(&state, &user, user_pi_uri(&object)).await?;
    similar_impl(state, user, uri).await
}

pub async fn user_similar_count_pi(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObjectPi>,
) -> Result<Response, ApiError> {
    let uri = resolve_pi(&state, &user, user_pi_uri(&object)).await?;
    similar_count_impl(state, user, uri).await
}

async fn similar_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    let hits: Vec<SimilarHit> = state.app.sequence().similar(&uri, &scope).await?;

    let mut bindings: Vec<Value> = Vec::with_capacity(hits.len());
    for hit in &hits {
        let meta = fetch_metadata(&state, &hit.iri).await?;
        bindings.push(similar_binding(&hit.iri, &meta));
    }
    Ok(render::search_response(&solutions(bindings)))
}

async fn similar_count_impl(
    state: AppState,
    user: Option<User>,
    uri: String,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    let count = state.app.sequence().similar_count(&uri, &scope).await?;
    Ok(render::count_response(&count_solutions(count)))
}

/// The object-metadata columns a search/`similar` binding carries, projected
/// from the object's own triples: the SBOL `type`, `displayId`, `version`,
/// `name` (`dcterms:title`), and `description` (`dcterms:description`). Reading
/// the object's triples resolves for every write path, including the shared
/// public graph a `makePublic` writes verbatim, where the `sbol_objects`
/// derived view is not materialized.
#[derive(Default)]
struct RowMeta {
    type_iri: Option<String>,
    display_id: Option<String>,
    version: Option<String>,
    name: Option<String>,
    description: Option<String>,
}

/// Read `iri`'s triples and project the binding metadata, matching
/// SBOLExplorer's SPARQL projection of a search row.
async fn fetch_metadata(state: &AppState, iri: &str) -> Result<RowMeta, ApiError> {
    let triples = state.service.triples_for_subject(iri).await?;
    Ok(project_metadata(&triples))
}

/// Project one object's binding metadata from its triples. `rdf:type` prefers an
/// SBOL vocabulary class (classic reports the SBOL type, not an ancillary
/// `rdf:type`); the single-valued fields keep the first value seen.
fn project_metadata(triples: &[Triple]) -> RowMeta {
    let mut meta = RowMeta::default();
    for triple in triples {
        let predicate = triple.predicate.as_str();
        match predicate {
            RDF_TYPE => {
                if let ObjectTerm::Iri(iri) = &triple.object {
                    let iri = iri.as_str();
                    let is_sbol = iri.contains("sbols.org");
                    let have_sbol = meta
                        .type_iri
                        .as_deref()
                        .is_some_and(|current| current.contains("sbols.org"));
                    if meta.type_iri.is_none() || (is_sbol && !have_sbol) {
                        meta.type_iri = Some(iri.to_owned());
                    }
                }
            }
            DISPLAY_ID => set_first(&mut meta.display_id, literal_value(&triple.object)),
            VERSION => set_first(&mut meta.version, literal_value(&triple.object)),
            TITLE => set_first(&mut meta.name, literal_value(&triple.object)),
            DESCRIPTION => set_first(&mut meta.description, literal_value(&triple.object)),
            _ => {}
        }
    }
    meta
}

fn set_first(slot: &mut Option<String>, value: Option<String>) {
    if slot.is_none() {
        *slot = value;
    }
}

fn literal_value(object: &ObjectTerm) -> Option<String> {
    match object {
        ObjectTerm::Literal { value, .. } => Some(value.clone()),
        _ => None,
    }
}

/// A sequence-search binding: the object metadata plus the alignment's
/// `percentMatch`/`strandAlignment`/`CIGAR`.
fn alignment_binding(hit: &SequenceAlignment, meta: &RowMeta) -> Value {
    let mut binding = base_binding(&hit.sequence_iri, meta);
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
fn similar_binding(iri: &str, meta: &RowMeta) -> Value {
    Value::Object(base_binding(iri, meta))
}

/// The shared object-metadata columns of a binding: subject plus the projected
/// type/displayId/version/name/description. A field absent from the object is
/// left unbound so the search view supplies classic's default (`''` for text,
/// `null` for `sbolType`/`role`).
fn base_binding(iri: &str, meta: &RowMeta) -> Map<String, Value> {
    let mut binding = Map::new();
    binding.insert("subject".to_owned(), uri_node(iri));
    if let Some(type_iri) = &meta.type_iri {
        binding.insert("type".to_owned(), uri_node(type_iri));
    }
    if let Some(display_id) = &meta.display_id {
        binding.insert("displayId".to_owned(), literal(display_id));
    }
    if let Some(version) = &meta.version {
        binding.insert("version".to_owned(), literal(version));
    }
    if let Some(name) = &meta.name {
        binding.insert("name".to_owned(), literal(name));
    }
    if let Some(description) = &meta.description {
        binding.insert("description".to_owned(), literal(description));
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

fn uri_node(value: &str) -> Value {
    json!({ "type": "uri", "value": value })
}

fn literal(value: &str) -> Value {
    json!({ "type": "literal", "value": value })
}
