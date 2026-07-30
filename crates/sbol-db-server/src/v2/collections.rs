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
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use sbol_db_app::SubmitRequest;
use serde::Deserialize;
use serde_json::json;

use super::auth::{require_user, Identity};
use super::util::{
    encode_iri_segment, parse_citations, required, resolve_overwrite, resolve_rdf_format,
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

/// `POST /api/v2/collections` — mint a submission into the caller's user
/// namespace. Accepts a JSON body or a `multipart/form-data` upload.
pub async fn create_collection(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<Response, V2Error> {
    let user = require_user(&identity)?;

    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    let form = if content_type.starts_with("multipart/form-data") {
        let multipart = Multipart::from_request(request, &state)
            .await
            .map_err(|e| {
                V2Error::from(ApiError::BadRequest(format!("invalid multipart body: {e}")))
            })?;
        parse_multipart(multipart).await?
    } else {
        let bytes = Bytes::from_request(request, &state).await.map_err(|e| {
            V2Error::from(ApiError::BadRequest(format!("invalid request body: {e}")))
        })?;
        super::util::parse_json(&bytes)?
    };

    let id = required(form.id, "id")?;
    let version = required(form.version, "version")?;
    let body = required(form.content, "content")?;
    let format = resolve_rdf_format(form.format.as_deref())?;
    let overwrite = resolve_overwrite(form.overwrite.as_deref())?;

    let request = SubmitRequest {
        owner: user.username.clone(),
        id,
        version,
        name: form.name.filter(|s| !s.is_empty()),
        description: form.description.filter(|s| !s.is_empty()),
        creator_name: form
            .creator_name
            .filter(|s| !s.is_empty())
            .or_else(|| Some(user.name.clone())),
        citations: form.citations,
        body,
        format,
        overwrite,
    };

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
