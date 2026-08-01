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
use axum::http::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH, LOCATION,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use sbol_db_app::SubmitRequest;
use sbol_db_core::{IriString, SerializationFormat, User};
use sbol_db_rdf::triples_to_rdf;
use sbol_db_storage::ConditionalContentWrite;
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

/// `GET /api/v2/collections/{iri}` — a compact synchronization descriptor.
/// The resource is hidden as `404` when it is absent or outside the caller's
/// public/owned/shared scope.
pub async fn get_collection(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(collection): Path<String>,
) -> Result<Response, V2Error> {
    validate_iri(&collection, "collection")?;
    let caller_graph = identity.0.as_ref().map(|user| user.graph_uri.as_str());
    let content = state
        .app
        .collection_sync_service()
        .read(caller_graph, &collection)
        .await?
        .ok_or_else(|| not_found(&collection))?;
    let content_url = format!(
        "/api/v2/collections/{}/content",
        encode_iri_segment(&collection)
    );
    let display_id = content.triples.iter().find_map(|triple| {
        if !matches!(&triple.subject, sbol_db_core::SubjectTerm::Iri(subject) if subject.as_str() == collection)
            || !matches!(
                triple.predicate.as_str(),
                "http://sbols.org/v2#displayId" | "http://sbols.org/v3#displayId"
            )
        {
            return None;
        }
        match &triple.object {
            sbol_db_core::ObjectTerm::Literal { value, .. } => Some(value.clone()),
            _ => None,
        }
    });
    let mut response = Json(json!({
        "iri": collection,
        "content_url": content_url,
        "content_etag": content.content_etag,
        "triple_count": content.triples.len(),
        "display_id": display_id,
    }))
    .into_response();
    attach_content_headers(&mut response, &content.content_etag)?;
    Ok(response)
}

/// `GET /api/v2/collections/{iri}/content` — the biological SBOL document,
/// excluding server-managed collaboration and audit metadata. Turtle is the
/// default; the four lossless RDF media types are accepted explicitly.
pub async fn get_collection_content(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(collection): Path<String>,
    headers: HeaderMap,
) -> Result<Response, V2Error> {
    validate_iri(&collection, "collection")?;
    let format = response_content_format(&headers)?;
    let caller_graph = identity.0.as_ref().map(|user| user.graph_uri.as_str());
    let content = state
        .app
        .collection_sync_service()
        .read(caller_graph, &collection)
        .await?
        .ok_or_else(|| not_found(&collection))?;
    let body = triples_to_rdf(&content.triples, format)?;
    let mut response = ([(CONTENT_TYPE, media_type(format))], body).into_response();
    attach_content_headers(&mut response, &content.content_etag)?;
    Ok(response)
}

/// `PUT /api/v2/collections/{iri}/content` — strict create-or-CAS replacement.
/// A create sends `If-None-Match: *`; an update sends the exact strong ETag
/// returned by GET in `If-Match`. There is no unconditional overwrite path.
pub async fn put_collection_content(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(collection): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, V2Error> {
    let user = require_contributor(&identity)?;
    validate_iri(&collection, "collection")?;
    let (expected_content_etag, creating) = request_precondition(&headers)?;
    let format = request_content_format(&headers)?;
    let body = std::str::from_utf8(&body).map_err(|error| {
        V2Error::from(ApiError::BadRequest(format!(
            "collection content must be UTF-8 RDF: {error}"
        )))
    })?;

    let outcome = state
        .app
        .collection_sync_service()
        .write(
            &user.graph_uri,
            user.is_admin,
            &collection,
            body,
            format,
            expected_content_etag.as_deref(),
        )
        .await?;
    match outcome {
        ConditionalContentWrite::Applied {
            triple_count,
            content_etag,
        } => {
            let payload = json!({
                "collection_uri": collection,
                "content_etag": content_etag,
                "triple_count": triple_count,
            });
            let status = if creating {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            let mut response = (status, Json(payload)).into_response();
            attach_content_headers(&mut response, &content_etag)?;
            Ok(response)
        }
        ConditionalContentWrite::PreconditionFailed {
            current_content_etag,
        } => {
            let mut response = V2Error::from(ApiError::PreconditionFailed(
                "the collection changed or the requested create identity already exists".to_owned(),
            ))
            .into_response();
            if let Some(etag) = current_content_etag {
                response.headers_mut().insert(
                    ETAG,
                    HeaderValue::from_str(&etag).map_err(|error| {
                        V2Error::from(ApiError::Domain(sbol_db_core::DomainError::Serialization(
                            error.to_string(),
                        )))
                    })?,
                );
            }
            Ok(response)
        }
    }
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

fn not_found(collection: &str) -> V2Error {
    ApiError::NotFound(format!(
        "collection {collection} was not found or is not visible"
    ))
    .into()
}

fn attach_content_headers(response: &mut Response, etag: &str) -> Result<(), V2Error> {
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(etag).map_err(|error| {
            V2Error::from(ApiError::Domain(sbol_db_core::DomainError::Serialization(
                error.to_string(),
            )))
        })?,
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    Ok(())
}

fn request_precondition(headers: &HeaderMap) -> Result<(Option<String>, bool), V2Error> {
    let if_match = headers.get(IF_MATCH).and_then(|value| value.to_str().ok());
    let if_none_match = headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok());
    match (if_match, if_none_match) {
        (Some(_), Some(_)) => Err(ApiError::BadRequest(
            "send either If-Match or If-None-Match, not both".to_owned(),
        )
        .into()),
        (Some(value), None) => {
            let value = value.trim();
            if value == "*"
                || value.starts_with("W/")
                || value.contains(',')
                || !value.starts_with('"')
                || !value.ends_with('"')
            {
                return Err(ApiError::BadRequest(
                    "If-Match must contain one exact strong collection content ETag".to_owned(),
                )
                .into());
            }
            Ok((Some(value.to_owned()), false))
        }
        (None, Some(value)) if value.trim() == "*" => Ok((None, true)),
        (None, Some(_)) => Err(ApiError::BadRequest(
            "collection creation requires If-None-Match: *".to_owned(),
        )
        .into()),
        (None, None) => Err(ApiError::PreconditionRequired(
            "send If-None-Match: * to create or the current ETag in If-Match to update".to_owned(),
        )
        .into()),
    }
}

fn request_content_format(headers: &HeaderMap) -> Result<SerializationFormat, V2Error> {
    let media = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    format_for_media_type(&media).ok_or_else(|| {
        ApiError::BadRequest(
            "Content-Type must be text/turtle, application/rdf+xml, application/ld+json, or application/n-triples"
                .to_owned(),
        )
        .into()
    })
}

fn response_content_format(headers: &HeaderMap) -> Result<SerializationFormat, V2Error> {
    let accept = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .trim();
    if accept.is_empty() {
        return Ok(SerializationFormat::Turtle);
    }
    let mut ranges = accept
        .split(',')
        .enumerate()
        .filter_map(|(index, raw)| {
            let mut parts = raw.split(';');
            let media = parts.next()?.trim().to_ascii_lowercase();
            let quality = parts
                .find_map(|part| {
                    part.trim()
                        .strip_prefix("q=")
                        .and_then(|value| value.parse::<f32>().ok())
                })
                .unwrap_or(1.0);
            Some((index, media, quality))
        })
        .collect::<Vec<_>>();
    ranges.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| left.0.cmp(&right.0))
    });
    for (_, media, quality) in ranges {
        if quality <= 0.0 {
            continue;
        }
        if matches!(media.as_str(), "*/*" | "application/*") {
            return Ok(SerializationFormat::Turtle);
        }
        if let Some(format) = format_for_media_type(&media) {
            return Ok(format);
        }
    }
    Err(
        ApiError::Sparql(sbol_db_sparql::SparqlError::UnsupportedFormat(
            accept.to_owned(),
        ))
        .into(),
    )
}

fn format_for_media_type(media: &str) -> Option<SerializationFormat> {
    match media {
        "text/turtle" | "application/x-turtle" => Some(SerializationFormat::Turtle),
        "application/rdf+xml" => Some(SerializationFormat::RdfXml),
        "application/ld+json" => Some(SerializationFormat::JsonLd),
        "application/n-triples" => Some(SerializationFormat::NTriples),
        _ => None,
    }
}

fn media_type(format: SerializationFormat) -> &'static str {
    match format {
        SerializationFormat::Turtle => "text/turtle",
        SerializationFormat::RdfXml => "application/rdf+xml",
        SerializationFormat::JsonLd => "application/ld+json",
        SerializationFormat::NTriples => "application/n-triples",
        _ => "application/octet-stream",
    }
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
