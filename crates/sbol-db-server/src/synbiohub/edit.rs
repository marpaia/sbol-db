//! SynBioHub v1 field-edit and membership routes.
//!
//! These port classic SynBioHub's in-place mutations: the mutable text fields
//! (`/updateMutableDescription`, `/updateMutableNotes`, `/updateMutableSource`)
//! and citations (`/updateCitations`), which take the target `uri` in the body;
//! and the generic `<uri>/edit|add|remove/:field` plus `<uri>/addToCollection`
//! and `<uri>/removeMembership`, which take the target in the path. Every route
//! is identity-gated: an anonymous caller is `403`, and the facade rejects a
//! non-owner (or a non-admin editing a public object).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use sbol_db_app::{EditService, FieldValue};
use serde::Deserialize;

use super::CurrentUser;
use crate::{ApiError, AppState};

/// The instance base IRI classic SynBioHub mints objects under.
const BASE: &str = "http://synbiohub.org/";

const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";
const DCTERMS_DESCRIPTION: &str = "http://purl.org/dc/terms/description";
const SBOL2_ROLE: &str = "http://sbols.org/v2#role";
const SBOL2_TYPE: &str = "http://sbols.org/v2#type";
const PROV_WAS_DERIVED_FROM: &str = "http://www.w3.org/ns/prov#wasDerivedFrom";

/// Build the [`EditService`] from the shared facade handles.
fn service(state: &AppState) -> EditService {
    EditService::new(
        state.app.store.clone(),
        state.app.sparql_update.clone(),
        state.app.acl_service.clone(),
    )
}

/// The authenticated caller, or a `403` for an anonymous mutation attempt.
fn require_user(user: Option<sbol_db_core::User>) -> Result<sbol_db_core::User, ApiError> {
    user.ok_or_else(|| ApiError::Forbidden("authentication is required to edit".to_owned()))
}

/// The classic plain-text success body.
fn success() -> Response {
    (axum::http::StatusCode::OK, "Success").into_response()
}

// --- mutable fields + citations (uri in body) --------------------------------

/// A `{uri, value}` body, posted by the mutable-field and citation editors.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MutableForm {
    pub uri: Option<String>,
    pub value: Option<String>,
}

pub async fn update_mutable_description(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_body::<MutableForm>(&headers, &body)?;
    let uri = required(form.uri, "uri")?;
    service(&state)
        .update_mutable_description(&user.graph_uri, user.is_admin, &uri, &value_of(&form.value))
        .await?;
    Ok(success())
}

pub async fn update_mutable_notes(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_body::<MutableForm>(&headers, &body)?;
    let uri = required(form.uri, "uri")?;
    service(&state)
        .update_mutable_notes(&user.graph_uri, user.is_admin, &uri, &value_of(&form.value))
        .await?;
    Ok(success())
}

pub async fn update_mutable_source(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_body::<MutableForm>(&headers, &body)?;
    let uri = required(form.uri, "uri")?;
    service(&state)
        .update_mutable_source(&user.graph_uri, user.is_admin, &uri, &value_of(&form.value))
        .await?;
    Ok(success())
}

pub async fn update_citations(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_body::<MutableForm>(&headers, &body)?;
    let uri = required(form.uri, "uri")?;
    let citations = parse_citations(form.value.as_deref())?;
    service(&state)
        .update_citations(&user.graph_uri, user.is_admin, &uri, &citations)
        .await?;
    Ok(success())
}

// --- generic edit/add/remove of a field (uri in path) ------------------------

/// A `<uri>/edit|add|remove/:field` body: the value, an optional previous value
/// (edit only), and the arbitrary predicate for the `annotation` field.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FieldForm {
    pub object: Option<String>,
    pub previous: Option<String>,
    pub pred: Option<String>,
}

/// A user object path plus the trailing `:field` segment.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserFieldPath {
    pub user_id: String,
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
    pub field: String,
}

/// A public object path plus the trailing `:field` segment.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicFieldPath {
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
    pub field: String,
}

fn user_field_uri(p: &UserFieldPath) -> String {
    format!(
        "{BASE}user/{}/{}/{}/{}",
        p.user_id, p.collection_id, p.display_id, p.version
    )
}

fn public_field_uri(p: &PublicFieldPath) -> String {
    format!(
        "{BASE}public/{}/{}/{}",
        p.collection_id, p.display_id, p.version
    )
}

/// The three field-edit verbs, sharing predicate resolution and value
/// formatting.
enum Verb {
    Edit,
    Add,
    Remove,
}

pub async fn user_edit_field(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<UserFieldPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    run_field(
        state,
        user,
        user_field_uri(&p),
        &p.field,
        Verb::Edit,
        &headers,
        &body,
    )
    .await
}

pub async fn user_add_field(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<UserFieldPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    run_field(
        state,
        user,
        user_field_uri(&p),
        &p.field,
        Verb::Add,
        &headers,
        &body,
    )
    .await
}

pub async fn user_remove_field(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<UserFieldPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    run_field(
        state,
        user,
        user_field_uri(&p),
        &p.field,
        Verb::Remove,
        &headers,
        &body,
    )
    .await
}

pub async fn public_edit_field(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<PublicFieldPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    run_field(
        state,
        user,
        public_field_uri(&p),
        &p.field,
        Verb::Edit,
        &headers,
        &body,
    )
    .await
}

pub async fn public_add_field(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<PublicFieldPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    run_field(
        state,
        user,
        public_field_uri(&p),
        &p.field,
        Verb::Add,
        &headers,
        &body,
    )
    .await
}

pub async fn public_remove_field(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<PublicFieldPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    run_field(
        state,
        user,
        public_field_uri(&p),
        &p.field,
        Verb::Remove,
        &headers,
        &body,
    )
    .await
}

async fn run_field(
    state: AppState,
    user: Option<sbol_db_core::User>,
    uri: String,
    field: &str,
    verb: Verb,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_body::<FieldForm>(headers, body)?;

    let predicate = resolve_predicate(field, form.pred.as_deref())?;
    let object = form
        .object
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("object is required".to_owned()))?;
    let value = format_field_value(field, &object);

    let svc = service(&state);
    match verb {
        Verb::Edit => {
            // Title/description replace whatever value is present (previous is a
            // wildcard); other fields replace only a matching previous value.
            let previous = if field == "title" || field == "description" {
                None
            } else {
                form.previous
                    .filter(|s| !s.is_empty())
                    .map(|p| format_field_value(field, &p))
            };
            svc.edit_field(
                &user.graph_uri,
                user.is_admin,
                &uri,
                &predicate,
                &value,
                previous.as_ref(),
            )
            .await?;
        }
        Verb::Add => {
            svc.add_field(&user.graph_uri, user.is_admin, &uri, &predicate, &value)
                .await?;
        }
        Verb::Remove => {
            svc.remove_field(&user.graph_uri, user.is_admin, &uri, &predicate, &value)
                .await?;
        }
    }
    Ok(success())
}

/// Map a field name to its predicate IRI. `annotation` takes the arbitrary
/// predicate from the request body; an unknown field is `404`.
fn resolve_predicate(field: &str, pred: Option<&str>) -> Result<String, ApiError> {
    match field {
        "title" => Ok(DCTERMS_TITLE.to_owned()),
        "description" => Ok(DCTERMS_DESCRIPTION.to_owned()),
        "role" => Ok(SBOL2_ROLE.to_owned()),
        "type" => Ok(SBOL2_TYPE.to_owned()),
        "wasDerivedFrom" => Ok(PROV_WAS_DERIVED_FROM.to_owned()),
        "annotation" => pred
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| ApiError::BadRequest("annotation predicate is required".to_owned())),
        _ => Err(ApiError::NotFound(format!("unknown field: {field}"))),
    }
}

/// Format a field's object as an IRI or a literal, mirroring classic
/// `formatObject`: `role`/`type`/`wasDerivedFrom` and any `http(s)://` value are
/// IRIs, except `title`/`description`, which are always literals.
fn format_field_value(field: &str, object: &str) -> FieldValue {
    let is_iri = (field == "wasDerivedFrom"
        || field == "type"
        || field == "role"
        || object.starts_with("http://")
        || object.starts_with("https://"))
        && field != "title"
        && field != "description";
    if is_iri {
        FieldValue::Iri(object.to_owned())
    } else {
        FieldValue::Literal(object.to_owned())
    }
}

// --- membership (uri in path) ------------------------------------------------

/// A user or public object path.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserObjectPath {
    pub user_id: String,
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicObjectPath {
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
}

/// A `{member}` body naming the object to remove from a Collection.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MemberForm {
    pub member: Option<String>,
}

/// The `{collections}` body of an `addToCollection`: the Collections to add the
/// path object to. Classic keys this on `collections` (the object is the URL
/// path), so a request that omits it adds no membership.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AddToCollectionForm {
    pub collections: Option<String>,
}

pub async fn user_add_to_collection(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<UserObjectPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let object = format!(
        "{BASE}user/{}/{}/{}/{}",
        p.user_id, p.collection_id, p.display_id, p.version
    );
    add_to_collections(state, user, object, &headers, &body).await
}

pub async fn user_remove_membership(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<UserObjectPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let collection = format!(
        "{BASE}user/{}/{}/{}/{}",
        p.user_id, p.collection_id, p.display_id, p.version
    );
    membership(state, user, collection, false, &headers, &body).await
}

pub async fn public_add_to_collection(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<PublicObjectPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let object = format!(
        "{BASE}public/{}/{}/{}",
        p.collection_id, p.display_id, p.version
    );
    add_to_collections(state, user, object, &headers, &body).await
}

pub async fn public_remove_membership(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<PublicObjectPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let collection = format!(
        "{BASE}public/{}/{}/{}",
        p.collection_id, p.display_id, p.version
    );
    membership(state, user, collection, false, &headers, &body).await
}

/// `removeMembership`: drop `member` (a body field) from `collection` (the URL
/// path object). Mirrors classic `removeMembership`, which keys the member on
/// the `member` body field.
async fn membership(
    state: AppState,
    user: Option<sbol_db_core::User>,
    collection: String,
    add: bool,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
    debug_assert!(!add, "addToCollection is served by add_to_collections");
    let user = require_user(user)?;
    let form = parse_body::<MemberForm>(headers, body)?;
    let member = normalize_member(required(form.member, "member")?);
    service(&state)
        .remove_membership(&user.graph_uri, user.is_admin, &collection, &member)
        .await?;
    Ok(success())
}

/// `addToCollection`: add `object` (the URL path) as a `sbol:member` of every
/// Collection named in the `collections` body field, mirroring classic
/// `addToCollection` (the object is the path, the target Collections are the
/// body). A request that omits `collections` adds no membership.
async fn add_to_collections(
    state: AppState,
    user: Option<sbol_db_core::User>,
    object: String,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_body::<AddToCollectionForm>(headers, body)?;
    let svc = service(&state);
    if let Some(collections) = form.collections.filter(|s| !s.is_empty()) {
        for target in collections
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let target = normalize_member(target.to_owned());
            svc.add_to_collection(&user.graph_uri, user.is_admin, &target, &object)
                .await?;
        }
    }
    Ok(success())
}

/// Expand a member reference to an absolute URI, mirroring classic's rewrite of
/// a leading `/public/` or `/user/` path onto the database prefix.
fn normalize_member(member: String) -> String {
    if let Some(rest) = member.strip_prefix("/public/") {
        format!("{BASE}public/{rest}")
    } else if let Some(rest) = member.strip_prefix("/user/") {
        format!("{BASE}user/{rest}")
    } else {
        member
    }
}

// --- shared helpers ----------------------------------------------------------

/// The mutable-field value, defaulting an absent field to the empty string
/// (which clears the field, matching classic).
fn value_of(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

/// A required form field, or a `400` naming the missing field.
fn required(value: Option<String>, field: &str) -> Result<String, ApiError> {
    value
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest(format!("{field} is required")))
}

/// Split and validate a comma-separated citations field as PubMed ids
/// (digits only), mirroring classic's `^[0-9]+(,[0-9]*)*$` guard.
fn parse_citations(raw: Option<&str>) -> Result<Vec<String>, ApiError> {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ApiError::BadRequest(
                "citations must be comma-separated PubMed ids".to_owned(),
            ));
        }
        ids.push(part.to_owned());
    }
    Ok(ids)
}

/// Parse a request body as JSON when the `Content-Type` says so, else as
/// form-encoded, matching the auth and makePublic routes. An empty body yields
/// the type's default.
fn parse_body<T: for<'de> Deserialize<'de> + Default>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, ApiError> {
    if body.is_empty() {
        return Ok(T::default());
    }
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false);
    if is_json {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
    } else {
        serde_urlencoded::from_bytes(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid form body: {e}")))
    }
}
