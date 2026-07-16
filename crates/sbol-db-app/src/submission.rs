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

use sbol_db_core::{DomainError, IriString, SerializationFormat};
use sbol_db_rdf::triples_to_rdf;
use sbol_db_storage::{GraphWriteMode, ImportOverwrite, SbolStore};

use crate::collection::{CollectionService, MintScope, Submission};

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

/// Mints a submission and writes it to the caller's own graph.
#[derive(Clone)]
pub struct SubmissionService {
    store: Arc<dyn SbolStore>,
    collection: CollectionService,
}

impl SubmissionService {
    /// Build a service over the store, minting under the default database
    /// prefix.
    pub fn new(store: Arc<dyn SbolStore>) -> Self {
        Self {
            store,
            collection: CollectionService::new(),
        }
    }

    /// Build a service with an explicit [`CollectionService`], e.g. one minting
    /// under a deployment-specific database prefix.
    pub fn with_collection_service(
        store: Arc<dyn SbolStore>,
        collection: CollectionService,
    ) -> Self {
        Self { store, collection }
    }

    /// Mint `request` into the caller's user namespace and write the resulting
    /// triples to the minted collection's graph.
    ///
    /// The minted collection URI names the target graph, so each submission is
    /// its own graph: [`ImportOverwrite::Fail`] pre-checks that graph is empty
    /// (the id/version is free) and errors on a collision, `Replace` clears it
    /// first, and `Merge` unions into it.
    pub async fn submit(&self, request: SubmitRequest) -> Result<SubmitOutcome, DomainError> {
        let submission = Submission {
            body: request.body,
            format: request.format,
            name: request.name,
            description: request.description,
            creator_name: request.creator_name,
            citations: request.citations,
        };
        let minted = self.collection.mint_uris(
            &submission,
            &request.owner,
            &request.id,
            &request.version,
            MintScope::User,
        )?;

        let graph_iri = minted.collection_uri.as_str().to_owned();

        let mode = match request.overwrite {
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
        let triple_count = self
            .store
            .graph_store_write(&graph_iri, &body, SerializationFormat::NTriples, mode)
            .await?;

        Ok(SubmitOutcome {
            collection_uri: minted.collection_uri,
            collection_persistent_identity: minted.collection_persistent_identity,
            members: minted.members,
            graph_iri,
            triple_count,
        })
    }
}
