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

        // The share fact lives on the target user's graph, so the grantee's
        // shared listing (its `sbh:canView` subjects) surfaces the object.
        let share =
            format!("INSERT DATA {{ <{target_user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> }}");
        self.run(&share, target_user_graph).await?;

        // The ownership stamps live on the object's own graph, one per URI in
        // the closure, so the grantee owns every object it reaches.
        let closure = self.acl_service.related_uris(object_uri).await?;
        let stamps = self.owned_by_body(&closure, target_user_graph);
        let stamp = format!("INSERT DATA {{ {stamps} }}");
        self.run(&stamp, &graph).await?;
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

        let share =
            format!("DELETE DATA {{ <{target_user_graph}> <{SBH_CAN_VIEW}> <{object_uri}> }}");
        self.run(&share, target_user_graph).await?;

        let closure = self.acl_service.related_uris(object_uri).await?;
        let stamps = self.owned_by_body(&closure, target_user_graph);
        let stamp = format!("DELETE DATA {{ {stamps} }}");
        self.run(&stamp, &graph).await?;
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

    async fn run(&self, update: &str, graph: &str) -> Result<(), SparqlError> {
        self.sparql_update
            .execute(update, Some(graph), &SparqlOptions::default())
            .await?;
        Ok(())
    }
}
