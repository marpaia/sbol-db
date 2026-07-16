//! App-side recursive and non-recursive SBOL object closure.
//!
//! Classic SynBioHub resolves a top-level SBOL object by crawling the
//! triplestore to a fixpoint: it fetches the object's own triples, discovers
//! every URI those triples reference under the instance's own database prefix,
//! fetches those, and repeats until nothing new appears. [`Downloader`]
//! reproduces that crawl as batched `CONSTRUCT { ?s ?p ?o }` queries over the
//! shared SPARQL engine, scoped to the caller's authorized graphs. It is
//! app-side batching to a fixpoint, not a single SPARQL `DESCRIBE`.
//!
//! [`Downloader::fetch_recursive`] returns the full transitive closure, with a
//! collection fast-path that pulls a Collection plus every member's triples in
//! one `CONSTRUCT` over the `sbh:topLevel` marker.
//! [`Downloader::fetch_non_recursive`] narrows the crawl to the object's own
//! URI prefix and drops `sbol2:member` edges, matching classic's `/sbolnr`.
//!
//! Every read runs under the caller's [`GraphScope`], reusing the same SPARQL
//! engine and authorization ceiling the P2 read routes use; no client-supplied
//! `FROM` is ever accepted.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use sbol_db_core::{DomainError, IriString, ObjectTerm, Triple};
use sbol_db_sparql::{GraphScope, ResultFormat, SparqlEngine, SparqlOptions};

/// The RDF `type` predicate, spelled in full so query strings need no prefix
/// block.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// The SBOL2 `member` predicate: the collection-to-member edge the
/// non-recursive closure drops.
const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";
/// The SBOL2 `Collection` class, which selects the collection fast-path.
const SBOL2_COLLECTION: &str = "http://sbols.org/v2#Collection";
/// The SynBioHub `topLevel` marker linking a top-level object's child triples
/// back to it; the collection fast-path pulls a member's whole graph through
/// this edge.
const SBH_TOP_LEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";
/// SynBioHub terms live under this prefix and are never crawled as data, so a
/// reference into the wiki vocabulary is not followed.
const WIKI_PREFIX: &str = "http://wiki.synbiohub.org/";

/// The instance base IRI objects are minted under by default, matching the V1
/// read routes' `BASE`. Only references under this prefix are followed, so the
/// crawl never chases an external (web-of-registries) URI.
pub const DEFAULT_DATABASE_PREFIX: &str = "http://synbiohub.org/";

/// The number of subject URIs resolved per crawl step. Classic's `resolveBatch`:
/// bounds how many `VALUES ?s { ... }` entries one `CONSTRUCT` carries.
const DEFAULT_RESOLVE_BATCH: usize = 200;
/// The `OFFSET`/`LIMIT` page size that staggers a single `CONSTRUCT` so no one
/// query materializes an unbounded graph. Classic's `staggeredQueryLimit`.
const DEFAULT_STAGGER_LIMIT: usize = 10_000;

/// Crawls the triplestore to the transitive closure of an SBOL object over the
/// shared, ACL-scoped SPARQL engine.
#[derive(Clone)]
pub struct Downloader {
    engine: Arc<SparqlEngine>,
    database_prefix: String,
    resolve_batch: usize,
    stagger_limit: usize,
}

impl Downloader {
    /// Build a downloader over the shared SPARQL engine, following references
    /// under [`DEFAULT_DATABASE_PREFIX`].
    pub fn new(engine: Arc<SparqlEngine>) -> Self {
        Self {
            engine,
            database_prefix: DEFAULT_DATABASE_PREFIX.to_owned(),
            resolve_batch: DEFAULT_RESOLVE_BATCH,
            stagger_limit: DEFAULT_STAGGER_LIMIT,
        }
    }

    /// Follow references under a different database prefix. The instance base
    /// IRI is deployment-specific; a caller wires its configured prefix here.
    pub fn with_database_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.database_prefix = prefix.into();
        self
    }

    /// The transitive closure of `object_iri`: its own triples plus, recursively,
    /// the triples of every object it references under the database prefix.
    ///
    /// A Collection takes the fast-path: one staggered `CONSTRUCT` gathers the
    /// Collection's own triples and every member's triples (reached through the
    /// `sbh:topLevel` marker), which for a large Collection is far cheaper than
    /// crawling each member. Every other type is crawled to a fixpoint.
    pub async fn fetch_recursive(
        &self,
        object_iri: &str,
        scope: GraphScope,
    ) -> Result<Vec<Triple>, DomainError> {
        if self.is_collection(object_iri, &scope).await? {
            let where_body = collection_where(object_iri);
            return self.construct_paged(&where_body, &scope).await;
        }
        self.crawl(object_iri, &scope, None, false).await
    }

    /// The non-recursive closure of `object_iri`: the crawl is narrowed to the
    /// object's own URI prefix and `sbol2:member` edges are dropped, so a
    /// sibling reachable only through the collection's membership is not pulled
    /// in. Matches classic's `/sbolnr`.
    pub async fn fetch_non_recursive(
        &self,
        object_iri: &str,
        scope: GraphScope,
    ) -> Result<Vec<Triple>, DomainError> {
        let prefix = object_prefix(object_iri);
        self.crawl(object_iri, &scope, Some(&prefix), true).await
    }

    /// Whether `root` is an SBOL2 Collection, probed with a `CONSTRUCT` that
    /// yields one triple iff the type assertion exists (so no SELECT-JSON
    /// parsing is needed).
    async fn is_collection(&self, root: &str, scope: &GraphScope) -> Result<bool, DomainError> {
        let query = format!(
            "CONSTRUCT {{ <{root}> <{RDF_TYPE}> <{SBOL2_COLLECTION}> }} \
             WHERE {{ <{root}> <{RDF_TYPE}> <{SBOL2_COLLECTION}> }}"
        );
        Ok(!self.construct(&query, scope).await?.is_empty())
    }

    /// The fixpoint crawl: resolve the root, discover the referenced URIs its
    /// triples introduce, resolve those, and repeat until nothing new appears.
    /// `prefix_filter` (the non-recursive narrowing) and `exclude_member` (drop
    /// `sbol2:member` edges) restrict which references are followed.
    async fn crawl(
        &self,
        root: &str,
        scope: &GraphScope,
        prefix_filter: Option<&str>,
        exclude_member: bool,
    ) -> Result<Vec<Triple>, DomainError> {
        let mut closure: Vec<Triple> = Vec::new();
        // Subjects already resolved or queued, so no URI is fetched twice.
        let mut seen: HashSet<String> = HashSet::new();
        let mut frontier: VecDeque<String> = VecDeque::new();
        seen.insert(root.to_owned());
        frontier.push_back(root.to_owned());

        while !frontier.is_empty() {
            let take = self.resolve_batch.min(frontier.len());
            let batch: Vec<String> = frontier.drain(0..take).collect();
            let where_body = subjects_where(&batch, exclude_member);
            let page = self.construct_paged(&where_body, scope).await?;
            for triple in &page {
                if let ObjectTerm::Iri(iri) = &triple.object {
                    let referenced = iri.as_str();
                    if self.should_resolve(referenced, prefix_filter)
                        && seen.insert(referenced.to_owned())
                    {
                        frontier.push_back(referenced.to_owned());
                    }
                }
            }
            closure.extend(page);
        }
        Ok(closure)
    }

    /// Whether a referenced URI should be crawled: it must live under the
    /// database prefix, not be a SynBioHub-terms URI, and — for the
    /// non-recursive closure — start with the object's own prefix.
    fn should_resolve(&self, uri: &str, prefix_filter: Option<&str>) -> bool {
        if !uri.starts_with(&self.database_prefix) || uri.starts_with(WIKI_PREFIX) {
            return false;
        }
        match prefix_filter {
            Some(prefix) => uri.starts_with(prefix),
            None => true,
        }
    }

    /// Run one `CONSTRUCT` body staggered across `OFFSET`/`LIMIT` pages so a
    /// single query never materializes an unbounded graph. `DISTINCT` in the
    /// inner select makes paging stable and a short page (fewer than the limit)
    /// a reliable end-of-results signal.
    async fn construct_paged(
        &self,
        where_body: &str,
        scope: &GraphScope,
    ) -> Result<Vec<Triple>, DomainError> {
        let mut all = Vec::new();
        let mut offset = 0usize;
        loop {
            let query = format!(
                "CONSTRUCT {{ ?s ?p ?o }} WHERE {{ {{ \
                 SELECT DISTINCT ?s ?p ?o WHERE {{ {where_body} }} \
                 ORDER BY ?s ?p ?o OFFSET {offset} LIMIT {limit} }} }}",
                limit = self.stagger_limit
            );
            let page = self.construct(&query, scope).await?;
            let page_len = page.len();
            all.extend(page);
            if page_len < self.stagger_limit {
                break;
            }
            offset += self.stagger_limit;
        }
        Ok(all)
    }

    /// Execute a `CONSTRUCT` under the caller's scope and parse its N-Triples
    /// output into domain triples. The graph tag is dropped: the closure is a
    /// bare set of `?s ?p ?o` facts, not a per-graph view.
    async fn construct(&self, query: &str, scope: &GraphScope) -> Result<Vec<Triple>, DomainError> {
        let options = SparqlOptions {
            authorized_graphs: scope.clone(),
            ..SparqlOptions::default()
        };
        let outcome = self
            .engine
            .execute(query, Some(ResultFormat::NTriples), None, &options)
            .await?;
        let body = String::from_utf8(outcome.payload.body)
            .map_err(|e| DomainError::Parse(e.to_string()))?;
        if body.trim().is_empty() {
            return Ok(Vec::new());
        }
        let graph = sbol_rdf::Graph::parse(&body, sbol_rdf::RdfFormat::NTriples)
            .map_err(|e| DomainError::Parse(e.to_string()))?;
        let placeholder = IriString::unchecked("");
        let mut triples = sbol_db_rdf::rdf_graph_to_triples(&graph, &placeholder);
        for triple in &mut triples {
            triple.graph_iri = None;
        }
        Ok(triples)
    }
}

/// The `WHERE` body that selects the triples of a batch of subjects, optionally
/// dropping `sbol2:member` edges (the non-recursive narrowing).
fn subjects_where(subjects: &[String], exclude_member: bool) -> String {
    let values = subjects
        .iter()
        .map(|s| format!("<{s}>"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut body = format!("VALUES ?s {{ {values} }} ?s ?p ?o .");
    if exclude_member {
        body.push_str(&format!(" FILTER(?p != <{SBOL2_MEMBER}>)"));
    }
    body
}

/// The collection fast-path `WHERE` body: the Collection's own triples, plus
/// every triple of every object whose `sbh:topLevel` is one of the Collection's
/// members.
fn collection_where(root: &str) -> String {
    format!(
        "{{ ?s ?p ?o . FILTER(?s = <{root}>) }} UNION \
         {{ <{root}> <{SBOL2_MEMBER}> ?topLevel . \
         ?s <{SBH_TOP_LEVEL}> ?topLevel . ?s ?p ?o . }}"
    )
}

/// The prefix of an object's URI: everything up to and including the last `/`.
/// The non-recursive closure follows only references under this prefix.
fn object_prefix(uri: &str) -> String {
    match uri.rfind('/') {
        Some(idx) => uri[..=idx].to_owned(),
        None => uri.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sbol_db_backend::Backend;
    use sbol_db_core::{ObjectTerm, Triple};
    use sbol_db_sparql::{GraphScope, SparqlEngine, SparqlOptions, SparqlUpdateEngine};
    use tempfile::TempDir;

    use super::*;

    const PREFIX: &str = "http://example.org/";
    const GRAPH: &str = "http://example.org/graph";
    const ROOT: &str = "http://example.org/cd/root";
    const SUBCOMP: &str = "http://example.org/cd/subcomp";
    const SIBLING: &str = "http://example.org/cd/sibling";

    /// A ComponentDefinition `root` that references `anno`, which references the
    /// transitively-reachable `subcomp`; `root` also `member`s a `sibling`
    /// reachable only through that membership edge. Every object is under the
    /// same prefix, so only the edge kind (member vs not) distinguishes them.
    const FIXTURE: &str = "\
INSERT DATA {
  <http://example.org/cd/root> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#ComponentDefinition> .
  <http://example.org/cd/root> <http://sbols.org/v2#sequenceAnnotation> <http://example.org/cd/root/anno> .
  <http://example.org/cd/root> <http://sbols.org/v2#member> <http://example.org/cd/sibling> .
  <http://example.org/cd/root/anno> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#SequenceAnnotation> .
  <http://example.org/cd/root/anno> <http://sbols.org/v2#component> <http://example.org/cd/subcomp> .
  <http://example.org/cd/subcomp> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#Component> .
  <http://example.org/cd/subcomp> <http://purl.org/dc/terms/title> \"deep child\" .
  <http://example.org/cd/sibling> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#ComponentDefinition> .
  <http://example.org/cd/sibling> <http://purl.org/dc/terms/title> \"sibling only via member\" .
}";

    /// Open a fresh SQLite backend, run its migrations, and seed the fixture via
    /// `INSERT DATA`. Returns a downloader over the read engine and the
    /// `TempDir` owning the database file.
    async fn seeded_downloader() -> (Downloader, TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("download.db");
        let url = format!("sqlite://{}", path.display());
        let backend = Backend::open(&url).await.expect("open sqlite backend");
        backend
            .migrator
            .as_ref()
            .expect("sqlite backend has a migrator")
            .run_migrations()
            .await
            .expect("run migrations");

        let update =
            SparqlUpdateEngine::new(backend.triple_source.clone(), backend.triple_writer.clone());
        update
            .execute(FIXTURE, Some(GRAPH), &SparqlOptions::default())
            .await
            .expect("seed fixture");

        let engine = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
        let downloader = Downloader::new(engine).with_database_prefix(PREFIX);
        (downloader, dir)
    }

    /// Whether the closure holds a triple with the given subject, predicate, and
    /// literal object.
    fn has_literal(triples: &[Triple], subject: &str, predicate: &str, value: &str) -> bool {
        triples.iter().any(|t| {
            matches!(&t.subject, sbol_db_core::SubjectTerm::Iri(iri) if iri.as_str() == subject)
                && t.predicate.as_str() == predicate
                && matches!(&t.object, ObjectTerm::Literal { value: v, .. } if v == value)
        })
    }

    const TITLE: &str = "http://purl.org/dc/terms/title";

    #[tokio::test]
    async fn recursive_includes_transitively_referenced_child() {
        let (downloader, _dir) = seeded_downloader().await;
        let closure = downloader
            .fetch_recursive(ROOT, GraphScope::Union)
            .await
            .expect("recursive closure");

        // `subcomp` is two references deep (root -> anno -> subcomp); the
        // transitive crawl must have reached it.
        assert!(
            has_literal(&closure, SUBCOMP, TITLE, "deep child"),
            "recursive closure should include the transitively-referenced child: {closure:?}"
        );
        // The recursive crawl follows the member edge, so the sibling is present.
        assert!(
            has_literal(&closure, SIBLING, TITLE, "sibling only via member"),
            "recursive closure should follow the member edge to the sibling"
        );
    }

    #[tokio::test]
    async fn non_recursive_excludes_sibling_reached_only_via_member() {
        let (downloader, _dir) = seeded_downloader().await;
        let closure = downloader
            .fetch_non_recursive(ROOT, GraphScope::Union)
            .await
            .expect("non-recursive closure");

        // Dropping member edges means the sibling is never discovered, even
        // though it shares the object's prefix.
        assert!(
            !has_literal(&closure, SIBLING, TITLE, "sibling only via member"),
            "non-recursive closure must exclude a sibling reachable only via member: {closure:?}"
        );
        // Non-member children under the object's prefix are still included.
        assert!(
            has_literal(&closure, SUBCOMP, TITLE, "deep child"),
            "non-recursive closure should still include children under the object's prefix"
        );
        // The dropped edge itself is absent from the closure.
        assert!(
            !closure.iter().any(|t| t.predicate.as_str() == SBOL2_MEMBER),
            "non-recursive closure must not contain member edges"
        );
    }
}
