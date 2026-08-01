//! Whole-collection machine synchronization over the same authorization and
//! storage contracts as interactive SBOL DB mutations.
//!
//! The synchronized representation contains biological SBOL only. Ownership,
//! sharing, timestamps, top-level indexes, and audit evidence remain
//! server-managed and are excluded from the content ETag.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use sbol_db_core::{DomainError, IriString, ObjectTerm, SerializationFormat, SubjectTerm, Triple};
use sbol_db_derive::to_rdf_format;
use sbol_db_rdf::rdf_graph_to_triples;
use sbol_db_search_sdk::{DocumentId, IndexMaintenanceEvent, IndexMutationSource};
use sbol_db_sparql::GraphScope;
use sbol_db_storage::{
    biological_content, collection_content_etag, ConditionalContentWrite, SbolStore, SBH_OWNED_BY,
};

use crate::{AclService, MutationError, SearchMaintenanceScheduler};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const SBOL2_COLLECTION: &str = "http://sbols.org/v2#Collection";
const SBOL3_COLLECTION: &str = "http://sbols.org/v3#Collection";
const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";
const SBOL3_MEMBER: &str = "http://sbols.org/v3#member";
const SBH_TOP_LEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";

/// The biological collection document plus its representation-independent
/// validator. The triples are graphless so callers can serialize them as an
/// ordinary SBOL file.
#[derive(Clone, Debug)]
pub struct CollectionContent {
    pub triples: Vec<Triple>,
    pub content_etag: String,
}

/// Validated effect of a whole-collection replacement before any store write.
#[derive(Clone, Debug)]
pub struct CollectionUpdatePreview {
    pub current_content_etag: String,
    pub proposed_content_etag: String,
    pub triple_count: usize,
}

#[derive(Clone)]
pub struct CollectionSyncService {
    store: Arc<dyn SbolStore>,
    acl: AclService,
    maintenance: Option<Arc<SearchMaintenanceScheduler>>,
}

impl CollectionSyncService {
    pub fn new(store: Arc<dyn SbolStore>, acl: AclService) -> Self {
        Self {
            store,
            acl,
            maintenance: None,
        }
    }

    pub fn with_maintenance(
        store: Arc<dyn SbolStore>,
        acl: AclService,
        maintenance: Arc<SearchMaintenanceScheduler>,
    ) -> Self {
        Self {
            store,
            acl,
            maintenance: Some(maintenance),
        }
    }

    /// Return `None` for both an absent graph and one outside the caller's
    /// public/owned/shared scope, preserving the ordinary non-disclosure rule.
    pub async fn read(
        &self,
        caller_graph: Option<&str>,
        collection: &str,
    ) -> Result<Option<CollectionContent>, DomainError> {
        IriString::new(collection.to_owned())?;
        let scope = self.acl.compute_scope(caller_graph).await?;
        if !graph_allowed(collection, &scope) {
            return Ok(None);
        }
        let current = self.store.graph_store_read(collection).await?;
        let triples = biological_content(&current);
        let Some(content_etag) = collection_content_etag(&triples) else {
            return Ok(None);
        };
        Ok(Some(CollectionContent {
            triples,
            content_etag,
        }))
    }

    /// Validate and authorize a replacement against one exact biological
    /// content ETag without changing registry state.
    pub async fn preview_update(
        &self,
        caller_graph: &str,
        is_admin: bool,
        collection: &str,
        body: &str,
        format: SerializationFormat,
        expected_content_etag: &str,
    ) -> Result<CollectionUpdatePreview, MutationError> {
        IriString::new(caller_graph.to_owned()).map_err(DomainError::from)?;
        IriString::new(collection.to_owned()).map_err(DomainError::from)?;
        let incoming = parse_and_validate_collection(body, format, collection)?;
        let current = self.store.graph_store_read(collection).await?;
        let current_content_etag = collection_content_etag(&biological_content(&current))
            .ok_or_else(|| MutationError::NotFound(collection.to_owned()))?;
        if !self
            .acl
            .can_write(caller_graph, is_admin, collection, collection)
            .await?
        {
            return Err(MutationError::NotAuthorized(collection.to_owned()));
        }
        if expected_content_etag != current_content_etag {
            return Err(MutationError::Domain(DomainError::Validation(format!(
                "collection content changed; current ETag is {current_content_etag}"
            ))));
        }
        let proposed_content_etag = collection_content_etag(&incoming).ok_or_else(|| {
            MutationError::Domain(DomainError::InvalidInput(
                "the proposed collection has no biological content".to_owned(),
            ))
        })?;
        Ok(CollectionUpdatePreview {
            current_content_etag,
            proposed_content_etag,
            triple_count: incoming.len(),
        })
    }

    /// Create when `expected_content_etag` is `None`, otherwise update by
    /// compare-and-swap. Client-supplied collaboration metadata is filtered by
    /// storage; this service regenerates ownership/top-level stamps from the
    /// authenticated principal and the live graph.
    pub async fn write(
        &self,
        caller_graph: &str,
        is_admin: bool,
        collection: &str,
        body: &str,
        format: SerializationFormat,
        expected_content_etag: Option<&str>,
    ) -> Result<ConditionalContentWrite, MutationError> {
        IriString::new(caller_graph.to_owned()).map_err(DomainError::from)?;
        IriString::new(collection.to_owned()).map_err(DomainError::from)?;
        let incoming = parse_and_validate_collection(body, format, collection)?;
        let current = self.store.graph_store_read(collection).await?;

        if current.is_empty() {
            if expected_content_etag.is_some() {
                return Ok(ConditionalContentWrite::PreconditionFailed {
                    current_content_etag: None,
                });
            }
            let owned_namespace = format!("{}/", caller_graph.trim_end_matches('/'));
            if !collection.starts_with(&owned_namespace) && !is_admin {
                return Err(MutationError::NotAuthorized(collection.to_owned()));
            }
        } else if !self
            .acl
            .can_write(caller_graph, is_admin, collection, collection)
            .await?
        {
            return Err(MutationError::NotAuthorized(collection.to_owned()));
        }

        let owners = live_owners(&current, collection)
            .into_iter()
            .chain(current.is_empty().then(|| caller_graph.to_owned()))
            .collect::<BTreeSet<_>>();
        let server_managed = management_triples(&incoming, collection, &owners);
        let indexed_subjects = incoming
            .iter()
            .filter_map(|triple| match &triple.subject {
                SubjectTerm::Iri(iri) => Some(DocumentId(iri.as_str().to_owned())),
                SubjectTerm::BlankNode(_) => None,
            })
            .collect::<BTreeSet<_>>();
        let outcome = self
            .store
            .graph_store_write_content_if_match(
                collection,
                incoming,
                server_managed,
                expected_content_etag,
            )
            .await?;

        if matches!(outcome, ConditionalContentWrite::Applied { .. }) {
            if let Some(maintenance) = &self.maintenance {
                maintenance
                    .schedule(IndexMaintenanceEvent::documents(
                        IndexMutationSource::CollectionSync,
                        indexed_subjects,
                    ))
                    .await?;
            }
        }
        Ok(outcome)
    }
}

fn graph_allowed(graph: &str, scope: &GraphScope) -> bool {
    match scope {
        GraphScope::Union => true,
        GraphScope::Only(graphs) => graphs.iter().any(|allowed| allowed == graph),
    }
}

fn parse_and_validate_collection(
    body: &str,
    format: SerializationFormat,
    collection: &str,
) -> Result<Vec<Triple>, DomainError> {
    let rdf_format = to_rdf_format(format)?;
    let validation = match sbol::detect_version(body, rdf_format) {
        Some(sbol::SbolVersion::V2) => sbol::v2::Document::read(body, rdf_format)
            .map_err(|error| DomainError::Parse(error.to_string()))?
            .validate(),
        Some(sbol::SbolVersion::V3) => sbol::v3::Document::read(body, rdf_format)
            .map_err(|error| DomainError::Parse(error.to_string()))?
            .validate(),
        Some(_) | None => {
            return Err(DomainError::InvalidInput(
                "collection synchronization requires an SBOL 2 or SBOL 3 document".to_owned(),
            ))
        }
    };
    if validation.has_errors() {
        return Err(DomainError::Validation(validation.to_string()));
    }

    let graph = sbol_rdf::Graph::parse(body, rdf_format)
        .map_err(|error| DomainError::Parse(error.to_string()))?;
    let mut triples = rdf_graph_to_triples(&graph, &IriString::unchecked(collection));
    for triple in &mut triples {
        triple.graph_iri = None;
    }
    if !triples.iter().any(|triple| {
        matches!(&triple.subject, SubjectTerm::Iri(subject) if subject.as_str() == collection)
            && triple.predicate.as_str() == RDF_TYPE
            && matches!(&triple.object, ObjectTerm::Iri(kind) if matches!(kind.as_str(), SBOL2_COLLECTION | SBOL3_COLLECTION))
    }) {
        return Err(DomainError::InvalidInput(format!(
            "the submitted document does not contain the root Collection {collection}"
        )));
    }

    let namespace = submission_namespace(collection)?;
    if let Some(subject) = triples.iter().find_map(|triple| match &triple.subject {
        SubjectTerm::Iri(subject) if !subject.as_str().starts_with(&namespace) => {
            Some(subject.as_str())
        }
        _ => None,
    }) {
        return Err(DomainError::InvalidInput(format!(
            "subject {subject} is outside the synchronized submission namespace {namespace}"
        )));
    }
    Ok(triples)
}

fn submission_namespace(collection: &str) -> Result<String, DomainError> {
    let (persistent_identity, _) = collection.rsplit_once('/').ok_or_else(|| {
        DomainError::InvalidInput("collection IRI must end in a version segment".to_owned())
    })?;
    let (namespace, _) = persistent_identity.rsplit_once('/').ok_or_else(|| {
        DomainError::InvalidInput("collection IRI must include a submission namespace".to_owned())
    })?;
    Ok(format!("{namespace}/"))
}

fn live_owners(current: &[Triple], collection: &str) -> BTreeSet<String> {
    current
        .iter()
        .filter_map(|triple| {
            if !matches!(&triple.subject, SubjectTerm::Iri(subject) if subject.as_str() == collection)
                || triple.predicate.as_str() != SBH_OWNED_BY
            {
                return None;
            }
            match &triple.object {
                ObjectTerm::Iri(owner) => Some(owner.as_str().to_owned()),
                _ => None,
            }
        })
        .collect()
}

fn management_triples(
    incoming: &[Triple],
    collection: &str,
    owners: &BTreeSet<String>,
) -> Vec<Triple> {
    let subjects = incoming
        .iter()
        .filter_map(|triple| match &triple.subject {
            SubjectTerm::Iri(subject) => Some(subject.as_str().to_owned()),
            SubjectTerm::BlankNode(_) => None,
        })
        .collect::<BTreeSet<_>>();
    let mut top_levels = HashSet::from([collection.to_owned()]);
    for triple in incoming {
        if !matches!(triple.predicate.as_str(), SBOL2_MEMBER | SBOL3_MEMBER) {
            continue;
        }
        if let ObjectTerm::Iri(member) = &triple.object {
            if subjects.contains(member.as_str()) {
                top_levels.insert(member.as_str().to_owned());
            }
        }
    }

    let mut managed = Vec::new();
    for subject in &subjects {
        for owner in owners {
            managed.push(iri_triple(subject, SBH_OWNED_BY, owner));
        }
        let top_level = if top_levels.contains(subject) {
            subject.as_str()
        } else {
            top_levels
                .iter()
                .filter(|candidate| subject.starts_with(&format!("{candidate}/")))
                .max_by_key(|candidate| candidate.len())
                .map(String::as_str)
                .unwrap_or(collection)
        };
        managed.push(iri_triple(subject, SBH_TOP_LEVEL, top_level));
    }
    managed
}

fn iri_triple(subject: &str, predicate: &str, object: &str) -> Triple {
    Triple {
        graph_iri: None,
        subject: SubjectTerm::Iri(IriString::unchecked(subject)),
        predicate: IriString::unchecked(predicate),
        object: ObjectTerm::Iri(IriString::unchecked(object)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_submission_namespace_from_versioned_collection() {
        assert_eq!(
            submission_namespace("https://sbol.io/user/alice/toggle/toggle_collection/1").unwrap(),
            "https://sbol.io/user/alice/toggle/"
        );
    }

    #[test]
    fn stamps_members_as_top_levels_and_nested_subjects_to_their_parent() {
        let root = "https://sbol.io/user/alice/toggle/toggle_collection/1";
        let member = "https://sbol.io/user/alice/toggle/pTet/1";
        let child = "https://sbol.io/user/alice/toggle/pTet/1/location";
        let incoming = vec![
            iri_triple(root, SBOL2_MEMBER, member),
            iri_triple(member, RDF_TYPE, "http://sbols.org/v2#ComponentDefinition"),
            iri_triple(child, RDF_TYPE, "http://sbols.org/v2#Range"),
        ];
        let managed = management_triples(
            &incoming,
            root,
            &BTreeSet::from(["https://sbol.io/user/alice".to_owned()]),
        );
        assert!(managed.iter().any(|triple| {
            matches!(&triple.subject, SubjectTerm::Iri(subject) if subject.as_str() == child)
                && triple.predicate.as_str() == SBH_TOP_LEVEL
                && matches!(&triple.object, ObjectTerm::Iri(target) if target.as_str() == member)
        }));
    }
}
