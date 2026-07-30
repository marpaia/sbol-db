//! SynBioHub v1 submission route.
//!
//! Classic SynBioHub's `POST /submit` takes a `multipart/form-data` body: the
//! submission `id` and `version`, the collection `name`/`description`, PubMed
//! `citations`, the `overwrite_merge` policy, and the SBOL `file`. This handler
//! mints the document into the authenticated caller's own user namespace through
//! the facade's [`SubmissionService`] and writes it to the caller's graph.
//!
//! Every mutation is identity-gated: an anonymous caller is rejected with `403`,
//! and the submission is minted under the caller's own username, so a caller can
//! never write into a namespace it does not own.

use axum::extract::{Multipart, State};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use sbol_db_app::SubmitRequest;
use sbol_db_core::{DomainError, SerializationFormat};
use sbol_db_storage::ImportOverwrite;

use super::CurrentUser;
use crate::{ApiError, AppState};

/// The parsed `multipart/form-data` fields of a `POST /submit`.
#[derive(Default)]
struct SubmitForm {
    id: Option<String>,
    version: Option<String>,
    name: Option<String>,
    description: Option<String>,
    citations: Option<String>,
    overwrite_merge: Option<String>,
    format: Option<String>,
    file: Option<String>,
    file_name: Option<String>,
    /// The existing collection URI to add the submission to, when the client
    /// submits into a collection rather than creating a new one (the UI sends
    /// `rootCollections`/`collections`). Its id/version replace `id`/`version`.
    root_collections: Option<String>,
}

/// `POST /submit`: mint a submission into the caller's user namespace.
pub async fn submit(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    let Some(user) = user else {
        return Err(ApiError::Forbidden(
            "authentication is required to submit".to_owned(),
        ));
    };

    let form = parse_form(multipart).await?;
    // Submitting into an existing collection: the client sends its URI as
    // `rootCollections`, and the submission merges into it. Otherwise `id` and
    // `version` name a new collection to mint.
    let root_collections = form.root_collections.filter(|s| !s.is_empty());
    let (id, version) = match &root_collections {
        Some(uri) => collection_id_version(uri)
            .ok_or_else(|| ApiError::BadRequest(format!("unrecognized collection URI: {uri}")))?,
        None => (required(form.id, "id")?, required(form.version, "version")?),
    };

    // The UI creates an empty collection by submitting metadata with no file
    // (classic accepts this). An absent or empty file mints just the root
    // collection, so default to an empty document.
    let (body, format) = match form.file.filter(|f| !f.is_empty()) {
        Some(file) => (
            file,
            resolve_format(form.format.as_deref(), form.file_name.as_deref())?,
        ),
        None => (String::new(), SerializationFormat::Turtle),
    };
    // Adding to an existing collection always merges into it; otherwise honor
    // the client's overwrite/merge policy for the new collection.
    let overwrite = if root_collections.is_some() {
        ImportOverwrite::Merge
    } else {
        resolve_overwrite_merge(form.overwrite_merge.as_deref())?
    };
    let citations = parse_citations(form.citations.as_deref());

    let request = SubmitRequest {
        owner: user.username.clone(),
        id,
        version,
        name: form.name.filter(|s| !s.is_empty()),
        description: form.description.filter(|s| !s.is_empty()),
        creator_name: Some(user.name.clone()),
        citations,
        body,
        format,
        overwrite,
    };

    state
        .app
        .submission_service()
        .submit(request)
        .await
        .map_err(submit_error)?;

    // Classic SynBioHub answers a successful V1 submission with a bare
    // `text/plain` acknowledgement; the pySBOL2 PartShop client asserts on this
    // exact string, so the adapter returns it verbatim rather than JSON.
    Ok((
        axum::http::StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        "Successfully uploaded",
    )
        .into_response())
}

/// Map a submission failure to an API error. A validation failure means the
/// uploaded SBOL did not pass validation, which is a client error (a bad
/// document), so it surfaces as `400` rather than the generic `500` a raw
/// [`DomainError::Validation`] maps to.
fn submit_error(err: DomainError) -> ApiError {
    match err {
        DomainError::Validation(message) => ApiError::BadRequest(message),
        other => ApiError::Domain(other),
    }
}

/// Drain the multipart body into a [`SubmitForm`]. Unknown fields are ignored,
/// matching classic's tolerance of extra form parts.
async fn parse_form(mut multipart: Multipart) -> Result<SubmitForm, ApiError> {
    let mut form = SubmitForm::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart body: {e}")))?
    {
        let name = field.name().map(str::to_owned);
        match name.as_deref() {
            Some("file") => {
                form.file_name = field.file_name().map(str::to_owned);
                form.file = Some(field_text(field).await?);
            }
            Some("id") => form.id = Some(field_text(field).await?),
            Some("version") => form.version = Some(field_text(field).await?),
            Some("name") => form.name = Some(field_text(field).await?),
            Some("description") => form.description = Some(field_text(field).await?),
            Some("citations") => form.citations = Some(field_text(field).await?),
            Some("overwrite_merge") => form.overwrite_merge = Some(field_text(field).await?),
            Some("rootCollections") | Some("collections") => {
                form.root_collections = Some(field_text(field).await?)
            }
            Some("format") => form.format = Some(field_text(field).await?),
            _ => {
                // Drain and discard any unrecognized part.
                let _ = field.bytes().await;
            }
        }
    }
    Ok(form)
}

/// Read a multipart field's bytes as UTF-8 text.
async fn field_text(field: axum::extract::multipart::Field<'_>) -> Result<String, ApiError> {
    field
        .text()
        .await
        .map_err(|e| ApiError::BadRequest(format!("invalid multipart field: {e}")))
}

/// A required form field, or a `400` naming the missing field.
fn required(value: Option<String>, field: &str) -> Result<String, ApiError> {
    value
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest(format!("{field} is required")))
}

/// Split the comma-separated `citations` field into trimmed, non-empty PubMed
/// IDs.
fn parse_citations(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

/// Resolve the submission's SBOL serialization from the explicit `format` field,
/// falling back to the uploaded file's extension, then to RDF/XML (classic's
/// default). Non-RDF formats are rejected: a submission mints from RDF triples.
/// Extract the submission `(id, version)` from an existing collection URI of the
/// form `…/<id>/<id>_collection/<version>`: the version is the last segment and
/// the id is the `<collectionId>_collection` segment with its `_collection`
/// suffix removed.
fn collection_id_version(uri: &str) -> Option<(String, String)> {
    let mut segments = uri.trim_end_matches('/').rsplit('/');
    let version = segments.next()?.to_owned();
    let id = segments.next()?.strip_suffix("_collection")?.to_owned();
    if id.is_empty() || version.is_empty() {
        return None;
    }
    Some((id, version))
}

fn resolve_format(
    format: Option<&str>,
    file_name: Option<&str>,
) -> Result<SerializationFormat, ApiError> {
    let hint = format
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| file_name.and_then(extension).map(str::to_owned));

    let resolved = match hint.as_deref() {
        None => SerializationFormat::RdfXml,
        Some(hint) => parse_rdf_format(hint).ok_or_else(|| {
            ApiError::BadRequest(format!("unsupported submission format: {hint}"))
        })?,
    };
    Ok(resolved)
}

/// The lowercased file extension of a filename, if any.
fn extension(file_name: &str) -> Option<&str> {
    file_name.rsplit_once('.').map(|(_, ext)| ext)
}

/// Map a format or extension hint to an RDF [`SerializationFormat`], rejecting
/// the non-RDF (sequence/JSON) formats a submission is never expressed in.
fn parse_rdf_format(hint: &str) -> Option<SerializationFormat> {
    match hint.to_ascii_lowercase().as_str() {
        "turtle" | "ttl" => Some(SerializationFormat::Turtle),
        "jsonld" => Some(SerializationFormat::JsonLd),
        "rdfxml" | "rdf" | "xml" => Some(SerializationFormat::RdfXml),
        "ntriples" | "nt" => Some(SerializationFormat::NTriples),
        _ => None,
    }
}

/// Map the classic `overwrite_merge` code to an [`ImportOverwrite`]: `0` fails
/// on a collision, `1` overwrites (delete then write), `2` merges additively,
/// and `3` (overwrite+merge) merges. An absent code defaults to `0`.
fn resolve_overwrite_merge(code: Option<&str>) -> Result<ImportOverwrite, ApiError> {
    let code = code.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("0");
    match code {
        "0" => Ok(ImportOverwrite::Fail),
        "1" => Ok(ImportOverwrite::Replace),
        "2" | "3" => Ok(ImportOverwrite::Merge),
        other => Err(ApiError::BadRequest(format!(
            "invalid overwrite_merge: {other}"
        ))),
    }
}
