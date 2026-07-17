//! Submission minting: SynBioHub-compliant URI re-homing and denormalization.
//!
//! [`CollectionService`] takes a submitted SBOL document and produces the triple
//! set SynBioHub stores for it: a freshly minted root Collection, every
//! submitted top-level object re-homed under the target namespace, and the
//! denormalization triples SynBioHub's derived views rely on (`sbh:topLevel`
//! self-links, `sbol:member` edges from the collection to each object,
//! `sbh:ownedBy` ownership stamps, `dc:creator`, citations, and a shared
//! `sbol:persistentIdentity` per object so versions of one object collapse to a
//! single identity).
//!
//! The transform is pure: it parses and validates the document with `sbol-rs`,
//! rewrites its IRIs at the triple level (the facade owns the rewrite; it does
//! not depend on an `sbol-rs` re-mint API), and returns the rewritten triples.
//! Nothing is written to the store here; a submission verb hands the result to
//! the import path.

use chrono::Utc;
use sbol_db_core::{DomainError, IriString, ObjectTerm, SerializationFormat, SubjectTerm, Triple};
use sbol_db_rdf::rdf_graph_to_triples;

use crate::download::DEFAULT_DATABASE_PREFIX;

/// SynBioHub RDF vocabulary. The predicates and classes the minting transform
/// reads from a submission and stamps onto the result, spelled in full so no
/// prefix block is needed.
mod vocab {
    /// `rdf:type`.
    pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

    /// `sbh:topLevel`: the self-link every top-level subject carries so the
    /// derived views can find a child's owning top level.
    pub const SBH_TOP_LEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";
    /// `sbh:ownedBy`: names the user graph that owns a top-level object.
    pub const SBH_OWNED_BY: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy";

    /// `sbol:member`: the collection-to-object membership edge.
    pub const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";
    /// `sbol:persistentIdentity`: the version-independent identity shared by
    /// every version of an object.
    pub const SBOL2_PERSISTENT_IDENTITY: &str = "http://sbols.org/v2#persistentIdentity";
    /// `sbol:displayId`: the object's short, path-safe local name.
    pub const SBOL2_DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
    /// `sbol:version`: the object's version segment.
    pub const SBOL2_VERSION: &str = "http://sbols.org/v2#version";
    /// The SBOL2 `Collection` class, the type of the minted root collection.
    pub const SBOL2_COLLECTION: &str = "http://sbols.org/v2#Collection";

    /// SBOL3 namespace, recognized when reading a submitted document's
    /// `displayId` so an SBOL3 submission mints the same way.
    pub const SBOL3_DISPLAY_ID: &str = "http://sbols.org/v3#displayId";

    /// Dublin Core terms namespace (`dcterms:`).
    pub const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";
    pub const DCTERMS_DESCRIPTION: &str = "http://purl.org/dc/terms/description";
    pub const DCTERMS_CREATED: &str = "http://purl.org/dc/terms/created";
    pub const DCTERMS_MODIFIED: &str = "http://purl.org/dc/terms/modified";

    /// Dublin Core elements `dc:creator`: the submission's creator name.
    pub const DC_CREATOR: &str = "http://purl.org/dc/elements/1.1/creator";

    /// The OBO citation predicate SynBioHub stamps a collection's PubMed
    /// citations under.
    pub const OBI_CITATION: &str = "http://purl.obolibrary.org/obo/OBI_0001617";

    /// `xsd:string`, the datatype of a plain string literal.
    pub const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
    /// `xsd:dateTime`, the datatype of the created/modified timestamps.
    pub const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
}

/// The SBOL2 and PROV classes SynBioHub treats as top-level objects. A subject
/// typed as one of these is re-homed to its own minted URI and added to the
/// root collection; every other subject is a child re-homed by prefix under its
/// owning top level.
const TOP_LEVEL_CLASSES: &[&str] = &[
    "http://sbols.org/v2#Collection",
    "http://sbols.org/v2#ComponentDefinition",
    "http://sbols.org/v2#ModuleDefinition",
    "http://sbols.org/v2#Sequence",
    "http://sbols.org/v2#Model",
    "http://sbols.org/v2#Attachment",
    "http://sbols.org/v2#Implementation",
    "http://sbols.org/v2#CombinatorialDerivation",
    "http://sbols.org/v2#Experiment",
    "http://sbols.org/v2#ExperimentalData",
    "http://sbols.org/v2#GenericTopLevel",
    "http://www.w3.org/ns/prov#Activity",
    "http://www.w3.org/ns/prov#Agent",
    "http://www.w3.org/ns/prov#Plan",
];

/// The namespace a submission mints into: a user's private space or the shared
/// public space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MintScope {
    /// A user's private space: URIs live under `<prefix>user/<username>/<id>/`.
    User,
    /// The shared public space: URIs live under `<prefix>public/<id>/`, with no
    /// username segment.
    Public,
}

/// A document submitted for minting, with the collection-level metadata
/// SynBioHub stamps onto the root collection.
#[derive(Clone, Debug)]
pub struct Submission {
    /// The serialized SBOL document.
    pub body: String,
    /// The serialization the body is expressed in.
    pub format: SerializationFormat,
    /// The collection title (`dcterms:title`).
    pub name: Option<String>,
    /// The collection description (`dcterms:description`).
    pub description: Option<String>,
    /// The creator name (`dc:creator`).
    pub creator_name: Option<String>,
    /// PubMed citations, stamped as `obo:OBI_0001617` literals.
    pub citations: Vec<String>,
}

/// The result of minting a submission: the root collection's identity plus every
/// rewritten triple ready to hand to the import path.
#[derive(Clone, Debug)]
pub struct MintedSubmission {
    /// The minted, version-qualified root Collection URI.
    pub collection_uri: IriString,
    /// The root Collection's version-independent persistent identity.
    pub collection_persistent_identity: IriString,
    /// The minted, version-qualified URI of every submitted top-level object,
    /// each a `sbol:member` of the root collection.
    pub members: Vec<IriString>,
    /// The full rewritten triple set: re-homed document triples plus the root
    /// collection and every denormalization stamp. Untagged (`graph_iri` is
    /// `None`); the import path tags them with the target document graph.
    pub triples: Vec<Triple>,
}

/// Mints submissions into SynBioHub-compliant URIs and denormalization triples.
#[derive(Clone)]
pub struct CollectionService {
    database_prefix: String,
}

impl Default for CollectionService {
    fn default() -> Self {
        Self::new()
    }
}

impl CollectionService {
    /// Build a service minting under [`DEFAULT_DATABASE_PREFIX`].
    pub fn new() -> Self {
        Self {
            database_prefix: DEFAULT_DATABASE_PREFIX.to_owned(),
        }
    }

    /// Mint under a different database prefix. The instance base IRI is
    /// deployment-specific; a caller wires its configured prefix here.
    pub fn with_database_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.database_prefix = prefix.into();
        self
    }

    /// The user graph IRI for `owner`: `<prefix>user/<owner>`. This is the value
    /// stamped as `sbh:ownedBy` and the graph a private submission's ownership
    /// resolves to.
    pub fn user_graph_iri(&self, owner: &str) -> String {
        format!("{}user/{}", self.database_prefix, owner)
    }

    /// The namespace a scope mints its objects under.
    ///
    /// - [`MintScope::User`] → `<prefix>user/<owner>/<id>/`
    /// - [`MintScope::Public`] → `<prefix>public/<id>/`
    fn base_namespace(&self, owner: &str, id: &str, scope: MintScope) -> String {
        match scope {
            MintScope::User => format!("{}user/{}/{}/", self.database_prefix, owner, id),
            MintScope::Public => format!("{}public/{}/", self.database_prefix, id),
        }
    }

    /// Parse and validate a submitted SBOL document, mint its root Collection and
    /// per-object URIs under the target namespace, rewrite every IRI, and stamp
    /// the SynBioHub denormalization triples.
    ///
    /// The root Collection is minted at
    /// `<base><id>_collection/<version>` and each submitted top-level object at
    /// `<base><displayId>/<version>`, where `<base>` is the namespace `scope`
    /// selects. Child objects are re-homed by prefix under their owning top
    /// level, so intra-document references stay internally consistent.
    pub fn mint_uris(
        &self,
        submission: &Submission,
        owner: &str,
        id: &str,
        version: &str,
        scope: MintScope,
    ) -> Result<MintedSubmission, DomainError> {
        let rdf_format = rdf_format(submission.format)?;

        // Validate the document as SBOL. `Document::read` parses (and upgrades a
        // SBOL2 body to the SBOL3 model), rejecting anything that is not a
        // well-formed SBOL document; `validate` rejects a structurally invalid
        // one.
        let document = sbol::v3::Document::read(&submission.body, rdf_format)
            .map_err(|e| DomainError::Parse(e.to_string()))?;
        let report = document.validate();
        if report.has_errors() {
            return Err(DomainError::Validation(report.to_string()));
        }

        // Rewrite the document verbatim at the triple level, not through the
        // upgraded model, so the stored triples match what was submitted.
        let graph = sbol_rdf::Graph::parse(&submission.body, rdf_format)
            .map_err(|e| DomainError::Parse(e.to_string()))?;
        let placeholder = IriString::unchecked("");
        let mut source = rdf_graph_to_triples(&graph, &placeholder);
        for triple in &mut source {
            triple.graph_iri = None;
        }

        Ok(self.assemble(
            source,
            owner,
            id,
            version,
            scope,
            submission.name.as_deref(),
            submission.description.as_deref(),
            submission.creator_name.as_deref(),
            &submission.citations,
        ))
    }

    /// Re-mint an already-materialized triple set into SynBioHub-compliant URIs
    /// and denormalization triples, skipping the SBOL parse and validation step.
    ///
    /// This is the makePublic path: the input is the transitive closure of an
    /// already-minted, already-valid private object, so re-validating it as a
    /// fresh SBOL document is both redundant and brittle (the closure carries the
    /// `sbh:` denormalization triples). The re-home and stamping logic is
    /// identical to [`mint_uris`](Self::mint_uris); only the parse/validate front
    /// is dropped. The caller passes `MintScope::Public` and the original owner
    /// so the public URIs are `<prefix>public/<id>/…` while the `sbh:ownedBy`
    /// stamp still names the originating user graph.
    #[allow(clippy::too_many_arguments)]
    pub fn remint_triples(
        &self,
        source: Vec<Triple>,
        owner: &str,
        id: &str,
        version: &str,
        scope: MintScope,
        name: Option<&str>,
        description: Option<&str>,
        creator_name: Option<&str>,
        citations: &[String],
    ) -> MintedSubmission {
        let source: Vec<Triple> = source
            .into_iter()
            .map(|mut t| {
                t.graph_iri = None;
                t
            })
            .collect();
        self.assemble(
            source,
            owner,
            id,
            version,
            scope,
            name,
            description,
            creator_name,
            citations,
        )
    }

    /// Re-home a graphless source triple set onto the target namespace and stamp
    /// the SynBioHub denormalization triples, returning the minted collection
    /// identity and the full rewritten triple set. Shared by the submission mint
    /// (which parses and validates first) and the makePublic re-mint (which does
    /// not).
    #[allow(clippy::too_many_arguments)]
    fn assemble(
        &self,
        source: Vec<Triple>,
        owner: &str,
        id: &str,
        version: &str,
        scope: MintScope,
        name: Option<&str>,
        description: Option<&str>,
        creator_name: Option<&str>,
        citations: &[String],
    ) -> MintedSubmission {
        let base = self.base_namespace(owner, id, scope);
        let owned_by = self.user_graph_iri(owner);

        // One rewrite entry per submitted top level, keyed on its old persistent
        // identity so the top level and all of its children re-home together.
        let mut top_levels = discover_top_levels(&source, &base);
        top_levels.sort_by(|a, b| a.new_uri.cmp(&b.new_uri));
        let rename: Vec<(String, String)> = top_levels
            .iter()
            .map(|t| {
                (
                    t.old_persistent_identity.clone(),
                    t.new_persistent_identity.clone(),
                )
            })
            .collect();

        // The set of managed subjects (each minted top level) whose stamped
        // predicates the transform re-derives rather than carries over.
        let managed: std::collections::HashSet<String> =
            top_levels.iter().map(|t| t.new_uri.clone()).collect();

        let now = Utc::now().to_rfc3339();

        let mut out: Vec<Triple> = Vec::new();
        for triple in &source {
            let rewritten = rewrite_triple(triple, &rename);
            if is_managed_stamp(&rewritten, &managed) {
                // Dropped: re-derived below so the stamp is canonical and never
                // duplicated across a re-submission.
                continue;
            }
            out.push(rewritten);
        }

        // Stamp each top level: canonical identity, ownership, and self-link.
        for top in &top_levels {
            out.push(iri_triple(
                &top.new_uri,
                vocab::SBOL2_PERSISTENT_IDENTITY,
                &top.new_persistent_identity,
            ));
            out.push(literal_triple(
                &top.new_uri,
                vocab::SBOL2_VERSION,
                version,
                vocab::XSD_STRING,
            ));
            out.push(iri_triple(&top.new_uri, vocab::SBH_TOP_LEVEL, &top.new_uri));
            out.push(iri_triple(&top.new_uri, vocab::SBH_OWNED_BY, &owned_by));
            out.push(literal_triple(
                &top.new_uri,
                vocab::DCTERMS_CREATED,
                &now,
                vocab::XSD_DATETIME,
            ));
            out.push(literal_triple(
                &top.new_uri,
                vocab::DCTERMS_MODIFIED,
                &now,
                vocab::XSD_DATETIME,
            ));
        }

        // Mint the root Collection and its membership, ownership, and metadata.
        let collection_pi = format!("{base}{id}_collection");
        let collection_uri = format!("{collection_pi}/{version}");
        let members: Vec<IriString> = top_levels
            .iter()
            .map(|t| IriString::unchecked(&t.new_uri))
            .collect();

        out.push(iri_triple(
            &collection_uri,
            vocab::RDF_TYPE,
            vocab::SBOL2_COLLECTION,
        ));
        out.push(literal_triple(
            &collection_uri,
            vocab::SBOL2_DISPLAY_ID,
            &format!("{id}_collection"),
            vocab::XSD_STRING,
        ));
        out.push(iri_triple(
            &collection_uri,
            vocab::SBOL2_PERSISTENT_IDENTITY,
            &collection_pi,
        ));
        out.push(literal_triple(
            &collection_uri,
            vocab::SBOL2_VERSION,
            version,
            vocab::XSD_STRING,
        ));
        out.push(iri_triple(
            &collection_uri,
            vocab::SBH_TOP_LEVEL,
            &collection_uri,
        ));
        out.push(iri_triple(&collection_uri, vocab::SBH_OWNED_BY, &owned_by));
        out.push(literal_triple(
            &collection_uri,
            vocab::DCTERMS_CREATED,
            &now,
            vocab::XSD_DATETIME,
        ));
        out.push(literal_triple(
            &collection_uri,
            vocab::DCTERMS_MODIFIED,
            &now,
            vocab::XSD_DATETIME,
        ));
        if let Some(name) = name {
            out.push(literal_triple(
                &collection_uri,
                vocab::DCTERMS_TITLE,
                name,
                vocab::XSD_STRING,
            ));
        }
        if let Some(description) = description {
            out.push(literal_triple(
                &collection_uri,
                vocab::DCTERMS_DESCRIPTION,
                description,
                vocab::XSD_STRING,
            ));
        }
        if let Some(creator) = creator_name {
            out.push(literal_triple(
                &collection_uri,
                vocab::DC_CREATOR,
                creator,
                vocab::XSD_STRING,
            ));
        }
        for citation in citations {
            out.push(literal_triple(
                &collection_uri,
                vocab::OBI_CITATION,
                citation,
                vocab::XSD_STRING,
            ));
        }
        for member in &members {
            out.push(iri_triple(
                &collection_uri,
                vocab::SBOL2_MEMBER,
                member.as_str(),
            ));
        }

        MintedSubmission {
            collection_uri: IriString::unchecked(collection_uri),
            collection_persistent_identity: IriString::unchecked(collection_pi),
            members,
            triples: out,
        }
    }
}

/// Find every submitted top-level object and compute its minted identity. Each
/// top level re-homes to `<base><displayId>`; its version-qualified URI follows
/// by re-homing the submitted URI onto that persistent identity.
fn discover_top_levels(source: &[Triple], base: &str) -> Vec<TopLevelMint> {
    let mut mints: Vec<TopLevelMint> = Vec::new();
    for triple in source {
        if triple.predicate.as_str() != vocab::RDF_TYPE {
            continue;
        }
        let ObjectTerm::Iri(class) = &triple.object else {
            continue;
        };
        if !TOP_LEVEL_CLASSES.contains(&class.as_str()) {
            continue;
        }
        let SubjectTerm::Iri(subject) = &triple.subject else {
            continue;
        };
        let old_uri = subject.as_str();
        if mints.iter().any(|m| m.old_uri == old_uri) {
            continue;
        }

        let old_pi = old_persistent_identity(old_uri, source);
        let display = display_id(old_uri, &old_pi, source);
        let new_pi = format!("{base}{display}");
        let new_uri = rewrite_iri(old_uri, &[(old_pi.clone(), new_pi.clone())]);

        mints.push(TopLevelMint {
            old_uri: old_uri.to_owned(),
            old_persistent_identity: old_pi,
            new_persistent_identity: new_pi,
            new_uri,
        });
    }
    mints
}

/// One submitted top-level object's rewrite: its old identity and the minted
/// target identity it and its children re-home onto.
struct TopLevelMint {
    old_uri: String,
    old_persistent_identity: String,
    new_persistent_identity: String,
    new_uri: String,
}

/// The version-independent identity of `subject`: its `sbol:persistentIdentity`
/// if present, otherwise the URI with a trailing version segment stripped when
/// one is declared, otherwise the URI itself.
fn old_persistent_identity(subject: &str, source: &[Triple]) -> String {
    if let Some(pi) = object_iri(subject, vocab::SBOL2_PERSISTENT_IDENTITY, source) {
        return pi;
    }
    if let Some(version) = literal_value(subject, vocab::SBOL2_VERSION, source) {
        let suffix = format!("/{version}");
        if let Some(stripped) = subject.strip_suffix(&suffix) {
            return stripped.to_owned();
        }
    }
    subject.to_owned()
}

/// The display id of `subject`: its `sbol:displayId` (SBOL2 or SBOL3) if
/// present, otherwise the last path segment of its persistent identity.
fn display_id(subject: &str, persistent_identity: &str, source: &[Triple]) -> String {
    if let Some(display) = literal_value(subject, vocab::SBOL2_DISPLAY_ID, source) {
        return display;
    }
    if let Some(display) = literal_value(subject, vocab::SBOL3_DISPLAY_ID, source) {
        return display;
    }
    persistent_identity
        .rsplit('/')
        .next()
        .unwrap_or(persistent_identity)
        .to_owned()
}

/// The IRI object of the first `(subject, predicate, ?o)` triple, if any.
fn object_iri(subject: &str, predicate: &str, source: &[Triple]) -> Option<String> {
    source.iter().find_map(|t| {
        let SubjectTerm::Iri(s) = &t.subject else {
            return None;
        };
        if s.as_str() != subject || t.predicate.as_str() != predicate {
            return None;
        }
        match &t.object {
            ObjectTerm::Iri(iri) => Some(iri.as_str().to_owned()),
            _ => None,
        }
    })
}

/// The literal value of the first `(subject, predicate, ?o)` triple, if any.
fn literal_value(subject: &str, predicate: &str, source: &[Triple]) -> Option<String> {
    source.iter().find_map(|t| {
        let SubjectTerm::Iri(s) = &t.subject else {
            return None;
        };
        if s.as_str() != subject || t.predicate.as_str() != predicate {
            return None;
        }
        match &t.object {
            ObjectTerm::Literal { value, .. } => Some(value.clone()),
            _ => None,
        }
    })
}

/// Rewrite both IRI positions of a triple by longest-prefix identity match.
fn rewrite_triple(triple: &Triple, rename: &[(String, String)]) -> Triple {
    let subject = match &triple.subject {
        SubjectTerm::Iri(iri) => {
            SubjectTerm::Iri(IriString::unchecked(rewrite_iri(iri.as_str(), rename)))
        }
        other => other.clone(),
    };
    let object = match &triple.object {
        ObjectTerm::Iri(iri) => {
            ObjectTerm::Iri(IriString::unchecked(rewrite_iri(iri.as_str(), rename)))
        }
        other => other.clone(),
    };
    Triple {
        graph_iri: None,
        subject,
        predicate: triple.predicate.clone(),
        object,
    }
}

/// Re-home an IRI onto its minted namespace: find the longest old persistent
/// identity that is `uri` or a path-prefix of it, and swap in the new one. An
/// IRI matching no top level (an external or vocabulary URI) is left untouched.
fn rewrite_iri(uri: &str, rename: &[(String, String)]) -> String {
    let mut best: Option<&(String, String)> = None;
    for entry in rename {
        let old = &entry.0;
        let matches = uri == old
            || uri
                .strip_prefix(old)
                .is_some_and(|rest| rest.starts_with('/'));
        if matches && best.is_none_or(|b| old.len() > b.0.len()) {
            best = Some(entry);
        }
    }
    match best {
        Some((old, new)) => format!("{new}{}", &uri[old.len()..]),
        None => uri.to_owned(),
    }
}

/// Whether a triple is a top-level stamp the transform re-derives: the stamped
/// predicates on a managed (minted top-level) subject. Carrying these over from
/// the submission would duplicate or stale them.
fn is_managed_stamp(triple: &Triple, managed: &std::collections::HashSet<String>) -> bool {
    let SubjectTerm::Iri(subject) = &triple.subject else {
        return false;
    };
    if !managed.contains(subject.as_str()) {
        return false;
    }
    matches!(
        triple.predicate.as_str(),
        vocab::SBH_TOP_LEVEL
            | vocab::SBH_OWNED_BY
            | vocab::SBOL2_PERSISTENT_IDENTITY
            | vocab::SBOL2_VERSION
            | vocab::DCTERMS_CREATED
            | vocab::DCTERMS_MODIFIED
    )
}

/// Build an untagged IRI-object triple.
fn iri_triple(subject: &str, predicate: &str, object: &str) -> Triple {
    Triple {
        graph_iri: None,
        subject: SubjectTerm::Iri(IriString::unchecked(subject)),
        predicate: IriString::unchecked(predicate),
        object: ObjectTerm::Iri(IriString::unchecked(object)),
    }
}

/// Build an untagged literal-object triple.
fn literal_triple(subject: &str, predicate: &str, value: &str, datatype: &str) -> Triple {
    Triple {
        graph_iri: None,
        subject: SubjectTerm::Iri(IriString::unchecked(subject)),
        predicate: IriString::unchecked(predicate),
        object: ObjectTerm::Literal {
            value: value.to_owned(),
            datatype: IriString::unchecked(datatype),
            language: None,
        },
    }
}

/// Map a [`SerializationFormat`] to the RDF reader's format, rejecting the
/// non-RDF (GenBank/FASTA/JSON) formats a submission is never expressed in.
fn rdf_format(format: SerializationFormat) -> Result<sbol::RdfFormat, DomainError> {
    match format {
        SerializationFormat::Turtle => Ok(sbol::RdfFormat::Turtle),
        SerializationFormat::JsonLd => Ok(sbol::RdfFormat::JsonLd),
        SerializationFormat::RdfXml => Ok(sbol::RdfFormat::RdfXml),
        SerializationFormat::NTriples => Ok(sbol::RdfFormat::NTriples),
        other => Err(DomainError::InvalidInput(format!(
            "cannot mint a submission from {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "alice";
    const ID: &str = "mysubmission";
    const VERSION: &str = "1";

    /// A compliant SBOL2 document (Turtle): a ComponentDefinition with a nested
    /// SequenceAnnotation child, plus a standalone Sequence, both versioned `1`.
    /// The child's URI is a path-suffix of the ComponentDefinition's persistent
    /// identity, exercising the by-prefix re-home of children.
    const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "My Component" ;
    sbol:sequenceAnnotation <http://example.org/cd/anno/1> .

<http://example.org/cd/anno/1>
    a sbol:SequenceAnnotation ;
    sbol:displayId "anno" ;
    sbol:persistentIdentity <http://example.org/cd/anno> ;
    sbol:version "1" .

<http://example.org/seq/1>
    a sbol:Sequence ;
    sbol:displayId "seq" ;
    sbol:persistentIdentity <http://example.org/seq> ;
    sbol:version "1" ;
    sbol:elements "atgc" .
"#;

    fn submission() -> Submission {
        Submission {
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            name: Some("My Submission".to_owned()),
            description: Some("A test submission".to_owned()),
            creator_name: Some("Alice Example".to_owned()),
            citations: vec!["12345678".to_owned()],
        }
    }

    fn has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
        triples.iter().any(|t| {
            matches!(&t.subject, SubjectTerm::Iri(s) if s.as_str() == subject)
                && t.predicate.as_str() == predicate
                && matches!(&t.object, ObjectTerm::Iri(o) if o.as_str() == object)
        })
    }

    fn has_literal(triples: &[Triple], subject: &str, predicate: &str, value: &str) -> bool {
        triples.iter().any(|t| {
            matches!(&t.subject, SubjectTerm::Iri(s) if s.as_str() == subject)
                && t.predicate.as_str() == predicate
                && matches!(&t.object, ObjectTerm::Literal { value: v, .. } if v == value)
        })
    }

    #[test]
    fn mints_root_collection_at_expected_uri() {
        let minted = CollectionService::new()
            .mint_uris(&submission(), OWNER, ID, VERSION, MintScope::User)
            .expect("mint");

        assert_eq!(
            minted.collection_uri.as_str(),
            "http://synbiohub.org/user/alice/mysubmission/mysubmission_collection/1"
        );
        assert_eq!(
            minted.collection_persistent_identity.as_str(),
            "http://synbiohub.org/user/alice/mysubmission/mysubmission_collection"
        );
    }

    #[test]
    fn mints_members_at_expected_uris() {
        let minted = CollectionService::new()
            .mint_uris(&submission(), OWNER, ID, VERSION, MintScope::User)
            .expect("mint");

        let member_uris: Vec<&str> = minted.members.iter().map(|m| m.as_str()).collect();
        assert!(
            member_uris.contains(&"http://synbiohub.org/user/alice/mysubmission/cd/1"),
            "component minted under the submission namespace: {member_uris:?}"
        );
        assert!(
            member_uris.contains(&"http://synbiohub.org/user/alice/mysubmission/seq/1"),
            "sequence minted under the submission namespace: {member_uris:?}"
        );
        // Two top levels: the component and the sequence (the annotation is a
        // child, not a top level).
        assert_eq!(
            member_uris.len(),
            2,
            "exactly the two top levels: {member_uris:?}"
        );
    }

    #[test]
    fn public_scope_drops_username_segment() {
        let minted = CollectionService::new()
            .mint_uris(&submission(), OWNER, ID, VERSION, MintScope::Public)
            .expect("mint");

        assert_eq!(
            minted.collection_uri.as_str(),
            "http://synbiohub.org/public/mysubmission/mysubmission_collection/1"
        );
        assert!(minted.members.iter().all(|m| m
            .as_str()
            .starts_with("http://synbiohub.org/public/mysubmission/")));
    }

    #[test]
    fn stamps_membership_ownership_and_self_links() {
        let minted = CollectionService::new()
            .mint_uris(&submission(), OWNER, ID, VERSION, MintScope::User)
            .expect("mint");

        let collection = "http://synbiohub.org/user/alice/mysubmission/mysubmission_collection/1";
        let component = "http://synbiohub.org/user/alice/mysubmission/cd/1";
        let user_graph = "http://synbiohub.org/user/alice";

        // Collection -> object membership edge.
        assert!(
            has_iri(&minted.triples, collection, vocab::SBOL2_MEMBER, component),
            "collection members the component"
        );
        // Self-referential top-level marker on every top level.
        assert!(
            has_iri(&minted.triples, component, vocab::SBH_TOP_LEVEL, component),
            "component carries its topLevel self-link"
        );
        assert!(
            has_iri(
                &minted.triples,
                collection,
                vocab::SBH_TOP_LEVEL,
                collection
            ),
            "collection carries its topLevel self-link"
        );
        // Ownership stamp naming the owner's user graph.
        assert!(
            has_iri(&minted.triples, component, vocab::SBH_OWNED_BY, user_graph),
            "component is owned by the user graph"
        );
        assert!(
            has_iri(&minted.triples, collection, vocab::SBH_OWNED_BY, user_graph),
            "collection is owned by the user graph"
        );
    }

    #[test]
    fn stamps_shared_persistent_identity_distinct_from_version() {
        let minted = CollectionService::new()
            .mint_uris(&submission(), OWNER, ID, VERSION, MintScope::User)
            .expect("mint");

        let component = "http://synbiohub.org/user/alice/mysubmission/cd/1";
        let component_pi = "http://synbiohub.org/user/alice/mysubmission/cd";
        assert!(
            has_iri(
                &minted.triples,
                component,
                vocab::SBOL2_PERSISTENT_IDENTITY,
                component_pi
            ),
            "component has a persistent identity"
        );
        assert_ne!(
            component, component_pi,
            "the persistent identity is version-independent, distinct from the versioned URI"
        );
    }

    #[test]
    fn re_homes_children_by_prefix() {
        let minted = CollectionService::new()
            .mint_uris(&submission(), OWNER, ID, VERSION, MintScope::User)
            .expect("mint");

        let component = "http://synbiohub.org/user/alice/mysubmission/cd/1";
        let child = "http://synbiohub.org/user/alice/mysubmission/cd/anno/1";
        // The reference from the top level to its child is re-homed onto the new
        // namespace, and the child's own triples move with it.
        assert!(
            has_iri(
                &minted.triples,
                component,
                "http://sbols.org/v2#sequenceAnnotation",
                child
            ),
            "the child reference is re-homed under the minted component"
        );
        assert!(
            has_literal(&minted.triples, child, vocab::SBOL2_DISPLAY_ID, "anno"),
            "the child's own triples are re-homed onto the new child URI"
        );
        // No triple retains the old example.org namespace.
        assert!(
            !minted
                .triples
                .iter()
                .any(|t| matches!(&t.subject, SubjectTerm::Iri(s) if s.as_str().starts_with("http://example.org/"))),
            "no subject retains the submitted namespace"
        );
    }

    #[test]
    fn stamps_collection_metadata_and_citations() {
        let minted = CollectionService::new()
            .mint_uris(&submission(), OWNER, ID, VERSION, MintScope::User)
            .expect("mint");

        let collection = "http://synbiohub.org/user/alice/mysubmission/mysubmission_collection/1";
        assert!(has_literal(
            &minted.triples,
            collection,
            vocab::DCTERMS_TITLE,
            "My Submission"
        ));
        assert!(has_literal(
            &minted.triples,
            collection,
            vocab::DCTERMS_DESCRIPTION,
            "A test submission"
        ));
        assert!(has_literal(
            &minted.triples,
            collection,
            vocab::DC_CREATOR,
            "Alice Example"
        ));
        assert!(has_literal(
            &minted.triples,
            collection,
            vocab::OBI_CITATION,
            "12345678"
        ));
    }

    #[test]
    fn rejects_non_rdf_format() {
        let mut submission = submission();
        submission.format = SerializationFormat::GenBank;
        let err = CollectionService::new()
            .mint_uris(&submission, OWNER, ID, VERSION, MintScope::User)
            .expect_err("genbank is not an RDF submission format");
        assert!(matches!(err, DomainError::InvalidInput(_)));
    }

    #[test]
    fn rejects_unparseable_document() {
        let mut submission = submission();
        submission.body = "this is not turtle {{{".to_owned();
        let err = CollectionService::new()
            .mint_uris(&submission, OWNER, ID, VERSION, MintScope::User)
            .expect_err("garbage does not parse as SBOL");
        assert!(matches!(err, DomainError::Parse(_)));
    }
}
