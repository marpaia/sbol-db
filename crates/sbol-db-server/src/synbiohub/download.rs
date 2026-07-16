//! SynBioHub v1 download routes.
//!
//! Classic SynBioHub serves a top-level object in several exchange formats off
//! its object path: `/sbol` (the recursive RDF closure), `/sbolnr` (the
//! non-recursive closure), `/gb`, `/fasta`, `/gff`, `/omex`, and `/summary`
//! (the recursive closure as JSON). Each handler resolves the object URI from
//! the P2 path grammar, computes the caller's authorized
//! [`GraphScope`](sbol_db_sparql::GraphScope) from the `X-authorization`
//! identity, crawls the ACL-scoped closure through the shared
//! [`Downloader`](sbol_db_app::Downloader), and renders it with the P3
//! serializers. No client-supplied `FROM` is ever accepted; the scope is the
//! read ceiling.
//!
//! `?version=sbol2|sbol3` selects the SBOL version for the RDF-bearing formats
//! (`/sbol`, `/sbolnr`, `/omex`, `/summary`); GenBank, FASTA, and GFF3 are
//! version-agnostic sequence formats and ignore it.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::Response;
use axum::Extension;
use sbol_db_app::Downloader;
use sbol_db_core::{DomainError, SerializationFormat, Triple, User};
use sbol_db_sparql::GraphScope;
use serde::Deserialize;

use super::routes::{public_uri, scope_for, user_uri, PublicObject, UserObject};
use super::CurrentUser;
use crate::serialize::{
    serialize_closure, serialize_gff3, serialize_omex, OmexAttachment, OmexAttachmentSource,
    Serialized,
};
use crate::{ApiError, AppState};

/// The download format a route serves, selecting the closure kind (recursive vs
/// non-recursive), the serializer, and the attachment file extension.
#[derive(Clone, Copy)]
enum Format {
    /// The recursive RDF closure as RDF/XML.
    Sbol,
    /// The non-recursive RDF closure as RDF/XML.
    SbolNonRecursive,
    /// The recursive closure as GenBank.
    GenBank,
    /// The recursive closure as FASTA.
    Fasta,
    /// The recursive closure as GFF3.
    Gff3,
    /// The recursive closure as an OMEX (COMBINE) archive.
    Omex,
    /// The recursive closure as JSON-LD, classic's `/summary`.
    Summary,
}

impl Format {
    /// The attachment filename extension for this format.
    fn extension(self) -> &'static str {
        match self {
            Format::Sbol | Format::SbolNonRecursive => "xml",
            Format::GenBank => "gb",
            Format::Fasta => "fasta",
            Format::Gff3 => "gff",
            Format::Omex => "omex",
            Format::Summary => "json",
        }
    }
}

/// The `?version=` selector shared by the RDF-bearing download routes.
#[derive(Debug, Default, Deserialize)]
pub struct DownloadParams {
    pub version: Option<String>,
}

/// Resolve `version` to whether the caller asked for SBOL2 output. Absent or
/// `sbol3` keeps the native SBOL3 view; matches the native routes' parsing.
fn wants_sbol2(version: Option<&str>) -> Result<bool, ApiError> {
    match version.unwrap_or("sbol3") {
        "sbol3" | "3" => Ok(false),
        "sbol2" | "2" => Ok(true),
        other => Err(ApiError::BadRequest(format!("unknown version: {other}"))),
    }
}

/// Generate a pair of route handlers (public and user object paths) for one
/// download [`Format`], each resolving its object URI through the matching P2
/// path builder and delegating to [`download`].
macro_rules! download_routes {
    ($public:ident, $user:ident, $format:expr) => {
        pub async fn $public(
            State(state): State<AppState>,
            Extension(CurrentUser(user)): Extension<CurrentUser>,
            Path(object): Path<PublicObject>,
            Query(params): Query<DownloadParams>,
        ) -> Result<Response, ApiError> {
            let display_id = object.display_id.clone();
            download(
                state,
                user,
                public_uri(&object),
                display_id,
                $format,
                params.version,
            )
            .await
        }

        pub async fn $user(
            State(state): State<AppState>,
            Extension(CurrentUser(user)): Extension<CurrentUser>,
            Path(object): Path<UserObject>,
            Query(params): Query<DownloadParams>,
        ) -> Result<Response, ApiError> {
            let display_id = object.display_id.clone();
            download(
                state,
                user,
                user_uri(&object),
                display_id,
                $format,
                params.version,
            )
            .await
        }
    };
}

download_routes!(public_sbol, user_sbol, Format::Sbol);
download_routes!(public_sbolnr, user_sbolnr, Format::SbolNonRecursive);
download_routes!(public_genbank, user_genbank, Format::GenBank);
download_routes!(public_fasta, user_fasta, Format::Fasta);
download_routes!(public_gff, user_gff, Format::Gff3);
download_routes!(public_omex, user_omex, Format::Omex);
download_routes!(public_summary, user_summary, Format::Summary);

/// Crawl the ACL-scoped closure of `uri` and render it as `format`, returning
/// the bytes as a download attachment named after the object's display id.
async fn download(
    state: AppState,
    user: Option<User>,
    uri: String,
    display_id: String,
    format: Format,
    version: Option<String>,
) -> Result<Response, ApiError> {
    let sbol2 = wants_sbol2(version.as_deref())?;
    let scope = scope_for(&state, &user).await?;
    let triples = fetch_closure(&state, &uri, format, scope).await?;
    if triples.is_empty() {
        return Err(ApiError::NotFound(uri));
    }
    // OMEX bundles the closure's attachment blobs as archive members, so its
    // members are prefetched (async) from the blob store before the synchronous
    // serialization; every other format ignores attachments.
    let attachments = match format {
        Format::Omex => Some(PrefetchedAttachments(omex_members(&state, &triples).await?)),
        _ => None,
    };
    let serialized = render(
        &triples,
        format,
        sbol2,
        attachments.as_ref().map(|a| a as &dyn OmexAttachmentSource),
    )?;
    attachment(serialized, &display_id, format.extension())
}

/// A closure's attachment blobs, resolved from the blob store, wrapped as a
/// synchronous [`OmexAttachmentSource`] for [`serialize_omex`].
struct PrefetchedAttachments(Vec<OmexAttachment>);

impl OmexAttachmentSource for PrefetchedAttachments {
    fn attachments_for(&self, _triples: &[Triple]) -> Result<Vec<OmexAttachment>, DomainError> {
        Ok(self.0.clone())
    }
}

/// Resolve every attachment referenced in `triples` against the blob store,
/// returning one COMBINE member per stored blob. A URL attachment (no local
/// blob) or a missing blob is skipped rather than failing the archive.
async fn omex_members(
    state: &AppState,
    triples: &[Triple],
) -> Result<Vec<OmexAttachment>, ApiError> {
    let mut members = Vec::new();
    for uri in sbol_db_app::attachment_uris(triples) {
        let Some(attachment) = sbol_db_app::read_attachment(triples, &uri) else {
            continue;
        };
        let Some(hash) = attachment.hash else {
            continue;
        };
        let Some(bytes) = state.app.blobs.get(&hash).await? else {
            continue;
        };
        let filename = attachment
            .name
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or("attachment").to_owned());
        let format = attachment
            .format
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        members.push(OmexAttachment {
            filename,
            format,
            bytes,
        });
    }
    Ok(members)
}

/// Fetch the object's closure: `/sbolnr` narrows to the non-recursive closure,
/// every other format takes the full recursive closure.
async fn fetch_closure(
    state: &AppState,
    uri: &str,
    format: Format,
    scope: GraphScope,
) -> Result<Vec<Triple>, ApiError> {
    let downloader = Downloader::new(state.app.sparql.clone());
    let triples = match format {
        Format::SbolNonRecursive => downloader.fetch_non_recursive(uri, scope).await?,
        _ => downloader.fetch_recursive(uri, scope).await?,
    };
    Ok(triples)
}

/// Render a closure to the bytes and content type for `format`. GenBank, FASTA,
/// and GFF3 are version-agnostic, so the SBOL2 flag applies only to the
/// RDF-bearing formats. `attachments` supplies the OMEX archive's blob members
/// and is ignored by every other format.
fn render(
    triples: &[Triple],
    format: Format,
    sbol2: bool,
    attachments: Option<&dyn OmexAttachmentSource>,
) -> Result<Serialized, ApiError> {
    let serialized = match format {
        Format::Sbol | Format::SbolNonRecursive => {
            serialize_closure(triples, SerializationFormat::RdfXml, sbol2)?
        }
        Format::GenBank => serialize_closure(triples, SerializationFormat::GenBank, false)?,
        Format::Fasta => serialize_closure(triples, SerializationFormat::Fasta, false)?,
        Format::Gff3 => serialize_gff3(triples)?,
        Format::Omex => serialize_omex(triples, sbol2, attachments)?,
        Format::Summary => serialize_closure(triples, SerializationFormat::JsonLd, sbol2)?,
    };
    Ok(serialized)
}

/// Wrap serialized bytes in a download response, setting the content type and a
/// `Content-Disposition` attachment filename, matching classic's headers.
fn attachment(
    serialized: Serialized,
    display_id: &str,
    extension: &str,
) -> Result<Response, ApiError> {
    let filename = format!("{}.{extension}", sanitize_filename(display_id));
    Response::builder()
        .header(CONTENT_TYPE, serialized.content_type)
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header("Access-Control-Expose-Headers", "Content-Disposition")
        .body(Body::from(serialized.bytes))
        .map_err(|e| ApiError::Domain(DomainError::Serialization(e.to_string())))
}

/// Strip the characters that would break a `Content-Disposition` filename or
/// escape the download directory: quotes, backslashes, and path separators.
fn sanitize_filename(display_id: &str) -> String {
    display_id
        .chars()
        .map(|c| match c {
            '"' | '\\' | '/' => '_',
            other => other,
        })
        .collect()
}
