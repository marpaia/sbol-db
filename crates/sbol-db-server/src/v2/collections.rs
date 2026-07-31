//! The V2 collections resource: submission as `POST /api/v2/collections`.
//!
//! Where the V1 adapter mints a submission from a `multipart/form-data`
//! `POST /submit`, the V2 surface takes a proper `POST` that creates a resource
//! and accepts either a JSON body (the idiomatic path, the SBOL document in a
//! `content` field) or the same `multipart/form-data` upload (so an existing
//! SBOL file rides through unchanged). Both mint through the same facade
//! [`SubmissionService`] the V1 route calls, into the authenticated caller's own
//! user namespace, and return `201 Created` with a `Location` to the minted
//! collection. An anonymous caller is `403`.

use axum::body::Bytes;
use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use sbol_db_app::SubmitRequest;
use sbol_db_core::{IriString, User};
use serde::Deserialize;
use serde_json::json;

use super::auth::{require_user, Identity};
use super::util::{
    encode_iri_segment, parse_citations, required, resolve_overwrite, resolve_submission_format,
};
use crate::error::ApiError;
use crate::v2::error::V2Error;
use crate::AppState;

/// The parsed submission fields, populated from a JSON body or a multipart
/// upload. `content` is the serialized SBOL document; `citations` is already
/// split into PubMed ids.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CreateForm {
    /// The submission id, the collection segment of every minted URI.
    id: Option<String>,
    /// The version segment of every minted URI.
    version: Option<String>,
    /// The collection title (`dcterms:title`).
    name: Option<String>,
    /// The collection description (`dcterms:description`).
    description: Option<String>,
    /// PubMed citation ids.
    citations: Vec<String>,
    /// The creator name (`dc:creator`); defaults to the caller's display name.
    creator_name: Option<String>,
    /// The SBOL serialization the `content` is expressed in (default RDF/XML).
    format: Option<String>,
    /// Collision policy (`fail`, `replace`, `merge`); defaults to `fail`.
    overwrite: Option<String>,
    /// The serialized SBOL document.
    content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MemberBody {
    member: Option<String>,
}

/// `POST /api/v2/collections` — mint a submission into the caller's user
/// namespace. Accepts a JSON body or a `multipart/form-data` upload.
pub async fn create_collection(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<Response, V2Error> {
    let user = require_contributor(&identity)?;
    let form = parse_create_form(request, &state).await?;
    let request = submit_request(form, &user)?;

    let outcome = state.app.submission_service().submit(request).await?;

    let members: Vec<&str> = outcome.members.iter().map(|m| m.as_str()).collect();
    let payload = json!({
        "collection_uri": outcome.collection_uri.as_str(),
        "persistent_identity": outcome.collection_persistent_identity.as_str(),
        "members": members,
        "graph": outcome.graph_iri,
        "triple_count": outcome.triple_count,
    });
    let location = format!(
        "/api/v2/objects/{}",
        encode_iri_segment(outcome.collection_uri.as_str())
    );
    Ok((StatusCode::CREATED, [(LOCATION, location)], Json(payload)).into_response())
}

/// `POST /api/v2/collections/validate` — run the exact parse, validation,
/// conversion, identity minting, and collision analysis used by commit without
/// writing a graph. This is the first step of the account contribution flow.
pub async fn validate_collection(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<Response, V2Error> {
    let user = require_contributor(&identity)?;
    let form = parse_create_form(request, &state).await?;
    let request = submit_request(form, &user)?;
    let preview = state.app.submission_service().preview(&request).await?;
    Ok(Json(preview).into_response())
}

/// Add an existing object to an owned collection. Both IRIs are validated
/// before reaching the SPARQL update service; the application layer enforces
/// ownership of the target collection.
pub async fn add_member(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(collection): Path<String>,
    body: Bytes,
) -> Result<StatusCode, V2Error> {
    let user = require_contributor(&identity)?;
    let form: MemberBody = super::util::parse_json(&body)?;
    let member = required(form.member, "member")?;
    validate_iri(&collection, "collection")?;
    validate_iri(&member, "member")?;
    state
        .app
        .edit_service()
        .add_to_collection(&user.graph_uri, user.is_admin, &collection, &member)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove one exact membership edge from an owned collection.
pub async fn remove_member(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((collection, member)): Path<(String, String)>,
) -> Result<StatusCode, V2Error> {
    let user = require_contributor(&identity)?;
    validate_iri(&collection, "collection")?;
    validate_iri(&member, "member")?;
    state
        .app
        .edit_service()
        .remove_membership(&user.graph_uri, user.is_admin, &collection, &member)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove an owned collection and the closure belonging to that submission.
/// This is deliberately separate from generic object deletion because a
/// collection removal has broader, explicit semantics.
pub async fn delete_collection(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(collection): Path<String>,
) -> Result<StatusCode, V2Error> {
    let user = require_contributor(&identity)?;
    validate_iri(&collection, "collection")?;
    state
        .app
        .mutation_service()
        .remove_collection(&user.graph_uri, user.is_admin, &collection)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_iri(value: &str, field: &str) -> Result<(), V2Error> {
    IriString::new(value.to_owned())
        .map(|_| ())
        .map_err(|error| {
            V2Error::from(ApiError::BadRequest(format!(
                "invalid {field} IRI: {error}"
            )))
        })
}

fn require_contributor(identity: &Identity) -> Result<User, V2Error> {
    let user = require_user(identity)?;
    if !user.is_member && !user.is_admin {
        return Err(V2Error::from(ApiError::Forbidden(
            "an active member account is required to contribute".to_owned(),
        )));
    }
    Ok(user)
}

async fn parse_create_form(request: Request, state: &AppState) -> Result<CreateForm, V2Error> {
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if content_type.starts_with("multipart/form-data") {
        let multipart = Multipart::from_request(request, state)
            .await
            .map_err(|error| {
                V2Error::from(ApiError::BadRequest(format!(
                    "invalid multipart body: {error}"
                )))
            })?;
        parse_multipart(multipart).await
    } else {
        let bytes = Bytes::from_request(request, state).await.map_err(|error| {
            V2Error::from(ApiError::BadRequest(format!(
                "invalid request body: {error}"
            )))
        })?;
        super::util::parse_json(&bytes)
    }
}

fn submit_request(form: CreateForm, user: &User) -> Result<SubmitRequest, V2Error> {
    Ok(SubmitRequest {
        owner: user.username.clone(),
        id: required(form.id, "id")?,
        version: required(form.version, "version")?,
        name: form.name.filter(|value| !value.is_empty()),
        description: form.description.filter(|value| !value.is_empty()),
        creator_name: form
            .creator_name
            .filter(|value| !value.is_empty())
            .or_else(|| Some(user.name.clone())),
        citations: form.citations,
        body: required(form.content, "content")?,
        format: resolve_submission_format(form.format.as_deref())?,
        overwrite: resolve_overwrite(form.overwrite.as_deref())?,
    })
}

/// Drain a multipart upload into a [`CreateForm`]. The SBOL document arrives in
/// the `file` part; `citations` is a comma-separated string. Unknown parts are
/// ignored, matching the V1 route's tolerance.
async fn parse_multipart(mut multipart: Multipart) -> Result<CreateForm, V2Error> {
    let mut form = CreateForm::default();
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        V2Error::from(ApiError::BadRequest(format!(
            "malformed multipart body: {e}"
        )))
    })? {
        let name = field.name().map(str::to_owned);
        match name.as_deref() {
            Some("file") | Some("content") => form.content = Some(field_text(field).await?),
            Some("id") => form.id = Some(field_text(field).await?),
            Some("version") => form.version = Some(field_text(field).await?),
            Some("name") => form.name = Some(field_text(field).await?),
            Some("description") => form.description = Some(field_text(field).await?),
            Some("creator_name") => form.creator_name = Some(field_text(field).await?),
            Some("format") => form.format = Some(field_text(field).await?),
            Some("overwrite") => form.overwrite = Some(field_text(field).await?),
            Some("citations") => form.citations = parse_citations(Some(&field_text(field).await?)),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    Ok(form)
}

/// Read a multipart field's bytes as UTF-8 text.
async fn field_text(field: axum::extract::multipart::Field<'_>) -> Result<String, V2Error> {
    field.text().await.map_err(|e| {
        V2Error::from(ApiError::BadRequest(format!(
            "invalid multipart field: {e}"
        )))
    })
}
