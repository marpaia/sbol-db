//! SynBioHub v1 attachment routes: attach, attachUrl, download.
//!
//! `POST <uri>/attach` stores an uploaded file in the blob store and mints a
//! `sbol:Attachment` top-level linked to the object; `POST <uri>/attachUrl`
//! records an external URL as an attachment with no local blob; `GET
//! <uri>/download` streams a stored blob back, gzip-encoded, with the media type
//! recorded on the attachment. The attach verbs are owner-gated through the
//! facade (`can_write`); the download read runs under the caller's authorized
//! [`GraphScope`](sbol_db_sparql::GraphScope), so a caller never streams a blob
//! attached to an object it cannot read.

use axum::body::{Body, Bytes};
use axum::extract::{Multipart, Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use sbol_db_app::{read_attachment, AttachmentService, Downloader};
use sbol_db_core::{DomainError, User};
use serde::Deserialize;

use super::routes::{public_uri, scope_for, user_uri, PublicObject, UserObject};
use super::CurrentUser;
use crate::{ApiError, AppState};

/// The media-type IRI prefix SynBioHub records attachment formats under; a format
/// under it maps back to its bare MIME type by stripping the prefix.
const MEDIATYPE_PREFIX: &str = "http://purl.org/NET/mediatypes/";

/// Build the [`AttachmentService`] from the shared facade handles.
fn service(state: &AppState) -> AttachmentService {
    state.app.attachment_service()
}

/// The authenticated caller, or a `403` for an anonymous attach attempt.
fn require_user(user: Option<User>) -> Result<User, ApiError> {
    user.ok_or_else(|| ApiError::Forbidden("authentication is required to attach".to_owned()))
}

/// The classic plain-text success body.
fn success() -> Response {
    (axum::http::StatusCode::OK, "Success").into_response()
}

// --- attach (multipart upload) -----------------------------------------------

pub async fn user_attach(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    attach(state, user, user_uri(&object), multipart).await
}

pub async fn public_attach(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    attach(state, user, public_uri(&object), multipart).await
}

/// Read the multipart file part and attach it to `target_uri`.
async fn attach(
    state: AppState,
    user: Option<User>,
    target_uri: String,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_attach_form(multipart).await?;
    let bytes = form
        .bytes
        .ok_or_else(|| ApiError::BadRequest("no file part in the upload".to_owned()))?;
    let name = form
        .file_name
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "attachment".to_owned());

    service(&state)
        .attach(
            &user.graph_uri,
            user.is_admin,
            &target_uri,
            &name,
            form.id.as_deref(),
            &bytes,
        )
        .await?;
    Ok(success())
}

/// The parsed fields of an attach upload: the optional display-id seed and the
/// file part's name and bytes.
#[derive(Default)]
struct AttachForm {
    id: Option<String>,
    file_name: Option<String>,
    bytes: Option<Vec<u8>>,
}

/// Drain the multipart body into an [`AttachForm`]. The first part carrying a
/// filename is the upload; an `id` field seeds the attachment display id. Any
/// other part is discarded, matching classic's tolerance of extra fields.
async fn parse_attach_form(mut multipart: Multipart) -> Result<AttachForm, ApiError> {
    let mut form = AttachForm::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart body: {e}")))?
    {
        let name = field.name().map(str::to_owned);
        let file_name = field.file_name().map(str::to_owned);
        if file_name.is_some() && form.bytes.is_none() {
            form.file_name = file_name;
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::BadRequest(format!("invalid file part: {e}")))?;
            form.bytes = Some(data.to_vec());
        } else if name.as_deref() == Some("id") {
            form.id = Some(
                field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("invalid id field: {e}")))?,
            );
        } else {
            let _ = field.bytes().await;
        }
    }
    Ok(form)
}

// --- attachUrl ----------------------------------------------------------------

/// The fields a `POST <uri>/attachUrl` carries: the external `url`, an optional
/// display `name`, the media `type` IRI, and an optional display-id `id`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AttachUrlForm {
    url: Option<String>,
    name: Option<String>,
    r#type: Option<String>,
    id: Option<String>,
}

pub async fn user_attach_url(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    attach_url(state, user, user_uri(&object), &headers, &body).await
}

pub async fn public_attach_url(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    attach_url(state, user, public_uri(&object), &headers, &body).await
}

/// Record an external URL as an attachment on `target_uri`.
async fn attach_url(
    state: AppState,
    user: Option<User>,
    target_uri: String,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_url_form(headers, body)?;
    let url = form
        .url
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("url is required".to_owned()))?;
    let name = form
        .name
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| url.rsplit('/').next().unwrap_or("attachment").to_owned());
    // Classic's attachUrl seeds the attachment display id from `name`
    // (`data.txt` -> `data_txt`); an explicit `id` field, when present, overrides.
    let id_seed = form
        .id
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&name);

    service(&state)
        .attach_url(
            &user.graph_uri,
            user.is_admin,
            &target_uri,
            &name,
            Some(id_seed),
            &url,
            form.r#type.as_deref().filter(|s| !s.is_empty()),
        )
        .await?;
    Ok(success())
}

/// Parse the attachUrl body as JSON when the `Content-Type` says so, else as
/// form-encoded, matching the other V1 mutation routes.
fn parse_url_form(headers: &HeaderMap, body: &[u8]) -> Result<AttachUrlForm, ApiError> {
    if body.is_empty() {
        return Ok(AttachUrlForm::default());
    }
    let is_json = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false);
    if is_json {
        serde_json::from_slice(body).map_err(|e| ApiError::BadRequest(format!("invalid JSON: {e}")))
    } else {
        serde_urlencoded::from_bytes(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid form body: {e}")))
    }
}

// --- download -----------------------------------------------------------------

pub async fn user_download(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    download(state, user, user_uri(&object)).await
}

pub async fn public_download(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    download(state, user, public_uri(&object)).await
}

/// Resolve the attachment at `uri` under the caller's scope, read its content
/// hash, and stream the stored blob back gzip-encoded. Mirrors classic's
/// `/download`: a missing attachment or blob is a `404`.
async fn download(state: AppState, user: Option<User>, uri: String) -> Result<Response, ApiError> {
    let scope = scope_for(&state, &user).await?;
    let downloader =
        Downloader::new(state.app.sparql.clone()).with_database_prefix(state.app.database_prefix());
    let triples = downloader.fetch_recursive(&uri, scope).await?;
    let attachment =
        read_attachment(&triples, &uri).ok_or_else(|| ApiError::NotFound(uri.clone()))?;
    let hash = attachment
        .hash
        .ok_or_else(|| ApiError::NotFound(format!("{uri} has no downloadable blob")))?;
    let gz = state
        .app
        .blobs
        .get_gz(&hash)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("blob {hash} is missing")))?;

    let mime = mime_for(attachment.format.as_deref());
    let filename = sanitize_filename(attachment.name.as_deref().unwrap_or("attachment"));
    Response::builder()
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_ENCODING, "gzip")
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header("Access-Control-Expose-Headers", "Content-Disposition")
        .body(Body::from(gz))
        .map_err(|e| ApiError::Domain(DomainError::Serialization(e.to_string())))
}

/// Map an attachment's format IRI to a response MIME type. A media-type IRI
/// collapses to its bare MIME by dropping the `mediatypes/` prefix; the legacy
/// SynBioHub image type maps to `image/png`; anything else is
/// `application/octet-stream`, classic's `attachmentTypeToMimeType` default.
fn mime_for(format: Option<&str>) -> String {
    match format {
        Some(iri) if iri.starts_with(MEDIATYPE_PREFIX) => iri[MEDIATYPE_PREFIX.len()..].to_owned(),
        Some("http://wiki.synbiohub.org/wiki/Terms/synbiohub#imageAttachment") => {
            "image/png".to_owned()
        }
        _ => "application/octet-stream".to_owned(),
    }
}

/// Strip the characters that would break a `Content-Disposition` filename or
/// escape the download directory.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '"' | '\\' | '/' => '_',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use flate2::read::GzDecoder;
    use sbol_db_app::{AppServices, Registration, SubmissionService, SubmitRequest};
    use sbol_db_backend::Backend;
    use sbol_db_core::SerializationFormat;
    use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
    use sbol_db_storage::ImportOverwrite;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::super::router;
    use crate::{Metrics, SchemaCache, ServerConfig};

    /// A compliant SBOL2 document minting a single ComponentDefinition `cd`.
    const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> ;
    dcterms:title "My Component" .
"#;

    const PAYLOAD: &[u8] = b"the quick brown fox jumps over the lazy dog\n";

    /// Register a user and mint them a token.
    async fn register(app: &AppServices, username: &str) -> String {
        let user = app
            .auth
            .register(Registration {
                username: username.to_owned(),
                name: username.to_owned(),
                email: format!("{username}@example.org"),
                affiliation: None,
                password: "s3cret".to_owned(),
                is_admin: false,
                is_curator: false,
                is_member: true,
            })
            .await
            .expect("register");
        app.auth.issue_token(user.id).await.expect("issue token")
    }

    /// Build a real [`AppState`] over a fresh SQLite backend with two accounts
    /// (owner `alice`, non-owner `bob`) and a submission owned by `alice`.
    async fn setup() -> (crate::AppState, String, String, TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("attach.db");
        let url = format!("sqlite://{}", path.display());
        let backend = Backend::open(&url).await.expect("open sqlite backend");
        backend
            .migrator
            .as_ref()
            .expect("sqlite backend has a migrator")
            .run_migrations()
            .await
            .expect("run migrations");

        let sparql = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
        let sparql_update = Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        ));
        let app = Arc::new(AppServices::from_backend(&backend));

        let alice_token = register(&app, "alice").await;
        let bob_token = register(&app, "bob").await;

        // Seed a submission owned by alice; its member `cd` is the attach target.
        SubmissionService::new(app.store.clone())
            .submit(SubmitRequest {
                owner: "alice".to_owned(),
                id: "mysub".to_owned(),
                version: "1".to_owned(),
                name: Some("My Submission".to_owned()),
                description: None,
                creator_name: Some("Alice".to_owned()),
                citations: Vec::new(),
                body: FIXTURE.to_owned(),
                format: SerializationFormat::Turtle,
                overwrite: ImportOverwrite::Fail,
            })
            .await
            .expect("seed submission");

        let state = crate::AppState {
            service: backend.store.clone(),
            sparql,
            sparql_update,
            app,
            metrics: Metrics::install(None, env!("CARGO_PKG_VERSION")),
            jobs: backend.jobs.clone(),
            lab: backend.lab.clone(),
            config: ServerConfig::default(),
            backend_kind: backend.kind,
            sql_console: backend.sql_console.clone(),
            db_stats: backend.db_stats.clone(),
            lsm_stats: backend.lsm_stats.clone(),
            schema_cache: Arc::new(SchemaCache::new()),
        };
        (state, alice_token, bob_token, dir)
    }

    /// A `multipart/form-data` body carrying an `id` field and a file part.
    fn multipart_body(boundary: &str, id: &str, filename: &str, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"id\"\r\n\r\n{id}\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(payload);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    }

    #[tokio::test]
    async fn owner_attaches_and_downloads_roundtrip() {
        let (state, alice_token, _bob_token, _dir) = setup().await;
        let app = router(state.clone()).with_state(state);

        let boundary = "sboldbtestboundary";
        let attach = Request::builder()
            .method("POST")
            .uri("/user/alice/mysub/cd/1/attach")
            .header("x-authorization", &alice_token)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart_body(
                boundary, "myicon", "icon.txt", PAYLOAD,
            )))
            .unwrap();
        let res = app.clone().oneshot(attach).await.expect("attach request");
        assert_eq!(res.status(), StatusCode::OK, "owner attach should succeed");

        let download = Request::builder()
            .method("GET")
            .uri("/user/alice/mysub/myicon/1/download")
            .header("x-authorization", &alice_token)
            .body(Body::empty())
            .unwrap();
        let res = app
            .clone()
            .oneshot(download)
            .await
            .expect("download request");
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get("content-encoding")
                .and_then(|v| v.to_str().ok()),
            Some("gzip"),
            "download must be gzip-encoded"
        );

        let gz = to_bytes(res.into_body(), 1 << 20).await.expect("body");
        let mut decoded = Vec::new();
        GzDecoder::new(&gz[..])
            .read_to_end(&mut decoded)
            .expect("gunzip");
        assert_eq!(
            decoded, PAYLOAD,
            "the downloaded blob must decompress to the uploaded bytes"
        );
    }

    #[tokio::test]
    async fn non_owner_attach_is_forbidden() {
        let (state, _alice_token, bob_token, _dir) = setup().await;
        let app = router(state.clone()).with_state(state);

        let boundary = "sboldbtestboundary";
        let attach = Request::builder()
            .method("POST")
            .uri("/user/alice/mysub/cd/1/attach")
            .header("x-authorization", &bob_token)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart_body(
                boundary, "sneaky", "evil.txt", PAYLOAD,
            )))
            .unwrap();
        let res = app.clone().oneshot(attach).await.expect("attach request");
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "a non-owner must not attach to another user's object"
        );
    }
}
