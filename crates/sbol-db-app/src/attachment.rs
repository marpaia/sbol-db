//! Attachment top-levels over the content-addressed blob store.
//!
//! [`AttachmentService`] ports classic SynBioHub's attachment surface: it stores
//! an uploaded payload in the [`BlobStore`](sbol_db_storage::BlobStore), mints a
//! first-class `sbol:Attachment` top-level under the target's submission
//! namespace, and writes the canonical attachment vocabulary into the target's
//! own graph (`<target> sbol:attachment <att>`, the `<att>_collection`
//! membership, and the attachment's `sbol:hash`/`sbol:size`/`sbol:format`/
//! `sbol:source` plus the `sbh:ownedBy`/`sbh:topLevel` stamps). This is exactly
//! the triple set classic's `AttachUpload.sparql` produces.
//!
//! Writing is canonical-only; the reader ([`read_attachment`] and
//! [`get_attachments`](AttachmentService::get_attachments)) accepts both the
//! current `sbol:*` vocabulary and the legacy `sbh:attachment*` annotations a
//! migrated SBOL2 corpus carries, mirroring classic's union read.
//!
//! Every mutating verb is identity-gated through the [`AclService`]: a caller may
//! attach only to an object its own user graph owns, an administrator may attach
//! to anything, and a public-graph target requires an administrator. An anonymous
//! or non-owning caller is rejected with [`MutationError::NotAuthorized`].

use std::sync::Arc;

use sbol_db_core::{DomainError, ObjectTerm, SubjectTerm, Triple};
use sbol_db_sparql::{SparqlError, SparqlOptions, SparqlUpdateEngine};
use sbol_db_storage::{BlobStore, SbolStore};

use crate::acl::AclService;
use crate::mutation::MutationError;

/// `rdf:type`.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// The SBOL2 attachment vocabulary. SynBioHub stores its derived view as SBOL2,
/// and `AttachUpload.sparql` writes these exact predicates and class.
const SBOL2_ATTACHMENT: &str = "http://sbols.org/v2#attachment";
const SBOL2_ATTACHMENT_CLASS: &str = "http://sbols.org/v2#Attachment";
const SBOL2_SOURCE: &str = "http://sbols.org/v2#source";
const SBOL2_HASH: &str = "http://sbols.org/v2#hash";
const SBOL2_SIZE: &str = "http://sbols.org/v2#size";
const SBOL2_FORMAT: &str = "http://sbols.org/v2#format";
const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";
const SBOL2_DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
const SBOL2_PERSISTENT_IDENTITY: &str = "http://sbols.org/v2#persistentIdentity";
const SBOL2_VERSION: &str = "http://sbols.org/v2#version";

/// SynBioHub terms: the ownership and top-level stamps every attachment carries.
const SBH_OWNED_BY: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy";
const SBH_TOP_LEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";

/// The legacy attachment annotations a migrated SBOL2 corpus carries on the
/// attachment object, read alongside the canonical `sbol:*` vocabulary.
const SBH_ATTACHMENT: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachment";
const SBH_ATTACHMENT_HASH: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentHash";
const SBH_ATTACHMENT_SIZE: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentSize";
const SBH_ATTACHMENT_TYPE: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentType";

const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";

/// The format IRI classic records for an attachment of unknown type, the
/// `getTypeFromExtension` fallback and the default a `/attachURL` with no
/// declared type takes.
pub const UNKNOWN_ATTACHMENT_TYPE: &str =
    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#unknownAttachment";

/// The version every minted attachment takes, matching classic's hardcoded `1`.
const ATTACHMENT_VERSION: &str = "1";

/// A resolved attachment: its top-level URI plus the metadata read from either
/// the canonical `sbol:*` vocabulary or the legacy `sbh:attachment*` annotations.
/// `hash` is `None` for a URL attachment (no local blob); `format` is the
/// media-type IRI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentRef {
    /// The attachment top-level's URI.
    pub uri: String,
    /// The attachment's human-readable name (`dcterms:title`/`sbol:displayId`).
    pub name: Option<String>,
    /// The content address of the stored blob, absent for a URL attachment.
    pub hash: Option<String>,
    /// The uncompressed byte size of the blob, when recorded.
    pub size: Option<u64>,
    /// The media-type IRI describing the payload.
    pub format: Option<String>,
    /// Where the bytes are served from: the `<att>/download` link for a stored
    /// blob, or the external URL for a URL attachment.
    pub source: Option<String>,
}

/// The attachment attach/read verbs, gated on caller ownership.
#[derive(Clone)]
pub struct AttachmentService {
    store: Arc<dyn SbolStore>,
    sparql_update: Arc<SparqlUpdateEngine>,
    acl_service: AclService,
    blobs: Arc<dyn BlobStore>,
}

impl AttachmentService {
    /// Build the service over the store, the SPARQL Update engine, the ACL
    /// service, and the blob store.
    pub fn new(
        store: Arc<dyn SbolStore>,
        sparql_update: Arc<SparqlUpdateEngine>,
        acl_service: AclService,
        blobs: Arc<dyn BlobStore>,
    ) -> Self {
        Self {
            store,
            sparql_update,
            acl_service,
            blobs,
        }
    }

    /// Store `bytes` in the blob store and attach them to `target_uri` as a
    /// freshly minted `sbol:Attachment` top-level, writing the canonical
    /// attachment vocabulary into the target's own graph. `name` becomes the
    /// attachment's `dcterms:title`; `id` seeds its display id (falling back to a
    /// generated one). Mirrors classic `addAttachmentToTopLevel` over an upload.
    pub async fn attach(
        &self,
        user_graph: &str,
        is_admin: bool,
        target_uri: &str,
        name: &str,
        id: Option<&str>,
        bytes: &[u8],
    ) -> Result<AttachmentRef, MutationError> {
        let graph = self.authorize(user_graph, is_admin, target_uri).await?;
        let blob = self.blobs.put(bytes).await?;
        let mint = AttachmentMint::new(target_uri, id);
        let source = format!("{}/download", mint.attachment_uri);

        let update = attach_update(
            target_uri,
            &mint,
            user_graph,
            name,
            &source,
            &blob.mime,
            Some(&blob.sha1),
            Some(blob.size),
        );
        self.run(&update, &graph).await?;

        Ok(AttachmentRef {
            uri: mint.attachment_uri,
            name: Some(name.to_owned()),
            hash: Some(blob.sha1),
            size: Some(blob.size),
            format: Some(blob.mime),
            source: Some(source),
        })
    }

    /// Attach an external URL to `target_uri` as a `sbol:Attachment` whose
    /// `sbol:source` is the URL itself: no blob is stored, so no `sbol:hash` or
    /// `sbol:size` is written. `format_iri` records the declared media type,
    /// defaulting to [`UNKNOWN_ATTACHMENT_TYPE`]. Mirrors classic
    /// `AttachUrl.sparql`.
    #[allow(clippy::too_many_arguments)]
    pub async fn attach_url(
        &self,
        user_graph: &str,
        is_admin: bool,
        target_uri: &str,
        name: &str,
        id: Option<&str>,
        url: &str,
        format_iri: Option<&str>,
    ) -> Result<AttachmentRef, MutationError> {
        if !is_safe_iri(url) {
            return Err(MutationError::Domain(DomainError::InvalidInput(format!(
                "attachment URL is not a valid IRI: {url}"
            ))));
        }
        let graph = self.authorize(user_graph, is_admin, target_uri).await?;
        let mint = AttachmentMint::new(target_uri, id);
        let format_iri = format_iri.unwrap_or(UNKNOWN_ATTACHMENT_TYPE);

        let update = attach_update(
            target_uri, &mint, user_graph, name, url, format_iri, None, None,
        );
        self.run(&update, &graph).await?;

        Ok(AttachmentRef {
            uri: mint.attachment_uri,
            name: Some(name.to_owned()),
            hash: None,
            size: None,
            format: Some(format_iri.to_owned()),
            source: Some(url.to_owned()),
        })
    }

    /// The attachments of `parent_uri`, read from its own triples. Recognizes both
    /// the canonical `sbol:attachment` edge and the legacy `sbh:attachment`
    /// annotation, and resolves each attachment object through
    /// [`read_attachment`] (which reads either vocabulary).
    pub async fn get_attachments(
        &self,
        parent_uri: &str,
    ) -> Result<Vec<AttachmentRef>, DomainError> {
        let parent_triples = self.store.triples_for_subject(parent_uri).await?;
        let mut out = Vec::new();
        for uri in attachment_uris(&parent_triples) {
            let triples = self.store.triples_for_subject(&uri).await?;
            if let Some(attachment) = read_attachment(&triples, &uri) {
                out.push(attachment);
            }
        }
        Ok(out)
    }

    /// Resolve the graph holding `uri` and enforce the write gate: the caller must
    /// own the object (or be an administrator), a public-graph target requires an
    /// administrator, and an unknown target is [`MutationError::NotFound`].
    async fn authorize(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
    ) -> Result<String, MutationError> {
        let graph = self
            .acl_service
            .graph_of_subject(uri)
            .await?
            .ok_or_else(|| MutationError::NotFound(uri.to_owned()))?;
        if !self
            .acl_service
            .can_write(user_graph, is_admin, uri, &graph)
            .await?
        {
            return Err(MutationError::NotAuthorized(uri.to_owned()));
        }
        Ok(graph)
    }

    /// Execute one SPARQL Update scoped to `graph`, the `default-graph-uri` the
    /// `INSERT DATA` targets, matching Virtuoso semantics.
    async fn run(&self, update: &str, graph: &str) -> Result<(), SparqlError> {
        self.sparql_update
            .execute(update, Some(graph), &SparqlOptions::default())
            .await?;
        Ok(())
    }
}

/// The minted identity of an attachment: its display id and the version-qualified
/// and version-independent URIs, plus the collection it becomes a member of.
struct AttachmentMint {
    display_id: String,
    persistent_identity: String,
    attachment_uri: String,
    collection_uri: String,
}

impl AttachmentMint {
    /// Mint an attachment under `target_uri`'s submission namespace. The base is
    /// the collection namespace (`<prefix>{user/<u>|public}/<id>`), taken by
    /// dropping the target's display-id and version segments; the attachment lands
    /// at `<base>/<displayId>/1` and joins `<base>/<id>_collection/1`, exactly as
    /// classic's `addAttachmentToTopLevel` computes them.
    fn new(target_uri: &str, id: Option<&str>) -> Self {
        let base = submission_base(target_uri);
        let display_id = clean_display_id(id);
        let persistent_identity = format!("{base}/{display_id}");
        let attachment_uri = format!("{persistent_identity}/{ATTACHMENT_VERSION}");
        let collection_segment = base.rsplit('/').next().unwrap_or_default();
        let collection_uri = format!("{base}/{collection_segment}_collection/{ATTACHMENT_VERSION}");
        Self {
            display_id,
            persistent_identity,
            attachment_uri,
            collection_uri,
        }
    }
}

/// The submission (collection) namespace of an object URI: the URI with its
/// display-id and version path segments dropped, e.g.
/// `…/user/alice/coll/part/1` → `…/user/alice/coll`.
fn submission_base(uri: &str) -> String {
    let without_version = uri.rsplit_once('/').map(|(head, _)| head).unwrap_or(uri);
    without_version
        .rsplit_once('/')
        .map(|(head, _)| head)
        .unwrap_or(without_version)
        .to_owned()
}

/// The display id for a minted attachment. A supplied `id` is reduced to
/// `[A-Za-z0-9]`-with-underscores and prefixed with `_` if it would start with a
/// digit; an absent or empty id yields `attachment_<uuid>`. Mirrors classic's
/// `cleanId` handling.
fn clean_display_id(id: Option<&str>) -> String {
    let cleaned = id.map(|raw| {
        raw.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
    });
    match cleaned.filter(|s| !s.is_empty()) {
        Some(clean) if clean.starts_with(|c: char| c.is_ascii_digit()) => format!("_{clean}"),
        Some(clean) => clean,
        None => format!("attachment_{}", uuid::Uuid::new_v4().simple()),
    }
}

/// Build the `INSERT DATA` that writes the canonical attachment triple set: the
/// parent `sbol:attachment` edge, the `_collection` membership, and the
/// attachment top-level's own vocabulary. `hash`/`size` are written only for a
/// stored blob (a URL attachment omits them).
#[allow(clippy::too_many_arguments)]
fn attach_update(
    target_uri: &str,
    mint: &AttachmentMint,
    owned_by: &str,
    name: &str,
    source: &str,
    format_iri: &str,
    hash: Option<&str>,
    size: Option<u64>,
) -> String {
    let att = &mint.attachment_uri;
    let mut body = String::new();
    body.push_str(&format!("<{target_uri}> <{SBOL2_ATTACHMENT}> <{att}> .\n"));
    body.push_str(&format!(
        "<{}> <{SBOL2_MEMBER}> <{att}> .\n",
        mint.collection_uri
    ));
    body.push_str(&format!(
        "<{att}> <{RDF_TYPE}> <{SBOL2_ATTACHMENT_CLASS}> .\n"
    ));
    body.push_str(&format!(
        "<{att}> <{DCTERMS_TITLE}> \"{}\" .\n",
        escape_literal(name)
    ));
    body.push_str(&format!(
        "<{att}> <{SBOL2_DISPLAY_ID}> \"{}\" .\n",
        escape_literal(&mint.display_id)
    ));
    body.push_str(&format!(
        "<{att}> <{SBOL2_PERSISTENT_IDENTITY}> <{}> .\n",
        mint.persistent_identity
    ));
    body.push_str(&format!(
        "<{att}> <{SBOL2_VERSION}> \"{ATTACHMENT_VERSION}\" .\n"
    ));
    body.push_str(&format!("<{att}> <{SBH_OWNED_BY}> <{owned_by}> .\n"));
    body.push_str(&format!("<{att}> <{SBH_TOP_LEVEL}> <{att}> .\n"));
    body.push_str(&format!("<{att}> <{SBOL2_SOURCE}> <{source}> .\n"));
    body.push_str(&format!("<{att}> <{SBOL2_FORMAT}> <{format_iri}> .\n"));
    if let Some(hash) = hash {
        body.push_str(&format!(
            "<{att}> <{SBOL2_HASH}> \"{}\" .\n",
            escape_literal(hash)
        ));
    }
    if let Some(size) = size {
        body.push_str(&format!("<{att}> <{SBOL2_SIZE}> \"{size}\" .\n"));
    }
    format!("INSERT DATA {{ {body} }}")
}

/// The attachment URIs referenced in `triples`: the objects of every
/// `sbol:attachment` (canonical) and `sbh:attachment` (legacy) edge, de-duplicated
/// in first-seen order.
pub fn attachment_uris(triples: &[Triple]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for triple in triples {
        let predicate = triple.predicate.as_str();
        if predicate != SBOL2_ATTACHMENT && predicate != SBH_ATTACHMENT {
            continue;
        }
        if let ObjectTerm::Iri(iri) = &triple.object {
            let uri = iri.as_str();
            if !out.iter().any(|seen| seen == uri) {
                out.push(uri.to_owned());
            }
        }
    }
    out
}

/// Read the attachment identified by `uri` from a triple set, accepting both the
/// canonical `sbol:*` vocabulary and the legacy `sbh:attachment*` annotations
/// (canonical wins when both are present). Returns `None` when `uri` carries none
/// of the attachment predicates.
pub fn read_attachment(triples: &[Triple], uri: &str) -> Option<AttachmentRef> {
    let hash = literal_of(triples, uri, SBOL2_HASH)
        .or_else(|| literal_of(triples, uri, SBH_ATTACHMENT_HASH));
    let size = literal_of(triples, uri, SBOL2_SIZE)
        .or_else(|| literal_of(triples, uri, SBH_ATTACHMENT_SIZE))
        .and_then(|value| value.parse::<u64>().ok());
    let format =
        iri_of(triples, uri, SBOL2_FORMAT).or_else(|| iri_of(triples, uri, SBH_ATTACHMENT_TYPE));
    let source = iri_of(triples, uri, SBOL2_SOURCE);
    let name = literal_of(triples, uri, DCTERMS_TITLE)
        .or_else(|| literal_of(triples, uri, SBOL2_DISPLAY_ID));

    if hash.is_none() && format.is_none() && source.is_none() {
        return None;
    }
    Some(AttachmentRef {
        uri: uri.to_owned(),
        name,
        hash,
        size,
        format,
        source,
    })
}

/// The literal object of the first `(uri, predicate, ?o)` triple, if any.
fn literal_of(triples: &[Triple], uri: &str, predicate: &str) -> Option<String> {
    triples.iter().find_map(|t| {
        let SubjectTerm::Iri(subject) = &t.subject else {
            return None;
        };
        if subject.as_str() != uri || t.predicate.as_str() != predicate {
            return None;
        }
        match &t.object {
            ObjectTerm::Literal { value, .. } => Some(value.clone()),
            _ => None,
        }
    })
}

/// The IRI object of the first `(uri, predicate, ?o)` triple, if any.
fn iri_of(triples: &[Triple], uri: &str, predicate: &str) -> Option<String> {
    triples.iter().find_map(|t| {
        let SubjectTerm::Iri(subject) = &t.subject else {
            return None;
        };
        if subject.as_str() != uri || t.predicate.as_str() != predicate {
            return None;
        }
        match &t.object {
            ObjectTerm::Iri(iri) => Some(iri.as_str().to_owned()),
            _ => None,
        }
    })
}

/// Whether a string is safe to embed as a `<...>` IRI in a SPARQL update: it
/// carries none of the characters an IRI reference forbids, so it cannot break
/// out of the angle brackets.
fn is_safe_iri(value: &str) -> bool {
    !value.is_empty()
        && !value.chars().any(|c| {
            c.is_whitespace() || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '\\' | '^' | '`')
        })
}

/// Escape a string for use inside a double-quoted SPARQL literal.
fn escape_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use sbol_db_core::IriString;

    use super::*;

    const NS: &str = "http://synbiohub.org/user/alice/coll";

    fn iri_triple(subject: &str, predicate: &str, object: &str) -> Triple {
        Triple {
            graph_iri: None,
            subject: SubjectTerm::Iri(IriString::unchecked(subject)),
            predicate: IriString::unchecked(predicate),
            object: ObjectTerm::Iri(IriString::unchecked(object)),
        }
    }

    fn literal_triple(subject: &str, predicate: &str, value: &str) -> Triple {
        Triple {
            graph_iri: None,
            subject: SubjectTerm::Iri(IriString::unchecked(subject)),
            predicate: IriString::unchecked(predicate),
            object: ObjectTerm::Literal {
                value: value.to_owned(),
                datatype: IriString::unchecked("http://www.w3.org/2001/XMLSchema#string"),
                language: None,
            },
        }
    }

    #[test]
    fn mints_attachment_under_submission_namespace() {
        let target = format!("{NS}/part/1");
        let mint = AttachmentMint::new(&target, Some("my icon.png"));
        assert_eq!(mint.display_id, "my_icon_png");
        assert_eq!(mint.persistent_identity, format!("{NS}/my_icon_png"));
        assert_eq!(mint.attachment_uri, format!("{NS}/my_icon_png/1"));
        assert_eq!(
            mint.collection_uri,
            format!("{NS}/coll_collection/1"),
            "membership lands on the submission's own collection"
        );
    }

    #[test]
    fn clean_display_id_prefixes_leading_digit_and_generates_when_absent() {
        assert_eq!(clean_display_id(Some("123abc")), "_123abc");
        assert!(clean_display_id(None).starts_with("attachment_"));
        assert!(clean_display_id(Some("")).starts_with("attachment_"));
    }

    #[test]
    fn read_canonical_and_legacy_yield_the_same_hash() {
        let uri = format!("{NS}/att/1");

        let canonical = vec![
            iri_triple(&uri, RDF_TYPE, SBOL2_ATTACHMENT_CLASS),
            literal_triple(&uri, SBOL2_HASH, "abc123"),
            literal_triple(&uri, SBOL2_SIZE, "42"),
            iri_triple(
                &uri,
                SBOL2_FORMAT,
                "http://purl.org/NET/mediatypes/text/plain",
            ),
            iri_triple(&uri, SBOL2_SOURCE, &format!("{uri}/download")),
            literal_triple(&uri, DCTERMS_TITLE, "notes.txt"),
        ];
        let from_canonical = read_attachment(&canonical, &uri).expect("canonical attachment");

        let legacy = vec![
            literal_triple(&uri, SBH_ATTACHMENT_HASH, "abc123"),
            literal_triple(&uri, SBH_ATTACHMENT_SIZE, "42"),
            iri_triple(
                &uri,
                SBH_ATTACHMENT_TYPE,
                "http://wiki.synbiohub.org/wiki/Terms/synbiohub#imageAttachment",
            ),
        ];
        let from_legacy = read_attachment(&legacy, &uri).expect("legacy attachment");

        assert_eq!(from_canonical.hash.as_deref(), Some("abc123"));
        assert_eq!(from_legacy.hash.as_deref(), Some("abc123"));
        assert_eq!(from_canonical.size, Some(42));
        assert_eq!(from_legacy.size, Some(42));
    }

    #[test]
    fn attachment_uris_reads_both_edge_vocabularies() {
        let parent = format!("{NS}/part/1");
        let canonical_att = format!("{NS}/a/1");
        let legacy_att = format!("{NS}/b/1");
        let triples = vec![
            iri_triple(&parent, SBOL2_ATTACHMENT, &canonical_att),
            iri_triple(&parent, SBH_ATTACHMENT, &legacy_att),
        ];
        let uris = attachment_uris(&triples);
        assert!(uris.contains(&canonical_att));
        assert!(uris.contains(&legacy_att));
        assert_eq!(uris.len(), 2);
    }

    #[test]
    fn attach_update_writes_canonical_vocabulary() {
        let target = format!("{NS}/part/1");
        let mint = AttachmentMint::new(&target, Some("icon"));
        let update = attach_update(
            &target,
            &mint,
            "http://synbiohub.org/user/alice",
            "icon.png",
            &format!("{}/download", mint.attachment_uri),
            "http://purl.org/NET/mediatypes/image/png",
            Some("deadbeef"),
            Some(1024),
        );
        assert!(update.starts_with("INSERT DATA {"));
        assert!(update.contains(&format!(
            "<{target}> <{SBOL2_ATTACHMENT}> <{}>",
            mint.attachment_uri
        )));
        assert!(update.contains(&format!(
            "<{}> <{SBOL2_MEMBER}> <{}>",
            mint.collection_uri, mint.attachment_uri
        )));
        assert!(update.contains(&format!("<{RDF_TYPE}> <{SBOL2_ATTACHMENT_CLASS}>")));
        assert!(update.contains(&format!("<{SBOL2_HASH}> \"deadbeef\"")));
        assert!(update.contains(&format!("<{SBOL2_SIZE}> \"1024\"")));
        assert!(update.contains("/download>"));
    }

    #[test]
    fn attach_url_update_omits_hash_and_size() {
        let target = format!("{NS}/part/1");
        let mint = AttachmentMint::new(&target, Some("ext"));
        let update = attach_update(
            &target,
            &mint,
            "http://synbiohub.org/user/alice",
            "external",
            "https://example.org/data.txt",
            UNKNOWN_ATTACHMENT_TYPE,
            None,
            None,
        );
        assert!(update.contains("<https://example.org/data.txt>"));
        assert!(!update.contains(SBOL2_HASH));
        assert!(!update.contains(SBOL2_SIZE));
    }

    #[test]
    fn rejects_unsafe_attachment_urls() {
        assert!(is_safe_iri("https://example.org/a.txt"));
        assert!(!is_safe_iri("https://example.org/a> <b"));
        assert!(!is_safe_iri("has space"));
        assert!(!is_safe_iri(""));
    }
}
