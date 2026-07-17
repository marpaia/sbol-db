//! Closure download rendering for the V2 objects resource.
//!
//! A `GET /api/v2/objects/{iri}?format=…` crawls the object's ACL-scoped
//! transitive closure through the facade's [`Downloader`] and renders it with
//! the shared serializers, the same facade verb and serializers the V1 download
//! routes use. `?version=` selects SBOL2 or SBOL3 for the RDF-bearing formats;
//! the sequence formats are version-agnostic and ignore it. No client-supplied
//! graph is accepted; the caller's [`GraphScope`] is the read ceiling.

use std::sync::Arc;

use axum::body::Body;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::Response;
use sbol_db_app::Downloader;
use sbol_db_core::{DomainError, SerializationFormat, Triple};
use sbol_db_sparql::GraphScope;

use crate::error::ApiError;
use crate::serialize::{
    serialize_closure, serialize_gff3, serialize_omex, OmexAttachment, OmexAttachmentSource,
    Serialized,
};
use crate::v2::error::V2Error;
use crate::AppState;

/// A downloadable representation selected by `?format=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadFormat {
    /// The recursive RDF closure as RDF/XML (`sbol`).
    Sbol,
    /// The non-recursive RDF closure as RDF/XML (`sbolnr`).
    SbolNonRecursive,
    /// The recursive closure as GenBank (`gb`).
    GenBank,
    /// The recursive closure as FASTA (`fasta`).
    Fasta,
    /// The recursive closure as GFF3 (`gff`).
    Gff3,
    /// The recursive closure as an OMEX (COMBINE) archive (`omex`).
    Omex,
}

impl DownloadFormat {
    /// Parse the `format` query value, or `None` when it names no download
    /// format (the caller then falls back to `Accept` negotiation).
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "sbol" => Some(Self::Sbol),
            "sbolnr" => Some(Self::SbolNonRecursive),
            "gb" | "genbank" => Some(Self::GenBank),
            "fasta" => Some(Self::Fasta),
            "gff" | "gff3" => Some(Self::Gff3),
            "omex" => Some(Self::Omex),
            _ => None,
        }
    }

    /// The attachment filename extension for this format.
    fn extension(self) -> &'static str {
        match self {
            Self::Sbol | Self::SbolNonRecursive => "xml",
            Self::GenBank => "gb",
            Self::Fasta => "fasta",
            Self::Gff3 => "gff",
            Self::Omex => "omex",
        }
    }
}

/// Crawl the ACL-scoped closure of `uri` and render it as `format`, returning
/// the bytes as a download attachment named after the object's display id. An
/// empty closure (the object is absent or wholly out of scope) is a `404`.
pub async fn render_download(
    state: &AppState,
    uri: &str,
    display_id: &str,
    format: DownloadFormat,
    sbol2: bool,
    scope: GraphScope,
) -> Result<Response, V2Error> {
    let triples = fetch_closure(state, uri, format, scope).await?;
    if triples.is_empty() {
        return Err(V2Error::from(ApiError::NotFound(format!("object {uri}"))));
    }
    // OMEX bundles the closure's attachment blobs as archive members, so its
    // members are prefetched from the blob store before the synchronous
    // serialization; every other format ignores attachments.
    let attachments = match format {
        DownloadFormat::Omex => Some(PrefetchedAttachments(omex_members(state, &triples).await?)),
        _ => None,
    };
    let serialized = render(
        &triples,
        format,
        sbol2,
        attachments.as_ref().map(|a| a as &dyn OmexAttachmentSource),
    )?;
    attachment(serialized, display_id, format.extension())
}

/// Fetch the object's closure: `sbolnr` narrows to the non-recursive closure,
/// every other format takes the full recursive closure. Cross-instance
/// references resolve through the federation map, so a federated reference is
/// spliced in and a non-federated instance stays local.
async fn fetch_closure(
    state: &AppState,
    uri: &str,
    format: DownloadFormat,
    scope: GraphScope,
) -> Result<Vec<Triple>, V2Error> {
    let resolver = Arc::new(state.app.federation());
    let downloader = Downloader::new(state.app.sparql.clone()).with_remote_resolver(resolver);
    let triples = match format {
        DownloadFormat::SbolNonRecursive => downloader.fetch_non_recursive(uri, scope).await?,
        _ => downloader.fetch_recursive(uri, scope).await?,
    };
    Ok(triples)
}

/// Render a closure to the bytes and content type for `format`. GenBank, FASTA,
/// and GFF3 are version-agnostic, so the SBOL2 flag applies only to the
/// RDF-bearing formats.
fn render(
    triples: &[Triple],
    format: DownloadFormat,
    sbol2: bool,
    attachments: Option<&dyn OmexAttachmentSource>,
) -> Result<Serialized, DomainError> {
    match format {
        DownloadFormat::Sbol | DownloadFormat::SbolNonRecursive => {
            serialize_closure(triples, SerializationFormat::RdfXml, sbol2)
        }
        DownloadFormat::GenBank => serialize_closure(triples, SerializationFormat::GenBank, false),
        DownloadFormat::Fasta => serialize_closure(triples, SerializationFormat::Fasta, false),
        DownloadFormat::Gff3 => serialize_gff3(triples),
        DownloadFormat::Omex => serialize_omex(triples, sbol2, attachments),
    }
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
/// blob) or a missing blob is skipped rather than failing the archive. The
/// attachment reads go through the same facade helpers the V1 route uses.
async fn omex_members(
    state: &AppState,
    triples: &[Triple],
) -> Result<Vec<OmexAttachment>, V2Error> {
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

/// Wrap serialized bytes in a download response with the content type and a
/// `Content-Disposition` attachment filename.
fn attachment(
    serialized: Serialized,
    display_id: &str,
    extension: &str,
) -> Result<Response, V2Error> {
    let filename = format!("{}.{extension}", sanitize_filename(display_id));
    Response::builder()
        .header(CONTENT_TYPE, serialized.content_type)
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header("Access-Control-Expose-Headers", "Content-Disposition")
        .body(Body::from(serialized.bytes))
        .map_err(|e| V2Error::from(ApiError::Domain(DomainError::Serialization(e.to_string()))))
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
