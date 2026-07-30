//! Field-edit and membership verbs over the SPARQL Update engine.
//!
//! [`EditService`] ports classic SynBioHub's in-place field mutations: the
//! mutable text fields (`sbh:mutableDescription`, `sbh:mutableNotes`,
//! `sbh:mutableProvenance`), the citation list (`obo:OBI_0001617`), the generic
//! `edit`/`add`/`remove` of a field predicate, and Collection membership
//! (`sbol:member`). Every field mutation refreshes `dcterms:modified` in the
//! same atomic update, matching the classic `UpdateTriple`/`AddTriple`/
//! `RemoveTriple` templates.
//!
//! Every verb is identity-gated through the [`AclService`]: a caller may mutate
//! only an object its own user graph owns, an administrator may mutate anything,
//! and a public-graph subject requires an administrator. An anonymous or
//! non-owning caller is rejected with [`MutationError::NotAuthorized`]. The
//! store stays authorization-free; the ownership fact is read from the object's
//! own triples.

use std::sync::Arc;

use sbol_db_search_sdk::{DocumentId, IndexMaintenanceEvent, IndexMutationSource};
use sbol_db_sparql::{SparqlError, SparqlOptions, SparqlUpdateEngine};
use sbol_db_storage::SbolStore;

use crate::acl::AclService;
use crate::mutation::MutationError;
use crate::SearchMaintenanceScheduler;

/// SynBioHub / SBOL vocabulary the field-edit templates write.
const DCTERMS_MODIFIED: &str = "http://purl.org/dc/terms/modified";
const SBH_MUTABLE_DESCRIPTION: &str =
    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableDescription";
const SBH_MUTABLE_NOTES: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableNotes";
const SBH_MUTABLE_PROVENANCE: &str =
    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableProvenance";
const OBI_CITATION: &str = "http://purl.obolibrary.org/obo/OBI_0001617";
const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";

/// An RDF term a field edit writes: an IRI reference or a plain string literal.
/// The adapter chooses which by the field's predicate (a `role`/`type`/
/// `wasDerivedFrom`/annotation value is an IRI; `title`/`description` are
/// literals), mirroring classic SynBioHub's `formatObject`.
#[derive(Clone, Debug)]
pub enum FieldValue {
    /// An IRI reference, serialized `<iri>`.
    Iri(String),
    /// A plain string literal, serialized `"value"` with escaping.
    Literal(String),
}

impl FieldValue {
    fn to_sparql(&self) -> String {
        match self {
            FieldValue::Iri(iri) => format!("<{iri}>"),
            FieldValue::Literal(value) => format!("\"{}\"", escape_literal(value)),
        }
    }
}

/// The field-edit and membership verbs, gated on caller ownership.
#[derive(Clone)]
pub struct EditService {
    sparql_update: Arc<SparqlUpdateEngine>,
    acl_service: AclService,
    maintenance: Option<Arc<SearchMaintenanceScheduler>>,
}

impl EditService {
    /// Build the service over the SPARQL Update engine and the ACL service. The
    /// store handle is unused directly (graph resolution and ownership go
    /// through the [`AclService`]), so it is not held.
    pub fn new(
        _store: Arc<dyn SbolStore>,
        sparql_update: Arc<SparqlUpdateEngine>,
        acl_service: AclService,
    ) -> Self {
        Self {
            sparql_update,
            acl_service,
            maintenance: None,
        }
    }

    /// Attach automatic search maintenance to this edit service.
    pub fn with_maintenance(mut self, maintenance: Arc<SearchMaintenanceScheduler>) -> Self {
        self.maintenance = Some(maintenance);
        self
    }

    /// Replace `sbh:mutableDescription` on `uri` with `value` (dropping the field
    /// when `value` is empty) and refresh `dcterms:modified`.
    pub async fn update_mutable_description(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        value: &str,
    ) -> Result<(), MutationError> {
        self.update_mutable_field(user_graph, is_admin, uri, SBH_MUTABLE_DESCRIPTION, value)
            .await
    }

    /// Replace `sbh:mutableNotes` on `uri` and refresh `dcterms:modified`.
    pub async fn update_mutable_notes(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        value: &str,
    ) -> Result<(), MutationError> {
        self.update_mutable_field(user_graph, is_admin, uri, SBH_MUTABLE_NOTES, value)
            .await
    }

    /// Replace `sbh:mutableProvenance` on `uri` and refresh `dcterms:modified`.
    /// This is classic's `updateMutableSource`, which writes `mutableProvenance`
    /// (not `sbol:source`).
    pub async fn update_mutable_source(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        value: &str,
    ) -> Result<(), MutationError> {
        self.update_mutable_field(user_graph, is_admin, uri, SBH_MUTABLE_PROVENANCE, value)
            .await
    }

    /// Replace the citation list (`obo:OBI_0001617`) on `uri` with `citations`
    /// (each a PubMed id) and refresh `dcterms:modified`.
    pub async fn update_citations(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        citations: &[String],
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, uri).await?;
        let mut inserts = String::new();
        for citation in citations {
            inserts.push_str(&format!(
                "<{uri}> <{OBI_CITATION}> \"{}\" .\n",
                escape_literal(citation)
            ));
        }
        let update = format!(
            "DELETE WHERE {{ <{uri}> <{OBI_CITATION}> ?citation }} ;\n\
             DELETE WHERE {{ <{uri}> <{DCTERMS_MODIFIED}> ?modified }} ;\n\
             INSERT DATA {{ {inserts}<{uri}> <{DCTERMS_MODIFIED}> \"{}\" }}",
            now_modified()
        );
        self.run(&update, &graph).await?;
        self.schedule_document(uri).await?;
        Ok(())
    }

    /// Edit a field: replace `<uri> <predicate> ?` with `value` and refresh
    /// `dcterms:modified`. When `previous` is `Some`, only a matching current
    /// value is replaced; when `None`, whatever value the predicate currently
    /// holds is replaced (the `title`/`description` case). Mirrors classic
    /// `UpdateTriple`.
    pub async fn edit_field(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        predicate: &str,
        value: &FieldValue,
        previous: Option<&FieldValue>,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, uri).await?;
        let previous = match previous {
            Some(term) => term.to_sparql(),
            None => "?previous".to_owned(),
        };
        let update = format!(
            "DELETE {{ <{uri}> <{predicate}> {previous} . <{uri}> <{DCTERMS_MODIFIED}> ?mod }}\n\
             INSERT {{ <{uri}> <{predicate}> {value} . <{uri}> <{DCTERMS_MODIFIED}> \"{modified}\" }}\n\
             WHERE {{ OPTIONAL {{ <{uri}> <{predicate}> {previous} . <{uri}> <{DCTERMS_MODIFIED}> ?mod }} }}",
            value = value.to_sparql(),
            modified = now_modified(),
        );
        self.run(&update, &graph).await?;
        self.schedule_document(uri).await?;
        Ok(())
    }

    /// Add a field value: insert `<uri> <predicate> value` and refresh
    /// `dcterms:modified`. Mirrors classic `AddTriple`.
    pub async fn add_field(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        predicate: &str,
        value: &FieldValue,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, uri).await?;
        let update = format!(
            "DELETE {{ <{uri}> <{DCTERMS_MODIFIED}> ?mod }}\n\
             INSERT {{ <{uri}> <{predicate}> {value} . <{uri}> <{DCTERMS_MODIFIED}> \"{modified}\" }}\n\
             WHERE {{ OPTIONAL {{ <{uri}> <{DCTERMS_MODIFIED}> ?mod }} }}",
            value = value.to_sparql(),
            modified = now_modified(),
        );
        self.run(&update, &graph).await?;
        self.schedule_document(uri).await?;
        Ok(())
    }

    /// Remove a field value: delete `<uri> <predicate> value` and refresh
    /// `dcterms:modified`. Mirrors classic `RemoveTriple`.
    pub async fn remove_field(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        predicate: &str,
        value: &FieldValue,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, uri).await?;
        let update = format!(
            "DELETE {{ <{uri}> <{predicate}> {value} . <{uri}> <{DCTERMS_MODIFIED}> ?mod }}\n\
             INSERT {{ <{uri}> <{DCTERMS_MODIFIED}> \"{modified}\" }}\n\
             WHERE {{ <{uri}> <{predicate}> {value} . OPTIONAL {{ <{uri}> <{DCTERMS_MODIFIED}> ?mod }} }}",
            value = value.to_sparql(),
            modified = now_modified(),
        );
        self.run(&update, &graph).await?;
        self.schedule_document(uri).await?;
        Ok(())
    }

    /// Add `member_uri` to `collection_uri` (`sbol:member`). The caller must own
    /// the Collection. Mirrors classic `addMembership.sparql`.
    pub async fn add_to_collection(
        &self,
        user_graph: &str,
        is_admin: bool,
        collection_uri: &str,
        member_uri: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, collection_uri).await?;
        let update =
            format!("INSERT DATA {{ <{collection_uri}> <{SBOL2_MEMBER}> <{member_uri}> }}");
        self.run(&update, &graph).await?;
        self.schedule_document(collection_uri).await?;
        Ok(())
    }

    /// Remove `member_uri` from `collection_uri` (`sbol:member`). The caller must
    /// own the Collection. Mirrors classic `removeMembership.sparql`.
    pub async fn remove_membership(
        &self,
        user_graph: &str,
        is_admin: bool,
        collection_uri: &str,
        member_uri: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, collection_uri).await?;
        let update =
            format!("DELETE WHERE {{ <{collection_uri}> <{SBOL2_MEMBER}> <{member_uri}> }}");
        self.run(&update, &graph).await?;
        self.schedule_document(collection_uri).await?;
        Ok(())
    }

    /// Build and run a mutable-field replacement: drop the field, drop the old
    /// `dcterms:modified`, then insert the new value (when non-empty) and a fresh
    /// `dcterms:modified`. Mirrors the classic `UpdateMutable*` templates.
    async fn update_mutable_field(
        &self,
        user_graph: &str,
        is_admin: bool,
        uri: &str,
        predicate: &str,
        value: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, uri).await?;
        let field_insert = if value.trim().is_empty() {
            String::new()
        } else {
            format!("<{uri}> <{predicate}> \"{}\" .\n", escape_literal(value))
        };
        let update = format!(
            "DELETE WHERE {{ <{uri}> <{predicate}> ?value }} ;\n\
             DELETE WHERE {{ <{uri}> <{DCTERMS_MODIFIED}> ?modified }} ;\n\
             INSERT DATA {{ {field_insert}<{uri}> <{DCTERMS_MODIFIED}> \"{}\" }}",
            now_modified()
        );
        self.run(&update, &graph).await?;
        self.schedule_document(uri).await?;
        Ok(())
    }

    /// Resolve the graph holding `uri` and enforce the write gate.
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

    /// Execute one SPARQL Update scoped to `graph` (the `default-graph-uri` the
    /// template operates over, matching Virtuoso semantics).
    async fn run(&self, update: &str, graph: &str) -> Result<(), SparqlError> {
        self.sparql_update
            .execute(update, Some(graph), &SparqlOptions::default())
            .await?;
        Ok(())
    }

    async fn schedule_document(&self, uri: &str) -> Result<(), MutationError> {
        if let Some(maintenance) = &self.maintenance {
            maintenance
                .schedule(IndexMaintenanceEvent::documents(
                    IndexMutationSource::ObjectEdit,
                    [DocumentId(uri.to_owned())],
                ))
                .await?;
        }
        Ok(())
    }
}

/// ISO-8601 UTC timestamp without fractional seconds, matching classic
/// SynBioHub's `dcterms:modified` stamp.
fn now_modified() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string()
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
