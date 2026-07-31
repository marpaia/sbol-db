//! Object-sharing verbs: grant and revoke another user's access.
//!
//! [`PermissionService`] ports classic SynBioHub's `addOwnedBy`/`removeOwnedBy`
//! actions. Granting access to an object does two things: it records
//! `<targetUserGraph> sbh:canView <object>` (so the object surfaces in the
//! grantee's shared listing), and it stamps `<uri> sbh:ownedBy <targetUserGraph>`
//! across the object's closure (so the grantee owns and may read every URI the
//! object reaches). Revoking reverses both.
//!
//! The caller must already own the object; an anonymous or non-owning caller is
//! rejected with [`MutationError::NotAuthorized`]. The store stays
//! authorization-free: ownership and the closure are read from the object's own
//! triples through the [`AclService`].

use std::sync::Arc;

use sbol_db_sparql::{SparqlError, SparqlOptions, SparqlUpdateEngine};

use crate::acl::AclService;
use crate::audit::{event_triples, AuditAction};
use crate::mutation::MutationError;

const SBH_OWNED_BY: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy";
const SBH_CAN_VIEW: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#canView";

/// The grant/revoke sharing verbs, gated on caller ownership.
#[derive(Clone)]
pub struct PermissionService {
    sparql_update: Arc<SparqlUpdateEngine>,
    acl_service: AclService,
}

impl PermissionService {
    pub fn new(sparql_update: Arc<SparqlUpdateEngine>, acl_service: AclService) -> Self {
        Self {
            sparql_update,
            acl_service,
        }
    }

    /// Grant `target_user_graph` view access to `object_uri`: record the
    /// `sbh:canView` share on the target's graph and stamp `sbh:ownedBy
    /// <target_user_graph>` across the object's closure. Mirrors classic
    /// `addOwnedBy`.
    pub async fn add_owner(
        &self,
        user_graph: &str,
        is_admin: bool,
        object_uri: &str,
        target_user_graph: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, object_uri).await?;

        // The share fact lives on the target user's graph while ownership
        // stamps live on the object's graph. One explicit-graph update applies
        // both partitions through a single atomic TripleWriter batch.
        let closure = self.acl_service.related_uris(object_uri).await?;
        let stamps = self.owned_by_body(&closure, target_user_graph);
        let update = format!(
            "INSERT DATA {{\n\
             GRAPH <{target_user_graph}> {{ <{target_user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> . }}\n\
             GRAPH <{graph}> {{ {stamps} }}\n\
             }}"
        );
        self.run_global(&update).await?;
        Ok(())
    }

    /// Grant read-only access without adding an ownership stamp. This is the
    /// native collaboration contract; classic `addOwner` continues to call
    /// [`Self::add_owner`] for wire-compatible co-ownership semantics.
    pub async fn grant_view(
        &self,
        user_graph: &str,
        is_admin: bool,
        object_uri: &str,
        target_user_graph: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, object_uri).await?;
        let event = event_triples(
            object_uri,
            AuditAction::ShareGranted,
            user_graph,
            Some(target_user_graph),
            None,
        )?;
        let update = format!(
            "INSERT DATA {{\n\
             GRAPH <{target_user_graph}> {{ <{target_user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> . }}\n\
             GRAPH <{graph}> {{ {event} }}\n\
             }}"
        );
        self.run_global(&update).await?;
        Ok(())
    }

    /// Revoke a native read-only share. Ownership facts are deliberately left
    /// untouched; a co-owner must be removed through the compatibility command
    /// or an explicit ownership transfer.
    pub async fn revoke_view(
        &self,
        user_graph: &str,
        is_admin: bool,
        object_uri: &str,
        target_user_graph: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, object_uri).await?;
        let event = event_triples(
            object_uri,
            AuditAction::ShareRevoked,
            user_graph,
            Some(target_user_graph),
            None,
        )?;
        let update = format!(
            "DELETE DATA {{ GRAPH <{target_user_graph}> {{ <{target_user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> . }} }} ;\n\
             INSERT DATA {{ GRAPH <{graph}> {{ {event} }} }}"
        );
        self.run_global(&update).await?;
        Ok(())
    }

    /// Revoke `target_user_graph`'s view access to `object_uri`: drop the
    /// `sbh:canView` share and the closure's `sbh:ownedBy` stamps. Mirrors
    /// classic `removeOwnedBy`.
    pub async fn remove_owner(
        &self,
        user_graph: &str,
        is_admin: bool,
        object_uri: &str,
        target_user_graph: &str,
    ) -> Result<(), MutationError> {
        let graph = self.authorize(user_graph, is_admin, object_uri).await?;

        let closure = self.acl_service.related_uris(object_uri).await?;
        let stamps = self.owned_by_body(&closure, target_user_graph);
        let update = format!(
            "DELETE DATA {{\n\
             GRAPH <{target_user_graph}> {{ <{target_user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> . }}\n\
             GRAPH <{graph}> {{ {stamps} }}\n\
             }}"
        );
        self.run_global(&update).await?;
        Ok(())
    }

    /// Transfer the caller's ownership of `object_uri` to another account in
    /// one storage transaction. The target gains the share index and closure
    /// ownership stamps while the caller loses both. Other co-owners are left
    /// unchanged.
    pub async fn transfer_owner(
        &self,
        user_graph: &str,
        _is_admin: bool,
        object_uri: &str,
        target_user_graph: &str,
    ) -> Result<(), MutationError> {
        if !self.acl_service.owns_object(user_graph, object_uri).await? {
            return Err(MutationError::NotAuthorized(object_uri.to_owned()));
        }
        let graph = self
            .acl_service
            .graph_of_subject(object_uri)
            .await?
            .ok_or_else(|| MutationError::NotFound(object_uri.to_owned()))?;
        if user_graph == target_user_graph {
            return Err(MutationError::Domain(
                sbol_db_core::DomainError::InvalidInput(
                    "the new owner must be a different account".to_owned(),
                ),
            ));
        }
        let closure = self.acl_service.related_uris(object_uri).await?;
        let old_stamps = self.owned_by_body(&closure, user_graph);
        let new_stamps = self.owned_by_body(&closure, target_user_graph);
        let event = event_triples(
            object_uri,
            AuditAction::OwnershipTransferred,
            user_graph,
            Some(target_user_graph),
            None,
        )?;
        let update = format!(
            "DELETE DATA {{\n\
             GRAPH <{user_graph}> {{ <{user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> . }}\n\
             GRAPH <{graph}> {{ {old_stamps} }}\n\
             }} ;\n\
             INSERT DATA {{\n\
             GRAPH <{target_user_graph}> {{ <{target_user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> . }}\n\
             GRAPH <{graph}> {{ {new_stamps}\n{event} }}\n\
             }}"
        );
        self.run_global(&update).await?;
        Ok(())
    }

    /// The `<uri> sbh:ownedBy <target>` triples of a closure, one per line, for
    /// an `INSERT DATA`/`DELETE DATA` body.
    fn owned_by_body(&self, closure: &[String], target_user_graph: &str) -> String {
        closure
            .iter()
            .map(|uri| format!("<{uri}> <{SBH_OWNED_BY}> <{target_user_graph}> ."))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Resolve the graph holding `uri` and enforce the write gate: the caller
    /// must own the object (or be an administrator).
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

    async fn run_global(&self, update: &str) -> Result<(), SparqlError> {
        self.sparql_update
            .execute(update, None, &SparqlOptions::default())
            .await?;
        Ok(())
    }
}
