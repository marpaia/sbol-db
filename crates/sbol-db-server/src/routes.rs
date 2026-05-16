use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use sbol_db_core::{
    Direction, DocumentId, DocumentRecord, ImportReport, IriString, NeighborhoodQuery,
    NeighborhoodResult, ObjectId, SbolObjectRecord, SerializationFormat,
};
use sbol_db_postgres::{
    BatchSequenceMatch, ImportInput, ListObjectsFilter, OntologyLoadReport, OntologyRecord,
    OntologyTermRecord, SequenceMatch, SequenceSearchOptions,
};
use sbol_db_sparql::{ResultFormat, SparqlOptions};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::export;
use crate::AppState;

const READYZ_TIMEOUT: Duration = Duration::from_secs(1);

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match tokio::time::timeout(READYZ_TIMEOUT, state.service.ping()).await {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Ok(Err(err)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not_ready", "reason": err.to_string() })),
        ),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not_ready", "reason": "database probe timed out" })),
        ),
    }
}

#[derive(Deserialize)]
pub struct CreateDocumentParams {
    pub format: Option<String>,
    pub source_uri: Option<String>,
    pub document_iri: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub created_by: Option<String>,
}

pub async fn create_document(
    State(state): State<AppState>,
    Query(params): Query<CreateDocumentParams>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<ImportReport>, ApiError> {
    let format = resolve_format(params.format.as_deref(), &headers)?;
    let document_iri = params
        .document_iri
        .map(IriString::new)
        .transpose()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let report = state
        .service
        .import_document(ImportInput {
            body,
            format,
            source_uri: params.source_uri,
            document_iri,
            created_by: params.created_by,
            name: params.name,
            description: params.description,
        })
        .await?;
    Ok(Json(report))
}

pub async fn get_document(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DocumentRecord>, ApiError> {
    let doc = state
        .service
        .documents()
        .get(DocumentId(id))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("document {id}")))?;
    Ok(Json(doc))
}

#[derive(Deserialize)]
pub struct GetObjectParams {
    pub iri: String,
}

pub async fn get_object_by_iri(
    State(state): State<AppState>,
    Query(params): Query<GetObjectParams>,
) -> Result<Json<SbolObjectRecord>, ApiError> {
    let obj = state
        .service
        .objects()
        .get_by_iri(&params.iri)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("object {}", params.iri)))?;
    Ok(Json(obj))
}

/// Hard cap on the size of a `POST /objects/lookup` request, applied
/// before hitting Postgres. Matches the spirit of `max_rows` on SPARQL —
/// keep batch APIs bounded even when the body limit allows more.
const LOOKUP_MAX_IRIS: usize = 1000;

#[derive(Deserialize)]
pub struct LookupObjectsBody {
    pub iris: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct LookupObjectsResponse {
    pub found: Vec<SbolObjectRecord>,
    pub missing: Vec<String>,
}

pub async fn lookup_objects(
    State(state): State<AppState>,
    Json(body): Json<LookupObjectsBody>,
) -> Result<Json<LookupObjectsResponse>, ApiError> {
    if body.iris.is_empty() {
        return Ok(Json(LookupObjectsResponse {
            found: Vec::new(),
            missing: Vec::new(),
        }));
    }
    if body.iris.len() > LOOKUP_MAX_IRIS {
        return Err(ApiError::BadRequest(format!(
            "request exceeds maximum of {LOOKUP_MAX_IRIS} IRIs per call"
        )));
    }
    for iri in &body.iris {
        IriString::new(iri.clone()).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    }
    let refs: Vec<&str> = body.iris.iter().map(String::as_str).collect();
    let found = state.service.objects().get_by_iris(&refs).await?;
    let found_set: std::collections::HashSet<&str> = found.iter().map(|r| r.iri.as_str()).collect();
    let missing: Vec<String> = body
        .iris
        .iter()
        .filter(|iri| !found_set.contains(iri.as_str()))
        .cloned()
        .collect();
    Ok(Json(LookupObjectsResponse { found, missing }))
}

const LIST_MAX_LIMIT: u32 = 5000;
const LIST_DEFAULT_LIMIT: u32 = 1000;

#[derive(Deserialize)]
pub struct ListObjectsParams {
    #[serde(default)]
    pub sbol_class: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub document_id: Option<Uuid>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
}

fn default_list_limit() -> u32 {
    LIST_DEFAULT_LIMIT
}

#[derive(serde::Serialize)]
pub struct ListObjectsResponse {
    pub objects: Vec<SbolObjectRecord>,
    /// Cursor for the next page (last `iri` of the current page) when the
    /// page filled to `limit`; `None` when the listing has been exhausted.
    pub next_cursor: Option<String>,
}

pub async fn list_objects(
    State(state): State<AppState>,
    Query(params): Query<ListObjectsParams>,
) -> Result<Json<ListObjectsResponse>, ApiError> {
    let limit = params.limit.clamp(1, LIST_MAX_LIMIT);
    let filter = ListObjectsFilter {
        sbol_class: params.sbol_class,
        role: params.role,
        document_id: params.document_id.map(DocumentId),
        after_iri: params.after,
        limit,
    };
    let objects = state.service.objects().list(&filter).await?;
    let next_cursor = if objects.len() as u32 >= limit {
        objects.last().map(|o| o.iri.as_str().to_owned())
    } else {
        None
    };
    Ok(Json(ListObjectsResponse {
        objects,
        next_cursor,
    }))
}

#[derive(Deserialize)]
pub struct ExportObjectParams {
    pub format: Option<String>,
}

pub async fn export_object(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ExportObjectParams>,
) -> Result<impl IntoResponse, ApiError> {
    let iri = state
        .service
        .objects()
        .get_iri_by_id(ObjectId(id))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("object {id}")))?;
    let raw_format = params.format.as_deref().unwrap_or("turtle");
    let format = parse_export_format(raw_format)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown format: {raw_format}")))?;
    let body = export::export_subject_rdf(state.service.quads(), &iri, format).await?;
    let content_type = match format {
        SerializationFormat::Turtle => "text/turtle",
        SerializationFormat::JsonLd => "application/ld+json",
        SerializationFormat::NTriples => "application/n-triples",
        SerializationFormat::RdfXml => "application/rdf+xml",
        _ => "text/plain",
    };
    Ok(([(CONTENT_TYPE, content_type)], body))
}

#[derive(Deserialize)]
pub struct NeighborhoodParams {
    pub iri: String,
    #[serde(default = "default_depth")]
    pub depth: u32,
    #[serde(default = "default_direction")]
    pub direction: String,
    /// Comma-separated list of predicate IRIs. serde_urlencoded (axum's
    /// default Query parser) doesn't support repeated keys, so callers send
    /// `?predicates=p1,p2`.
    #[serde(default)]
    pub predicates: Option<String>,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: u32,
    #[serde(default)]
    pub literals: bool,
}

fn parse_predicates(value: Option<String>) -> Vec<IriString> {
    value
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(IriString::unchecked)
                .collect()
        })
        .unwrap_or_default()
}

fn default_depth() -> u32 {
    2
}
fn default_direction() -> String {
    "forward".to_owned()
}
fn default_max_nodes() -> u32 {
    2048
}

fn parse_direction(s: &str) -> Result<Direction, ApiError> {
    match s.to_ascii_lowercase().as_str() {
        "forward" | "out" => Ok(Direction::Forward),
        "backward" | "back" | "in" => Ok(Direction::Backward),
        "both" | "either" => Ok(Direction::Both),
        other => Err(ApiError::BadRequest(format!("unknown direction: {other}"))),
    }
}

fn build_neighborhood_query(p: NeighborhoodParams) -> Result<NeighborhoodQuery, ApiError> {
    let root = IriString::new(p.iri).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let direction = parse_direction(&p.direction)?;
    Ok(NeighborhoodQuery {
        root_iri: root,
        depth: p.depth,
        direction,
        predicate_allowlist: parse_predicates(p.predicates),
        max_nodes: Some(p.max_nodes),
        include_literals: p.literals,
    })
}

pub async fn neighborhood(
    State(state): State<AppState>,
    Query(params): Query<NeighborhoodParams>,
) -> Result<Json<NeighborhoodResult>, ApiError> {
    let query = build_neighborhood_query(params)?;
    let result = state.service.neighborhood().walk(&query).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct NeighborhoodRdfParams {
    pub iri: String,
    #[serde(default = "default_depth")]
    pub depth: u32,
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(default)]
    pub predicates: Option<String>,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: u32,
    #[serde(default = "default_literals_for_rdf")]
    pub literals: bool,
    #[serde(default = "default_rdf_format")]
    pub format: String,
}

fn default_rdf_format() -> String {
    "turtle".to_owned()
}
fn default_literals_for_rdf() -> bool {
    // RDF export usually wants literals so the dump is faithful.
    true
}

pub async fn neighborhood_rdf(
    State(state): State<AppState>,
    Query(params): Query<NeighborhoodRdfParams>,
) -> Result<impl IntoResponse, ApiError> {
    let format = parse_format(&params.format)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown format: {}", params.format)))?;
    let query = build_neighborhood_query(NeighborhoodParams {
        iri: params.iri,
        depth: params.depth,
        direction: params.direction,
        predicates: params.predicates,
        max_nodes: params.max_nodes,
        literals: params.literals,
    })?;
    let result = state.service.neighborhood().walk(&query).await?;
    let body = sbol_db_rdf::neighborhood_to_rdf(&result, format)?;
    let content_type = match format {
        SerializationFormat::Turtle => "text/turtle",
        SerializationFormat::JsonLd => "application/ld+json",
        SerializationFormat::NTriples => "application/n-triples",
        SerializationFormat::RdfXml => "application/rdf+xml",
        _ => "text/plain",
    };
    Ok(([(CONTENT_TYPE, content_type)], body))
}

#[derive(Deserialize, Default)]
pub struct SparqlGetParams {
    pub query: Option<String>,
    /// Override the format that would otherwise be derived from `Accept`.
    pub format: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct SparqlFormParams {
    pub query: Option<String>,
    pub format: Option<String>,
}

pub async fn sparql_get(
    State(state): State<AppState>,
    Query(params): Query<SparqlGetParams>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let query = params
        .query
        .ok_or_else(|| ApiError::BadRequest("missing ?query= parameter".to_owned()))?;
    let format = resolve_sparql_format(params.format.as_deref(), &headers)?;
    run_sparql(state, &query, format).await
}

pub async fn sparql_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or("").trim().to_owned())
        .unwrap_or_default();
    let (query_str, override_format): (String, Option<String>) = match content_type.as_str() {
        "application/sparql-query" | "" => {
            let q = std::str::from_utf8(&body)
                .map_err(|_| ApiError::BadRequest("query body is not UTF-8".to_owned()))?
                .to_owned();
            (q, None)
        }
        "application/x-www-form-urlencoded" => {
            let form: SparqlFormParams = serde_urlencoded::from_bytes(&body)
                .map_err(|e| ApiError::BadRequest(format!("invalid form body: {e}")))?;
            let q = form
                .query
                .ok_or_else(|| ApiError::BadRequest("missing query= field".to_owned()))?;
            (q, form.format)
        }
        other => {
            return Err(ApiError::BadRequest(format!(
                "unsupported content-type for /sparql: {other}"
            )));
        }
    };
    let format = resolve_sparql_format(override_format.as_deref(), &headers)?;
    run_sparql(state, &query_str, format).await
}

async fn run_sparql(
    state: AppState,
    query: &str,
    format: Option<ResultFormat>,
) -> Result<axum::response::Response, ApiError> {
    let options = SparqlOptions::default();
    let outcome = state.sparql.execute(query, format, &options).await?;
    let mut response =
        axum::response::Response::builder().header(CONTENT_TYPE, outcome.payload.content_type);
    if outcome.payload.truncated {
        response = response.header("X-SBOL-DB-Truncated", "true");
    }
    response
        .body(axum::body::Body::from(outcome.payload.body))
        .map_err(|e| ApiError::Domain(sbol_db_core::DomainError::Serialization(e.to_string())))
}

fn resolve_sparql_format(
    explicit: Option<&str>,
    headers: &HeaderMap,
) -> Result<Option<ResultFormat>, ApiError> {
    if let Some(s) = explicit {
        return s
            .parse::<ResultFormat>()
            .map(Some)
            .map_err(|e| ApiError::BadRequest(format!("{e}")));
    }
    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    for raw in accept.split(',') {
        let media = raw.split(';').next().unwrap_or("").trim();
        if media.is_empty() || media == "*/*" {
            continue;
        }
        if let Ok(f) = media.parse::<ResultFormat>() {
            return Ok(Some(f));
        }
    }
    // No explicit / no recognized Accept: let the engine pick the form's
    // natural default.
    Ok(None)
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct RevalidateBody {
    pub document_id: Option<Uuid>,
}

pub async fn revalidate_document(
    State(_state): State<AppState>,
    Json(_body): Json<RevalidateBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Phase 1 stub: revalidation requires the raw payload, which we currently
    // store in `sbol_documents.raw_payload` as a snapshot rather than as
    // re-parseable serialized form. Wire this up in Phase 2 when we round-trip
    // the JSON-LD payload.
    Err(ApiError::BadRequest(
        "revalidate is not yet implemented in Phase 1".to_owned(),
    ))
}

fn resolve_format(
    query_format: Option<&str>,
    headers: &HeaderMap,
) -> Result<SerializationFormat, ApiError> {
    if let Some(f) = query_format {
        return parse_format(f).ok_or_else(|| ApiError::BadRequest(format!("unknown format: {f}")));
    }
    let ct = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let format = match ct.split(';').next().unwrap_or("").trim() {
        "text/turtle" | "application/x-turtle" => SerializationFormat::Turtle,
        "application/ld+json" => SerializationFormat::JsonLd,
        "application/rdf+xml" | "text/xml" => SerializationFormat::RdfXml,
        "application/n-triples" => SerializationFormat::NTriples,
        _ => {
            return Err(ApiError::BadRequest(
                "specify ?format= or set Content-Type to a supported RDF media type".to_owned(),
            ))
        }
    };
    Ok(format)
}

fn parse_format(s: &str) -> Option<SerializationFormat> {
    match s.to_ascii_lowercase().as_str() {
        "turtle" | "ttl" => Some(SerializationFormat::Turtle),
        "jsonld" => Some(SerializationFormat::JsonLd),
        "rdfxml" | "rdf" | "xml" => Some(SerializationFormat::RdfXml),
        "ntriples" | "nt" => Some(SerializationFormat::NTriples),
        "nquads" | "nq" => Some(SerializationFormat::NQuads),
        "trig" => Some(SerializationFormat::TriG),
        "json" => Some(SerializationFormat::Json),
        _ => None,
    }
}

fn parse_export_format(s: &str) -> Option<SerializationFormat> {
    parse_format(s)
}

#[derive(Deserialize)]
pub struct SequenceSearchParams {
    pub pattern: String,
    #[serde(default = "default_sequence_max_hits")]
    pub max_hits: u32,
    #[serde(default)]
    pub forward_only: bool,
}

fn default_sequence_max_hits() -> u32 {
    1024
}

pub async fn sequence_search(
    State(state): State<AppState>,
    Query(params): Query<SequenceSearchParams>,
) -> Result<Json<Vec<SequenceMatch>>, ApiError> {
    let matches = state
        .service
        .sequence_search()
        .search(
            &params.pattern,
            SequenceSearchOptions {
                max_hits: Some(params.max_hits),
                forward_only: if params.forward_only {
                    Some(true)
                } else {
                    None
                },
            },
        )
        .await?;
    Ok(Json(matches))
}

const SEARCH_MAX_PATTERNS: usize = 256;

#[derive(Deserialize)]
pub struct SequenceSearchBody {
    pub patterns: Vec<String>,
    #[serde(default)]
    pub max_hits: Option<u32>,
    #[serde(default)]
    pub forward_only: Option<bool>,
}

pub async fn sequence_search_batch(
    State(state): State<AppState>,
    Json(body): Json<SequenceSearchBody>,
) -> Result<Json<Vec<BatchSequenceMatch>>, ApiError> {
    if body.patterns.is_empty() {
        return Ok(Json(Vec::new()));
    }
    if body.patterns.len() > SEARCH_MAX_PATTERNS {
        return Err(ApiError::BadRequest(format!(
            "request exceeds maximum of {SEARCH_MAX_PATTERNS} patterns per call"
        )));
    }
    let opts = SequenceSearchOptions {
        max_hits: Some(body.max_hits.unwrap_or(default_sequence_max_hits())),
        forward_only: match body.forward_only {
            Some(true) => Some(true),
            _ => None,
        },
    };
    let results = state
        .service
        .sequence_search()
        .search_many(&body.patterns, opts)
        .await?;
    Ok(Json(results))
}

#[derive(Deserialize)]
pub struct OntologyLoadParams {
    pub prefix: String,
    pub url: Option<String>,
    pub name: Option<String>,
}

pub async fn ontology_load(
    State(state): State<AppState>,
    Json(body): Json<OntologyLoadParams>,
) -> Result<Json<OntologyLoadReport>, ApiError> {
    let (url, name) = resolve_ontology_defaults(&body.prefix, body.url, body.name)
        .map_err(ApiError::BadRequest)?;
    let report = state
        .service
        .ontology()
        .load_from_url(&body.prefix.to_ascii_uppercase(), &name, &url)
        .await?;
    Ok(Json(report))
}

pub async fn ontology_list(
    State(state): State<AppState>,
) -> Result<Json<Vec<OntologyRecord>>, ApiError> {
    Ok(Json(state.service.ontology().list_ontologies().await?))
}

#[derive(Deserialize)]
pub struct OntologyTermQuery {
    pub iri: String,
}

pub async fn ontology_term(
    State(state): State<AppState>,
    Query(params): Query<OntologyTermQuery>,
) -> Result<Json<OntologyTermRecord>, ApiError> {
    let canonical = canonical_term_iri(&params.iri);
    let term = state
        .service
        .ontology()
        .get_term(&canonical)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("ontology term {}", params.iri)))?;
    Ok(Json(term))
}

#[derive(serde::Serialize)]
pub struct OntologyDescendant {
    pub iri: String,
    pub depth: i16,
}

pub async fn ontology_descendants(
    State(state): State<AppState>,
    Query(params): Query<OntologyTermQuery>,
) -> Result<Json<Vec<OntologyDescendant>>, ApiError> {
    let canonical = canonical_term_iri(&params.iri);
    let rows = state.service.ontology().descendants(&canonical).await?;
    let out = rows
        .into_iter()
        .map(|(iri, depth)| OntologyDescendant { iri, depth })
        .collect();
    Ok(Json(out))
}

fn canonical_term_iri(input: &str) -> String {
    if input.contains("://") {
        return input.to_owned();
    }
    if let Some((prefix, suffix)) = input.split_once(':') {
        let p = prefix.to_ascii_uppercase();
        return format!("http://purl.obolibrary.org/obo/{p}_{suffix}");
    }
    input.to_owned()
}

fn resolve_ontology_defaults(
    prefix: &str,
    url: Option<String>,
    name: Option<String>,
) -> Result<(String, String), String> {
    let prefix_lower = prefix.to_ascii_lowercase();
    match prefix_lower.as_str() {
        "so" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/so.obo".to_owned()),
            name.unwrap_or_else(|| "Sequence Ontology".to_owned()),
        )),
        "sbo" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/sbo.obo".to_owned()),
            name.unwrap_or_else(|| "Systems Biology Ontology".to_owned()),
        )),
        _ => {
            let url = url.ok_or_else(|| {
                format!("`url` is required for ontology prefix {prefix} (no default known)")
            })?;
            let name = name.unwrap_or_else(|| prefix.to_owned());
            Ok((url, name))
        }
    }
}
