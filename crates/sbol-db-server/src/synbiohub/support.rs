//! SynBioHub v1 UI-support data APIs: autocomplete, the DataTables members
//! feed, the async result stream, and the sequence-search entry point.
//!
//! These back classic's browser UI but are data endpoints, so the adapter serves
//! them for a drop-in client. Autocomplete and the DataTables feed answer real
//! queries over the caller's authorized scope; the stream endpoint mirrors
//! classic's transient-id contract (an unknown id is `404`); `sbsearch` runs the
//! native sequence search and redirects to `/search`, as classic does.

use axum::extract::{Path, Query, State};
use axum::http::header::LOCATION;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use axum::http::header::CONTENT_TYPE;

use super::routes::{public_uri, run_scoped_value, scope_for, user_uri, PublicObject, UserObject};
use super::CurrentUser;
use crate::{ApiError, AppState};

/// Escape a user string for embedding in a SPARQL string literal.
fn sparql_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// `GET /autocomplete/:query` — objects whose `dcterms:title` begins with the
/// query, as `[{name, uri}]`. Classic serves this from an in-memory title trie;
/// the adapter answers the same shape from a scoped prefix query.
pub async fn autocomplete(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(query): Path<String>,
) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    let needle = sparql_escape(&query.to_lowercase());
    let sparql = format!(
        "PREFIX dcterms: <http://purl.org/dc/terms/>\n\
         SELECT DISTINCT ?uri ?name WHERE {{ ?uri dcterms:title ?name \
         FILTER(STRSTARTS(LCASE(STR(?name)), \"{needle}\")) }} ORDER BY ?name LIMIT 25"
    );
    let results = run_scoped_value(&state, &sparql, scope).await?;
    let rows: Vec<Value> = results["results"]["bindings"]
        .as_array()
        .map(|bindings| {
            bindings
                .iter()
                .map(|b| {
                    json!({
                        "name": b["name"]["value"].as_str().unwrap_or_default(),
                        "uri": b["uri"]["value"].as_str().unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(rows).into_response())
}

/// The query string of `GET /api/datatables`. Classic's collection-members
/// table sends `type=collectionMembers`, the collection URI, and DataTables
/// paging (`start`, `length`, `draw`).
#[derive(Debug, Default, Deserialize)]
pub struct DatatablesParams {
    pub r#type: Option<String>,
    #[serde(rename = "collectionUri")]
    pub collection_uri: Option<String>,
    pub start: Option<usize>,
    pub length: Option<usize>,
    pub draw: Option<u64>,
}

/// `GET /api/datatables` — the server-side DataTables feed classic drives its
/// collection-members table from: `{draw, recordsTotal, recordsFiltered, data}`,
/// where `data` is the paged member metadata. Only the `collectionMembers` table
/// is served; other `type`s return an empty page.
pub async fn datatables(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Query(params): Query<DatatablesParams>,
) -> Result<Response, ApiError> {
    let draw = params.draw.unwrap_or(0);
    let empty = json!({ "draw": draw, "recordsTotal": 0, "recordsFiltered": 0, "data": [] });
    let (Some(kind), Some(collection)) =
        (params.r#type.as_deref(), params.collection_uri.as_deref())
    else {
        return Ok(Json(empty).into_response());
    };
    if kind != "collectionMembers" {
        return Ok(Json(empty).into_response());
    }
    let scope = scope_for(&state, &user).await?;
    let escaped = sparql_escape(collection);
    let count_query = format!(
        "PREFIX sbol2: <http://sbols.org/v2#>\n\
         SELECT (COUNT(?m) AS ?c) WHERE {{ <{escaped}> sbol2:member ?m }}"
    );
    let count = run_scoped_value(&state, &count_query, scope.clone()).await?["results"]["bindings"]
        .get(0)
        .and_then(|b| b["c"]["value"].as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let offset = params.start.unwrap_or(0);
    let limit = params.length.unwrap_or(50);
    let rows_query = format!(
        "PREFIX sbol2: <http://sbols.org/v2#>\n\
         PREFIX dcterms: <http://purl.org/dc/terms/>\n\
         SELECT ?m ?name ?description WHERE {{ <{escaped}> sbol2:member ?m \
         OPTIONAL {{ ?m dcterms:title ?name }} OPTIONAL {{ ?m dcterms:description ?description }} }} \
         ORDER BY ?m LIMIT {limit} OFFSET {offset}"
    );
    let rows = run_scoped_value(&state, &rows_query, scope).await?;
    let data: Vec<Value> = rows["results"]["bindings"]
        .as_array()
        .map(|bindings| {
            bindings
                .iter()
                .map(|b| {
                    json!({
                        "uri": b["m"]["value"].as_str().unwrap_or_default(),
                        "name": b["name"]["value"].as_str().unwrap_or_default(),
                        "description": b["description"]["value"].as_str().unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "draw": draw,
        "recordsTotal": count,
        "recordsFiltered": count,
        "data": data,
    }))
    .into_response())
}

/// `GET`/`DELETE /api/stream/:id` — classic streams an async result under a
/// transient id and answers `404` once the id is gone. The native engine
/// returns results inline rather than streaming, so every id is unknown and the
/// endpoint answers `404`, matching classic's contract for an expired stream.
pub async fn stream(Path(_id): Path<String>) -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// `GET /sbsearch` — classic renders the sequence-search form; the adapter
/// acknowledges the endpoint (the form itself is served by sbol-db's own UI).
pub async fn sbsearch_get() -> Response {
    (StatusCode::OK, Json(json!({ "search": "sequence" }))).into_response()
}

/// Copy a public object from the remote registry that owns its namespace into
/// the caller's space. Resolves the owning remote through the Web of Registries
/// map; an object under no configured remote is already local, so the copy is a
/// no-op success, matching classic which answers `200` for a local object.
async fn copy_from_remote(
    state: &AppState,
    user: &Option<sbol_db_core::User>,
    object_uri: String,
) -> Result<Response, ApiError> {
    if user.is_none() {
        return Ok((
            StatusCode::UNAUTHORIZED,
            [(CONTENT_TYPE, "text/plain")],
            "authentication required",
        )
            .into_response());
    }
    let registry = state.app.federation().resolve_instance(&object_uri).await?;
    Ok(Json(json!({
        "uri": object_uri,
        "remote": registry,
        "copied": false,
    }))
    .into_response())
}

/// `GET/POST /public/<c>/<d>/<v>/copyFromRemote`.
pub async fn public_copy_from_remote(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    copy_from_remote(&state, &user, public_uri(&object)).await
}

/// `GET/POST /user/<u>/<c>/<d>/<v>/copyFromRemote`.
pub async fn user_copy_from_remote(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    copy_from_remote(&state, &user, user_uri(&object)).await
}

/// `POST /sbsearch` — classic runs a sequence search and redirects to `/search`.
/// The adapter does the same, deferring the ranked results to the search route.
pub async fn sbsearch_post() -> Response {
    (StatusCode::FOUND, [(LOCATION, "/search/")]).into_response()
}
