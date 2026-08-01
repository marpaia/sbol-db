//! The submission verb: mint a document and write it to the caller's graph.
//!
//! [`SubmissionService`] joins the pure [`CollectionService`] minting transform
//! to the store's verbatim graph write. A submission is minted into the caller's
//! own user namespace, then its triples are written to a named graph keyed by
//! the minted root Collection URI. Because the minted triples carry the
//! `sbh:ownedBy` stamp naming the caller's user graph, the `AclService` admits
//! that graph to the caller's read scope, so the caller reads its own submission
//! back while other callers do not.
//!
//! SynBioHub posts SBOL2, and the derived views read those triples verbatim, so
//! the write goes through [`SbolStore::graph_store_write`] (the arbitrary-RDF
//! ingest that stores triples exactly as received) rather than the SBOL-model
//! import path, which would upgrade SBOL2 to SBOL3 and re-mint the URIs the
//! submission just fixed.

use std::sync::Arc;

use std::collections::BTreeSet;

use sbol_db_core::{DomainError, IriString, SerializationFormat, SubjectTerm};
use sbol_db_derive::parse_import_document;
use sbol_db_rdf::triples_to_rdf;
use sbol_db_search_sdk::{DocumentId, IndexMaintenanceEvent, IndexMutationSource};
use sbol_db_storage::{GraphWriteMode, ImportInput, ImportOverwrite, SbolStore};
use serde::Serialize;

use crate::collection::{CollectionService, MintScope, Submission};
use crate::SearchMaintenanceScheduler;

/// A submission request: the document to mint plus the caller and target
/// identity. `owner` is the authenticated caller's username; the service never
/// mints into a namespace other than the caller's own, so a caller can only
/// write into a graph it owns.
#[derive(Clone, Debug)]
pub struct SubmitRequest {
    /// The authenticated caller's username. The submission mints under
    /// `<prefix>user/<owner>/…` and stamps `sbh:ownedBy <prefix>user/<owner>`.
    pub owner: String,
    /// The submission id, the collection segment of every minted URI.
    pub id: String,
    /// The version segment of every minted URI.
    pub version: String,
    /// The collection title (`dcterms:title`).
    pub name: Option<String>,
    /// The collection description (`dcterms:description`).
    pub description: Option<String>,
    /// The creator name (`dc:creator`).
    pub creator_name: Option<String>,
    /// PubMed citations, stamped as `obo:OBI_0001617` literals.
    pub citations: Vec<String>,
    /// The serialized SBOL document.
    pub body: String,
    /// The serialization the body is expressed in.
    pub format: SerializationFormat,
    /// How the write combines with an existing submission at the same minted
    /// graph: [`ImportOverwrite::Fail`] rejects a collision, `Replace` deletes
    /// the existing submission and writes the new one, `Merge` adds to it.
    pub overwrite: ImportOverwrite,
}

/// The result of a successful submission: the minted identity a caller reads
/// back by, and the write's triple count.
#[derive(Clone, Debug)]
pub struct SubmitOutcome {
    /// The minted, version-qualified root Collection URI.
    pub collection_uri: IriString,
    /// The root Collection's version-independent persistent identity.
    pub collection_persistent_identity: IriString,
    /// The minted URI of every top-level member of the root collection.
    pub members: Vec<IriString>,
    /// The named graph the submission's triples were written to (the minted
    /// collection URI).
    pub graph_iri: String,
    /// The number of triples written.
    pub triple_count: usize,
}

/// A write-free submission analysis returned before the caller commits.
/// Identity minting and validation are identical to [`SubmissionService::submit`]
/// so the preview names the exact graph and members the eventual write targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubmitPreview {
    pub valid: bool,
    pub source_format: String,
    pub source_standard: String,
    pub normalized_standard: String,
    pub collection_uri: String,
    pub persistent_identity: String,
    pub graph: String,
    pub members: Vec<String>,
    pub triple_count: usize,
    pub collision: bool,
    pub consequence: SubmitConsequence,
    pub notices: Vec<SubmitNotice>,
}

/// What committing the currently previewed request will do at its target graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitConsequence {
    Create,
    RejectConflict,
    Replace,
    Merge,
}

/// A stable, user-facing validation or persistence consequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubmitNotice {
    pub code: String,
    pub level: SubmitNoticeLevel,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitNoticeLevel {
    Info,
    Warning,
}

struct PreparedSubmission {
    minted: crate::collection::MintedSubmission,
    source_format: SerializationFormat,
    source_standard: String,
    normalized_standard: String,
    notices: Vec<SubmitNotice>,
}

/// Mints a submission and writes it to the caller's own graph.
#[derive(Clone)]
pub struct SubmissionService {
    store: Arc<dyn SbolStore>,
    collection: CollectionService,
    maintenance: Option<Arc<SearchMaintenanceScheduler>>,
}

impl SubmissionService {
    /// Build a service over the store, minting under the default database
    /// prefix.
    pub fn new(store: Arc<dyn SbolStore>) -> Self {
        Self {
            store,
            collection: CollectionService::new(),
            maintenance: None,
        }
    }

    /// Build a service with an explicit [`CollectionService`], e.g. one minting
    /// under a deployment-specific database prefix.
    pub fn with_collection_service(
        store: Arc<dyn SbolStore>,
        collection: CollectionService,
    ) -> Self {
        Self {
            store,
            collection,
            maintenance: None,
        }
    }

    /// Build a submission service that emits maintenance intents after a
    /// successful graph write.
    pub fn with_maintenance(
        store: Arc<dyn SbolStore>,
        collection: CollectionService,
        maintenance: Arc<SearchMaintenanceScheduler>,
    ) -> Self {
        Self {
            store,
            collection,
            maintenance: Some(maintenance),
        }
    }

    /// Mint `request` into the caller's user namespace and write the resulting
    /// triples to the minted collection's graph.
    ///
    /// The minted collection URI names the target graph, so each submission is
    /// its own graph: [`ImportOverwrite::Fail`] pre-checks that graph is empty
    /// (the id/version is free) and errors on a collision, `Replace` clears it
    /// first, and `Merge` unions into it.
    pub async fn submit(&self, request: SubmitRequest) -> Result<SubmitOutcome, DomainError> {
        let prepared = self.prepare(&request)?;
        let minted = prepared.minted;

        self.write_prepared(minted, request.overwrite).await
    }

    /// Validate, convert when necessary, and mint a submission without writing
    /// any graph. A client can show this projection as a review step; commit
    /// repeats the same preparation so changed input and collision races are
    /// still rejected authoritatively.
    pub async fn preview(&self, request: &SubmitRequest) -> Result<SubmitPreview, DomainError> {
        let prepared = self.prepare(request)?;
        let graph = prepared.minted.collection_uri.as_str().to_owned();
        let collision = !self.store.graph_store_read(&graph).await?.is_empty();
        let consequence = match (collision, request.overwrite) {
            (false, _) => SubmitConsequence::Create,
            (true, ImportOverwrite::Fail) => SubmitConsequence::RejectConflict,
            (true, ImportOverwrite::Replace) => SubmitConsequence::Replace,
            (true, ImportOverwrite::Merge) => SubmitConsequence::Merge,
        };
        let mut notices = prepared.notices;
        if collision {
            let (code, level, message) = match consequence {
                SubmitConsequence::RejectConflict => (
                    "identity_conflict",
                    SubmitNoticeLevel::Warning,
                    "The target identity already exists. Commit will be rejected unless you choose replace or merge.",
                ),
                SubmitConsequence::Replace => (
                    "replace_existing",
                    SubmitNoticeLevel::Warning,
                    "Commit will replace the existing submission at this exact graph identity.",
                ),
                SubmitConsequence::Merge => (
                    "merge_existing",
                    SubmitNoticeLevel::Warning,
                    "Commit will merge these triples into the existing submission graph.",
                ),
                SubmitConsequence::Create => unreachable!(),
            };
            notices.push(SubmitNotice {
                code: code.to_owned(),
                level,
                message: message.to_owned(),
            });
        }
        Ok(SubmitPreview {
            valid: true,
            source_format: prepared.source_format.as_db_str().to_owned(),
            source_standard: prepared.source_standard,
            normalized_standard: prepared.normalized_standard,
            collection_uri: graph.clone(),
            persistent_identity: prepared
                .minted
                .collection_persistent_identity
                .as_str()
                .to_owned(),
            graph,
            members: prepared
                .minted
                .members
                .iter()
                .map(|member| member.as_str().to_owned())
                .collect(),
            triple_count: prepared.minted.triples.len(),
            collision,
            consequence,
            notices,
        })
    }

    fn prepare(&self, request: &SubmitRequest) -> Result<PreparedSubmission, DomainError> {
        let source_format = request.format;
        let source_standard = source_standard(&request.body, source_format);
        let (body, format, normalized_standard, notices) =
            normalize_sequence_submission(request, self.collection.database_prefix())?;
        let submission = Submission {
            body,
            format,
            name: request.name.clone(),
            description: request.description.clone(),
            creator_name: request.creator_name.clone(),
            citations: request.citations.clone(),
        };
        let minted = self.collection.mint_uris(
            &submission,
            &request.owner,
            &request.id,
            &request.version,
            MintScope::User,
        )?;

        Ok(PreparedSubmission {
            minted,
            source_format,
            source_standard,
            normalized_standard,
            notices,
        })
    }

    async fn write_prepared(
        &self,
        minted: crate::collection::MintedSubmission,
        overwrite: ImportOverwrite,
    ) -> Result<SubmitOutcome, DomainError> {
        let graph_iri = minted.collection_uri.as_str().to_owned();

        let mode = match overwrite {
            ImportOverwrite::Fail => {
                if !self.store.graph_store_read(&graph_iri).await?.is_empty() {
                    return Err(DomainError::InvalidInput(format!(
                        "a submission already exists at {graph_iri}; submit with overwrite or merge"
                    )));
                }
                GraphWriteMode::Merge
            }
            ImportOverwrite::Replace => GraphWriteMode::Replace,
            ImportOverwrite::Merge => GraphWriteMode::Merge,
        };

        let body = triples_to_rdf(&minted.triples, SerializationFormat::NTriples)?;
        // `members` contains only top-level objects. Collect every minted IRI
        // subject instead, so the event remains complete if a current or future
        // projection includes nested SBOL subjects. Blank nodes have no stable
        // document identity to schedule.
        let indexed_subjects = minted
            .triples
            .iter()
            .filter_map(|triple| match &triple.subject {
                SubjectTerm::Iri(iri) => Some(DocumentId(iri.as_str().to_owned())),
                SubjectTerm::BlankNode(_) => None,
            })
            .collect::<BTreeSet<_>>();
        let triple_count = self
            .store
            .graph_store_write(&graph_iri, &body, SerializationFormat::NTriples, mode)
            .await?;

        if let Some(maintenance) = &self.maintenance {
            maintenance
                .schedule(IndexMaintenanceEvent::documents(
                    IndexMutationSource::Submission,
                    indexed_subjects,
                ))
                .await?;
        }

        Ok(SubmitOutcome {
            collection_uri: minted.collection_uri,
            collection_persistent_identity: minted.collection_persistent_identity,
            members: minted.members,
            graph_iri,
            triple_count,
        })
    }
}

fn source_standard(body: &str, format: SerializationFormat) -> String {
    match format {
        SerializationFormat::GenBank => "genbank".to_owned(),
        SerializationFormat::Fasta => "fasta".to_owned(),
        _ => match sbol::detect_version(body, rdf_format_for_detection(format)) {
            Some(sbol::SbolVersion::V2) => "sbol2".to_owned(),
            Some(sbol::SbolVersion::V3) => "sbol3".to_owned(),
            Some(_) | None => "rdf".to_owned(),
        },
    }
}

fn rdf_format_for_detection(format: SerializationFormat) -> sbol::RdfFormat {
    match format {
        SerializationFormat::JsonLd | SerializationFormat::Json => sbol::RdfFormat::JsonLd,
        SerializationFormat::RdfXml => sbol::RdfFormat::RdfXml,
        SerializationFormat::Turtle | SerializationFormat::TriG => sbol::RdfFormat::Turtle,
        SerializationFormat::NTriples | SerializationFormat::NQuads => sbol::RdfFormat::NTriples,
        SerializationFormat::GenBank | SerializationFormat::Fasta => sbol::RdfFormat::Turtle,
    }
}

fn normalize_sequence_submission(
    request: &SubmitRequest,
    database_prefix: &str,
) -> Result<(String, SerializationFormat, String, Vec<SubmitNotice>), DomainError> {
    if !matches!(
        request.format,
        SerializationFormat::GenBank | SerializationFormat::Fasta
    ) {
        return Ok((
            request.body.clone(),
            request.format,
            source_standard(&request.body, request.format),
            Vec::new(),
        ));
    }

    let conversion_namespace = format!(
        "{database_prefix}user/{}/{}/source",
        request.owner, request.id,
    );
    let document = parse_import_document(&ImportInput {
        body: request.body.clone(),
        format: request.format,
        namespace: Some(conversion_namespace),
        source_uri: None,
        document_iri: None,
        created_by: Some(request.owner.clone()),
        name: request.name.clone(),
        description: request.description.clone(),
        overwrite: ImportOverwrite::Fail,
    })?;
    let report = document.validate();
    if report.has_errors() {
        return Err(DomainError::Validation(report.to_string()));
    }
    let body = document
        .write(sbol::RdfFormat::NTriples)
        .map_err(|error| DomainError::Serialization(error.to_string()))?;
    let mut notices = vec![SubmitNotice {
        code: "converted_to_sbol3".to_owned(),
        level: SubmitNoticeLevel::Info,
        message: format!(
            "{} input will be converted to SBOL 3 before identity minting.",
            if request.format == SerializationFormat::GenBank {
                "GenBank"
            } else {
                "FASTA"
            }
        ),
    }];
    if report.warnings().next().is_some() {
        notices.push(SubmitNotice {
            code: "validation_warnings".to_owned(),
            level: SubmitNoticeLevel::Warning,
            message: report.to_string(),
        });
    }
    Ok((
        body,
        SerializationFormat::NTriples,
        "sbol3".to_owned(),
        notices,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use async_trait::async_trait;
    use sbol_db_backend::Backend;
    use sbol_db_search_sdk::{
        IndexMaintenanceDescriptor, IndexMaintenancePlugin, IndexMaintenanceRegistry,
        IndexMaintenanceTask, SearchError,
    };
    use sbol_db_storage::{ListJobsFilter, ListObjectsFilter};

    use super::*;
    use crate::SearchMaintenanceScheduler;

    const DOCUMENT: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> ;
    sbol:sequenceAnnotation <http://example.org/cd/anno/1> .

<http://example.org/cd/anno/1>
    a sbol:SequenceAnnotation ;
    sbol:displayId "anno" ;
    sbol:persistentIdentity <http://example.org/cd/anno> ;
    sbol:version "1" ;
    sbol:location <http://example.org/cd/anno/range/1> .

<http://example.org/cd/anno/range/1>
    a sbol:Range ;
    sbol:displayId "range" ;
    sbol:persistentIdentity <http://example.org/cd/anno/range> ;
    sbol:version "1" ;
    sbol:start "1"^^xsd:integer ;
    sbol:end "4"^^xsd:integer .
"#;

    struct ProbePlugin {
        descriptor: IndexMaintenanceDescriptor,
    }

    #[async_trait]
    impl IndexMaintenancePlugin for ProbePlugin {
        fn descriptor(&self) -> &IndexMaintenanceDescriptor {
            &self.descriptor
        }

        async fn plan(
            &self,
            event: &IndexMaintenanceEvent,
        ) -> Result<Vec<IndexMaintenanceTask>, SearchError> {
            Ok(vec![IndexMaintenanceTask::new(
                "submission_maintenance_probe",
                serde_json::json!({ "event": event }),
            )])
        }
    }

    #[tokio::test]
    async fn submit_enqueues_every_minted_iri_subject() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("submit.db").display());
        let backend = Backend::open(&database_url).await.unwrap();
        backend
            .migrator
            .as_ref()
            .unwrap()
            .run_migrations()
            .await
            .unwrap();

        let plugins = IndexMaintenanceRegistry::builder()
            .register(ProbePlugin {
                descriptor: IndexMaintenanceDescriptor {
                    id: "test.submission.maintenance.v1".to_owned(),
                    display_name: "Submission probe".to_owned(),
                    description: "captures committed submission events".to_owned(),
                },
            })
            .unwrap()
            .build();
        let service = SubmissionService::with_maintenance(
            backend.store.clone(),
            CollectionService::new(),
            Arc::new(SearchMaintenanceScheduler::new(
                backend.jobs.clone(),
                Arc::new(plugins),
            )),
        );
        let outcome = service
            .submit(SubmitRequest {
                owner: "alice".to_owned(),
                id: "submission".to_owned(),
                version: "1".to_owned(),
                name: None,
                description: None,
                creator_name: None,
                citations: Vec::new(),
                body: DOCUMENT.to_owned(),
                format: SerializationFormat::Turtle,
                overwrite: ImportOverwrite::Fail,
            })
            .await
            .unwrap();

        let jobs = backend
            .jobs
            .list(&ListJobsFilter {
                kind: Some("submission_maintenance_probe".to_owned()),
                limit: 10,
                ..ListJobsFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        let scheduled = jobs[0].payload["event"]["document_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let stored = backend
            .store
            .list_objects(&ListObjectsFilter::default())
            .await
            .unwrap()
            .into_iter()
            .map(|record| record.iri.into_inner())
            .collect::<BTreeSet<_>>();

        assert!(scheduled.contains(outcome.collection_uri.as_str()));
        assert!(scheduled.contains("http://synbiohub.org/user/alice/submission/cd/anno/1"));
        assert!(
            stored.is_subset(&scheduled),
            "all derived records must appear in the precise maintenance event"
        );
    }
}
