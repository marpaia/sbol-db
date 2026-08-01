//! Destructive submission verbs: remove, replace, removeCollection, makePublic.
//!
//! [`MutationService`] joins the identity-aware [`AclService`], the pure
//! [`CollectionService`] re-mint transform, the SPARQL Update engine, and the
//! verbatim store into the mutating counterparts of the submission surface.
//!
//! Every verb is identity-gated. A caller may mutate only an object its own user
//! graph owns (its `sbh:ownedBy` names the caller's user graph); an anonymous or
//! non-owner caller is rejected with [`MutationError::NotAuthorized`], which the
//! adapter maps to `403`. Mutating a subject in the public graph additionally
//! requires an administrator, mirroring classic SynBioHub's `edit` gate. The
//! storage core stays authorization-free: the ownership fact is read from the
//! object's own triples through the [`AclService`].
//!
//! `makePublic` is a copy-then-delete, never an in-place graph move: it reads the
//! private submission's triples, re-mints every URI to the public prefix
//! (`<prefix>public/<id>/…`) through [`CollectionService::remint_triples`],
//! writes the result into the shared public graph, and then deletes the private
//! original. `remove`, `remove_collection`, and `replace` delete triples through
//! the SPARQL Update engine using the same templates classic SynBioHub issues
//! against Virtuoso (`remove.sparql`, `removeCollection.sparql`), scoped to the
//! graph that holds the target.

use std::sync::Arc;

use sbol_db_core::{DomainError, IriString, ObjectTerm, SerializationFormat, SubjectTerm};
use sbol_db_rdf::triples_to_rdf;
use sbol_db_search_sdk::{IndexMaintenanceEvent, IndexMutationSource};
use sbol_db_sparql::{SparqlError, SparqlOptions, SparqlUpdateEngine};
use sbol_db_storage::{GraphWriteMode, ImportOverwrite, SbolStore};

use crate::acl::{AclService, PUBLIC_GRAPH};
use crate::collection::{CollectionService, MintScope};
use crate::SearchMaintenanceScheduler;

/// SynBioHub vocabulary used to build the delete templates and to recognize a
/// Collection in a fetched closure.
const SBH_TERMS: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#";
const SBOL2_NS: &str = "http://sbols.org/v2#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const SBOL2_COLLECTION: &str = "http://sbols.org/v2#Collection";
const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";
const DCTERMS_DESCRIPTION: &str = "http://purl.org/dc/terms/description";
const DC_CREATOR: &str = "http://purl.org/dc/elements/1.1/creator";

/// A mutation's failure mode, kept distinct from a plain [`DomainError`] so the
/// adapter can map an authorization failure to `403` rather than `500`.
#[derive(Debug, thiserror::Error)]
pub enum MutationError {
    /// The caller does not own the target object (or is anonymous), or the
    /// target lives in the public graph and the caller is not an administrator.
    #[error("not authorized to mutate {0}")]
    NotAuthorized(String),
    /// The target object does not exist in any graph the caller can reach.
    #[error("no such object: {0}")]
    NotFound(String),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Sparql(#[from] SparqlError),
}

/// A makePublic request: the private object to publish plus the target public
/// identity and the metadata stamped onto the new public root collection.
#[derive(Clone, Debug)]
pub struct MakePublicRequest {
    /// The private, version-qualified object to make public (typically the
    /// submission's root Collection).
    pub source_uri: String,
    /// The username whose graph is stamped as `sbh:ownedBy` on the public
    /// objects. Classic keeps the originating owner on a published object.
    pub owner_username: String,
    /// The public submission id, the collection segment of every public URI.
    pub public_id: String,
    /// The version segment of every public URI.
    pub version: String,
    /// The public collection title (`dcterms:title`).
    pub name: Option<String>,
    /// The public collection description (`dcterms:description`).
    pub description: Option<String>,
    /// The creator name (`dc:creator`).
    pub creator_name: Option<String>,
    /// PubMed citations, stamped as `obo:OBI_0001617` literals.
    pub citations: Vec<String>,
    /// Collision policy against the public collection URI: [`ImportOverwrite::Fail`]
    /// rejects an id/version already in use; `Replace`/`Merge` publish anyway.
    pub overwrite: ImportOverwrite,
}

/// The result of a successful makePublic.
#[derive(Clone, Debug)]
pub struct MakePublicOutcome {
    /// The minted public root Collection URI.
    pub collection_uri: IriString,
    /// The minted public URI of every top-level member.
    pub members: Vec<IriString>,
    /// The number of triples written to the public graph.
    pub triple_count: usize,
}

/// The mutating submission verbs, gated on caller ownership.
#[derive(Clone)]
pub struct MutationService {
    store: Arc<dyn SbolStore>,
    sparql_update: Arc<SparqlUpdateEngine>,
    acl_service: AclService,
    collection: CollectionService,
    maintenance: Option<Arc<SearchMaintenanceScheduler>>,
}

impl MutationService {
    /// Build the service over the store, the SPARQL Update engine, and the ACL
    /// service, minting public URIs under the default database prefix.
    pub fn new(
        store: Arc<dyn SbolStore>,
        sparql_update: Arc<SparqlUpdateEngine>,
        acl_service: AclService,
    ) -> Self {
        Self {
            store,
            sparql_update,
            acl_service,
            collection: CollectionService::new(),
            maintenance: None,
        }
    }

    /// Mint published identities under a deployment-specific database prefix.
    pub fn with_database_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.collection = self.collection.with_database_prefix(prefix);
        self
    }

    /// Attach automatic search maintenance to this mutation service.
    pub fn with_maintenance(mut self, maintenance: Arc<SearchMaintenanceScheduler>) -> Self {
        self.maintenance = Some(maintenance);
        self
    }

    /// Delete a top-level object and the triples of everything whose
    /// `sbh:topLevel` names it, mirroring classic `remove.sparql`. References
    /// held by other objects are left intact.
    pub async fn remove(
        &self,
        user_graph: &str,
        is_admin: bool,
        object_uri: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, object_uri).await?;
        self.run_update(&remove_query(object_uri), &graph).await?;
        self.schedule_reconciliation().await?;
        Ok(())
    }

    /// Delete a Collection and every member's closure under the submission's URI
    /// prefix, then the Collection itself, mirroring classic
    /// `removeCollection.sparql` followed by `remove.sparql`.
    pub async fn remove_collection(
        &self,
        user_graph: &str,
        is_admin: bool,
        collection_uri: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, collection_uri).await?;
        let prefix = submission_prefix(collection_uri);
        self.run_update(&remove_collection_query(collection_uri, &prefix), &graph)
            .await?;
        self.run_update(&remove_query(collection_uri), &graph)
            .await?;
        self.schedule_reconciliation().await?;
        Ok(())
    }

    /// Delete a top-level object's own triples while leaving references to it
    /// intact, so a fresh version can take its place. This is classic `replace`:
    /// it runs `remove.sparql` and expects a follow-up submission.
    pub async fn replace(
        &self,
        user_graph: &str,
        is_admin: bool,
        object_uri: &str,
    ) -> Result<(), MutationError> {
        self.remove(user_graph, is_admin, object_uri).await
    }

    /// Publish a private object to the shared public graph.
    ///
    /// Reads the private submission's triples, re-mints every URI to the public
    /// prefix, writes the result into the public graph, and deletes the private
    /// original. A copy-then-delete, not an in-place move: the private and public
    /// forms have distinct URIs, so the closure is re-minted rather than
    /// relabeled.
    pub async fn make_public(
        &self,
        user_graph: &str,
        is_admin: bool,
        request: MakePublicRequest,
    ) -> Result<MakePublicOutcome, MutationError> {
        // Ownership is checked against the private source; a private object never
        // lives in the public graph, so the admin-only public gate does not trip.
        let private_graph = self
            .authorize(user_graph, is_admin, &request.source_uri)
            .await?;

        // The submission's closure. Each submission is its own named graph, so
        // the private graph's triples are exactly the object's closure; reading
        // the graph is both complete (children included) and self-contained.
        let closure = self.store.graph_store_read(&private_graph).await?;

        // Reduce the closure to exactly what classic publishes: for a
        // Collection, the transitive closure of its `sbol:member` targets (never
        // the Collection's own triples, which the re-mint rebuilds, and never an
        // orphan left in the private graph after a `removeMembership`); for a
        // single object, its own closure. The makePublic form carries no
        // title/description/creator, so when the request omits them they are
        // inherited from the private collection's existing metadata, matching
        // classic SynBioHub which preserves the collection's descriptive fields
        // across publication.
        let (source, name, description, creator_name) =
            if is_collection(&closure, &request.source_uri) {
                let name = request
                    .name
                    .clone()
                    .or_else(|| literal_object(&closure, &request.source_uri, DCTERMS_TITLE));
                let description = request
                    .description
                    .clone()
                    .or_else(|| literal_object(&closure, &request.source_uri, DCTERMS_DESCRIPTION));
                let creator_name = request
                    .creator_name
                    .clone()
                    .or_else(|| literal_object(&closure, &request.source_uri, DC_CREATOR));
                let members = member_uris(&closure, &request.source_uri);
                (
                    reachable_closure(closure, &members),
                    name,
                    description,
                    creator_name,
                )
            } else {
                (
                    closure,
                    request.name.clone(),
                    request.description.clone(),
                    request.creator_name.clone(),
                )
            };

        let minted = self.collection.remint_triples(
            source,
            &request.owner_username,
            &request.public_id,
            &request.version,
            MintScope::Public,
            name.as_deref(),
            description.as_deref(),
            creator_name.as_deref(),
            &request.citations,
        );

        let public_collection = minted.collection_uri.as_str().to_owned();
        if request.overwrite == ImportOverwrite::Fail
            && !self
                .store
                .triples_for_subject(&public_collection)
                .await?
                .is_empty()
        {
            return Err(MutationError::Domain(DomainError::InvalidInput(format!(
                "public submission already exists at {public_collection}"
            ))));
        }

        // Public objects share the single public graph, so append rather than
        // replace: a makePublic never clobbers unrelated public submissions.
        let body = triples_to_rdf(&minted.triples, SerializationFormat::NTriples)?;
        let triple_count = self
            .store
            .graph_store_write(
                PUBLIC_GRAPH,
                &body,
                SerializationFormat::NTriples,
                GraphWriteMode::Merge,
            )
            .await?;

        // Delete the private original. The submission owns its graph outright, so
        // clearing the graph removes the private closure in full.
        self.store.graph_store_clear(&private_graph).await?;
        self.schedule_reconciliation().await?;

        Ok(MakePublicOutcome {
            collection_uri: minted.collection_uri,
            members: minted.members,
            triple_count,
        })
    }

    /// Resolve the graph holding `uri` and enforce the write gate: an anonymous
    /// or non-owning caller is rejected, a public-graph subject requires an
    /// administrator, and an unknown subject is [`MutationError::NotFound`].
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

    /// Execute one SPARQL Update scoped to `graph`: the `default-graph-uri` the
    /// template's `WHERE`/`DELETE` operate over, matching Virtuoso semantics.
    async fn run_update(&self, update: &str, graph: &str) -> Result<(), SparqlError> {
        self.sparql_update
            .execute(update, Some(graph), &SparqlOptions::default())
            .await?;
        Ok(())
    }

    /// Removal and publication can affect an arbitrary closure of objects, so
    /// they deliberately request the plugin's corpus-level reconciliation path.
    async fn schedule_reconciliation(&self) -> Result<(), MutationError> {
        if let Some(maintenance) = &self.maintenance {
            maintenance
                .schedule(IndexMaintenanceEvent::corpus(
                    IndexMutationSource::ObjectMutation,
                ))
                .await?;
        }
        Ok(())
    }
}

/// The classic `remove.sparql`: delete every triple of every subject whose
/// `sbh:topLevel` names `uri`.
fn remove_query(uri: &str) -> String {
    format!(
        "PREFIX sbh: <{SBH_TERMS}>\n\
         DELETE {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o . ?s sbh:topLevel <{uri}> }}"
    )
}

/// The classic `removeCollection.sparql`: delete every member's closure, keyed
/// off the Collection's `sbol2:member` edges and narrowed to the submission's
/// URI prefix.
fn remove_collection_query(collection: &str, uri_prefix: &str) -> String {
    format!(
        "PREFIX sbh: <{SBH_TERMS}>\n\
         PREFIX sbol2: <{SBOL2_NS}>\n\
         DELETE {{ ?s ?p ?o }} WHERE {{ \
         ?s ?p ?o . \
         <{collection}> sbol2:member ?member . \
         ?s sbh:topLevel ?member . \
         FILTER(STRSTARTS(str(?s), '{uri_prefix}')) }}"
    )
}

/// The submission URI prefix of a collection URI, mirroring classic's two-step
/// truncation: drop the version segment, then drop the `<id>_collection`
/// segment, leaving `<prefix>{user/<u>|public}/<id>/`.
fn submission_prefix(collection_uri: &str) -> String {
    let without_version = match collection_uri.rfind('/') {
        Some(idx) => &collection_uri[..idx],
        None => collection_uri,
    };
    match without_version.rfind('/') {
        Some(idx) => collection_uri[..=idx].to_owned(),
        None => without_version.to_owned(),
    }
}

/// Whether the closure types `uri` as an SBOL2 Collection.
fn is_collection(triples: &[sbol_db_core::Triple], uri: &str) -> bool {
    triples.iter().any(|t| {
        matches!(&t.subject, SubjectTerm::Iri(s) if s.as_str() == uri)
            && t.predicate.as_str() == RDF_TYPE
            && matches!(&t.object, ObjectTerm::Iri(o) if o.as_str() == SBOL2_COLLECTION)
    })
}

/// The literal value of the first `(uri, predicate, ?o)` triple in the closure,
/// used to carry a collection's descriptive metadata across makePublic.
fn literal_object(triples: &[sbol_db_core::Triple], uri: &str, predicate: &str) -> Option<String> {
    triples.iter().find_map(|t| {
        let SubjectTerm::Iri(s) = &t.subject else {
            return None;
        };
        if s.as_str() != uri || t.predicate.as_str() != predicate {
            return None;
        }
        match &t.object {
            ObjectTerm::Literal { value, .. } => Some(value.clone()),
            _ => None,
        }
    })
}

/// The `sbol:member` targets of `collection_uri` in the closure.
fn member_uris(triples: &[sbol_db_core::Triple], collection_uri: &str) -> Vec<String> {
    let member = format!("{SBOL2_NS}member");
    triples
        .iter()
        .filter_map(|t| {
            let SubjectTerm::Iri(s) = &t.subject else {
                return None;
            };
            if s.as_str() != collection_uri || t.predicate.as_str() != member {
                return None;
            }
            match &t.object {
                ObjectTerm::Iri(o) => Some(o.as_str().to_owned()),
                _ => None,
            }
        })
        .collect()
}

/// The transitive closure reachable from `seeds`: every triple whose subject is
/// a seed or an object IRI reachable from one. This is what classic publishes
/// for a Collection, the union of its members' object closures; an orphan not
/// reachable from any member is excluded.
fn reachable_closure(
    triples: Vec<sbol_db_core::Triple>,
    seeds: &[String],
) -> Vec<sbol_db_core::Triple> {
    let mut visited: std::collections::HashSet<String> = seeds.iter().cloned().collect();
    let mut frontier: Vec<String> = seeds.to_vec();
    while let Some(current) = frontier.pop() {
        for triple in &triples {
            let SubjectTerm::Iri(s) = &triple.subject else {
                continue;
            };
            if s.as_str() != current {
                continue;
            }
            if let ObjectTerm::Iri(o) = &triple.object {
                if visited.insert(o.as_str().to_owned()) {
                    frontier.push(o.as_str().to_owned());
                }
            }
        }
    }
    triples
        .into_iter()
        .filter(|t| matches!(&t.subject, SubjectTerm::Iri(s) if visited.contains(s.as_str())))
        .collect()
}
