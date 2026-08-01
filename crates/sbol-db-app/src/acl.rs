//! Caller-identity to graph-scope resolution.
//!
//! [`AclService`] reads the ownership and sharing facts already recorded in the
//! store (`sbh:ownedBy` / `sbh:canView`) and turns a caller into a
//! [`GraphScope`] the SPARQL engine enforces on reads. The store itself stays
//! authorization-free; only this service knows how an identity maps to the set
//! of graphs it may read.

use std::collections::BTreeSet;
use std::sync::Arc;

use sbol_db_core::{DomainError, ObjectTerm};
use sbol_db_sparql::GraphScope;
use sbol_db_storage::{AclStore, SbolStore};

/// The public graph. Its contents are readable by everyone, authenticated or
/// not, so it is always present in a computed scope.
pub const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";

/// `sbh:ownedBy`: the ownership stamp a write-authorization check reads to
/// decide whether a caller's user graph owns a top-level object.
const SBH_OWNED_BY: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy";

/// Turns a caller identity into the [`GraphScope`] its reads are authorized
/// for.
#[derive(Clone)]
pub struct AclService {
    store: Arc<dyn SbolStore>,
    acl: Arc<dyn AclStore>,
    public_graph: String,
}

impl AclService {
    pub fn new(store: Arc<dyn SbolStore>, acl: Arc<dyn AclStore>) -> Self {
        Self {
            store,
            acl,
            public_graph: PUBLIC_GRAPH.to_owned(),
        }
    }

    pub fn with_public_graph(mut self, public_graph: impl Into<String>) -> Self {
        self.public_graph = public_graph.into();
        self
    }

    pub fn public_graph(&self) -> &str {
        &self.public_graph
    }

    /// The graph scope a caller may read.
    ///
    /// For an authenticated caller (`Some(user_graph_iri)`, the identity's RDF
    /// bridge `http://synbiohub.org/user/<username>`) the scope is
    /// [`GraphScope::Only`] of the always-readable public graph, the graphs the
    /// user owns (`sbh:ownedBy`), and the graphs holding the objects shared
    /// with the user (`sbh:canView`).
    ///
    /// For an anonymous caller (`None`) the scope is [`GraphScope::Only`] the
    /// public graph: unauthenticated reads see public data alone, matching the
    /// visibility classic SynBioHub grants an anonymous client. The
    /// Virtuoso-compatible `/sparql` endpoint keeps its own unscoped default and
    /// does not resolve identity through this service.
    pub async fn compute_scope(
        &self,
        user_graph_iri: Option<&str>,
    ) -> Result<GraphScope, DomainError> {
        let Some(user) = user_graph_iri else {
            return Ok(GraphScope::Only(vec![self.public_graph.clone()]));
        };

        let mut graphs: BTreeSet<String> = BTreeSet::new();
        graphs.insert(self.public_graph.clone());

        for owned in self.acl.owned_graphs(user).await? {
            graphs.insert(owned);
        }

        // A shared fact names an object, not its graph; resolve each shared
        // object to the graph that holds it so the scope stays graph-level.
        for object in self.acl.viewable_objects(user).await? {
            if let Some(graph) = self.graph_of_object(&object).await? {
                graphs.insert(graph);
            }
        }

        Ok(GraphScope::Only(graphs.into_iter().collect()))
    }

    /// Whether `user_graph_iri` owns `object_iri`: the object carries an
    /// `<object_iri> sbh:ownedBy <user_graph_iri>` stamp. This is the
    /// write-authorization primitive the mutation verbs gate on; a caller may
    /// only mutate an object its own user graph owns. The store stays
    /// authorization-free, so ownership is read here from the object's own
    /// triples rather than enforced in storage.
    pub async fn owns_object(
        &self,
        user_graph_iri: &str,
        object_iri: &str,
    ) -> Result<bool, DomainError> {
        let triples = self.store.triples_for_subject(object_iri).await?;
        Ok(triples.iter().any(|t| {
            t.predicate.as_str() == SBH_OWNED_BY
                && matches!(&t.object, ObjectTerm::Iri(o) if o.as_str() == user_graph_iri)
        }))
    }

    /// The named graph holding `object_iri`, taken from the object's own
    /// triples. `None` when the subject appears in no named graph (an invalid or
    /// absent object). This is the write path's graph resolver: a mutation
    /// targets the graph that already holds the subject, never the caller's
    /// graph, so the ownership fact travels with the object.
    pub async fn graph_of_subject(&self, object_iri: &str) -> Result<Option<String>, DomainError> {
        let triples = self.store.triples_for_subject(object_iri).await?;
        Ok(triples
            .into_iter()
            .find_map(|t| t.graph_iri.map(|g| g.into_inner())))
    }

    /// The write-authorization decision for a mutation targeting `object_iri`
    /// held in `graph`. An administrator may mutate anything; a subject in the
    /// public graph requires an administrator (mirroring classic SynBioHub's
    /// `edit` gate); otherwise the caller's user graph must own the object
    /// (`sbh:ownedBy`). An anonymous caller has no owning graph and so is never
    /// admitted. The store stays authorization-free: this reads the object's own
    /// triples through [`owns_object`](Self::owns_object).
    pub async fn can_write(
        &self,
        user_graph_iri: &str,
        is_admin: bool,
        object_iri: &str,
        graph: &str,
    ) -> Result<bool, DomainError> {
        if is_admin {
            return Ok(true);
        }
        if graph == self.public_graph {
            return Ok(false);
        }
        self.owns_object(user_graph_iri, object_iri).await
    }

    /// The connected closure of `root` within its submission namespace, mirroring
    /// classic SynBioHub's `retrieveUris` BFS: start at `root`, follow every IRI
    /// object of each reached subject, and keep those under the same submission
    /// namespace (`<prefix>user/<owner>/` or `<prefix>public/`). This bounds the
    /// walk to the object's own submission, excluding shared vocabulary IRIs, so
    /// an ownership stamp lands on the object and everything it owns and nothing
    /// else. `root` is always included.
    pub async fn related_uris(&self, root: &str) -> Result<Vec<String>, DomainError> {
        let namespace = submission_namespace(root);
        let mut resolved = vec![root.to_owned()];
        let mut seen: BTreeSet<String> = BTreeSet::from([root.to_owned()]);
        let mut frontier = vec![root.to_owned()];
        while let Some(subject) = frontier.pop() {
            for triple in self.store.triples_for_subject(&subject).await? {
                if let ObjectTerm::Iri(object) = &triple.object {
                    let object = object.as_str();
                    if object != subject
                        && object.starts_with(&namespace)
                        && seen.insert(object.to_owned())
                    {
                        resolved.push(object.to_owned());
                        frontier.push(object.to_owned());
                    }
                }
            }
        }
        Ok(resolved)
    }

    /// The named graph holding `object_iri`, or `None` when the object is
    /// unknown. Derived object records retain their logical document IRI; a
    /// verbatim submission root may not have a derived record, so fall back to
    /// the authoritative subject triples instead of silently dropping a valid
    /// read-only share from the caller's scope.
    async fn graph_of_object(&self, object_iri: &str) -> Result<Option<String>, DomainError> {
        if let Some(record) = self.store.get_object_by_iri(object_iri).await? {
            if let Some(graph_id) = record.graph_id {
                if let Some(graph) = self.store.get_graph(graph_id).await? {
                    if let Some(document_iri) = graph.document_iri {
                        return Ok(Some(document_iri.into_inner()));
                    }
                }
            }
        }
        self.graph_of_subject(object_iri).await
    }
}

/// The submission namespace an object's closure is bounded to: everything
/// through the owner segment for a user object (`<prefix>user/<owner>/`), or
/// through the `public/` segment for a public object. Falls back to the whole
/// URI when neither pattern matches, which reduces the closure to `root` alone.
fn submission_namespace(uri: &str) -> String {
    if let Some(pos) = uri.find("/user/") {
        let after = pos + "/user/".len();
        if let Some(slash) = uri[after..].find('/') {
            return uri[..after + slash + 1].to_owned();
        }
    }
    if let Some(pos) = uri.find("/public/") {
        return uri[..pos + "/public/".len()].to_owned();
    }
    uri.to_owned()
}
