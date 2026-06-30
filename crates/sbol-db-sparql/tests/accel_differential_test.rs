//! Differential validation of the SynBioHub query accelerator: for every
//! recognized template, the accelerator must return exactly what the generic
//! SPARQL engine returns. The generic path is the ground truth (it is itself
//! validated against Virtuoso), so accelerator == generic proves the
//! purpose-built indexes answer faithfully.
//!
//! The same templates run against both SQL backends (SQLite always; Postgres
//! when a database is reachable), so the per-backend serving SQL — Postgres'
//! `UNNEST`/collation and SQLite's chunked inserts/`substr` prefix match — is
//! exercised against the shared derivation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sbol_db_core::{DomainError, Triple};
use sbol_db_sparql::{ResultFormat, SparqlEngine, SparqlOptions};
use sbol_db_storage::{
    AccelSolutions, AcceleratedQuery, GraphFilter, GraphWriteMode, IdGraphFilter, IdQuad,
    PatternObject, PatternSubject, SbolStore, TermId, TermKey, TermValue, TripleSource,
};

const SBOL2: &str = include_str!("synbiohub_sbol2.ttl");
const PUBLIC_GRAPH: &str = "https://synbiohub.org/public";
const COLLECTION: &str = "https://synbiohub.org/public/igem/igem_collection/1";
const MEMBER_PREFIX: &str = "https://synbiohub.org/public/igem/";
/// A top-level object with both a title and a description.
const META_FULL: &str = "https://synbiohub.org/public/igem/BBa_J23100/1";
/// A top-level object with a title but no description (the optional stays unbound).
const META_NO_DESC: &str = "https://synbiohub.org/public/igem/BBa_B0034/1";
/// An object with no title (the required pattern fails: zero rows).
const META_NO_TITLE: &str = "https://synbiohub.org/public/igem/BBa_J23100_sequence/1";
/// An IRI absent from the graph (no metadata record: zero rows).
const META_MISSING: &str = "https://synbiohub.org/public/igem/Nonexistent/1";

/// A `TripleSource` decorator that forces the generic engine by declining every
/// accelerated query, while delegating all real reads to the inner source.
struct NoAccel(Arc<dyn TripleSource>);

/// A `TripleSource` decorator that counts how many queries the inner source
/// actually answered from the accelerator, so a test can assert the accelerator
/// was engaged (not silently bypassed).
struct Probe {
    inner: Arc<dyn TripleSource>,
    hits: Arc<AtomicUsize>,
}

macro_rules! delegate_reads {
    ($field:tt) => {
        fn scan_pattern(
            &self,
            subject: Option<&PatternSubject>,
            predicate: Option<&str>,
            object: Option<&PatternObject>,
            graph: Option<&GraphFilter>,
            limit: i64,
        ) -> Result<Vec<Triple>, DomainError> {
            self.$field
                .scan_pattern(subject, predicate, object, graph, limit)
        }
        fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError> {
            self.$field.distinct_named_graphs()
        }
        fn triples_for_graph(
            &self,
            graph: Option<&str>,
            limit: i64,
        ) -> Result<Vec<Triple>, DomainError> {
            self.$field.triples_for_graph(graph, limit)
        }
        fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
            self.$field.triples_for_subject(subject_iri)
        }
        fn supports_id_scan(&self) -> bool {
            self.$field.supports_id_scan()
        }
        fn id_scan(
            &self,
            subject: Option<TermId>,
            predicate: Option<TermId>,
            object: Option<TermId>,
            graph: &IdGraphFilter,
            limit: i64,
        ) -> Result<Vec<IdQuad>, DomainError> {
            self.$field
                .id_scan(subject, predicate, object, graph, limit)
        }
        fn term_to_id(&self, key: TermKey<'_>) -> Result<TermId, DomainError> {
            self.$field.term_to_id(key)
        }
        fn id_to_term(&self, id: TermId) -> Result<TermValue, DomainError> {
            self.$field.id_to_term(id)
        }
    };
}

impl TripleSource for NoAccel {
    delegate_reads!(0);
    fn run_accelerated(
        &self,
        _query: &AcceleratedQuery,
    ) -> Result<Option<AccelSolutions>, DomainError> {
        Ok(None)
    }
}

impl TripleSource for Probe {
    delegate_reads!(inner);
    fn run_accelerated(
        &self,
        query: &AcceleratedQuery,
    ) -> Result<Option<AccelSolutions>, DomainError> {
        let result = self.inner.run_accelerated(query)?;
        if result.is_some() {
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
        Ok(result)
    }
}

fn options() -> SparqlOptions {
    SparqlOptions {
        timeout: Duration::from_secs(30),
        max_rows: 100_000,
        max_query_size: 64 * 1024,
        default_graph: Some(PUBLIC_GRAPH.to_owned()),
    }
}

/// The bindings of a SELECT result as a sorted multiset, so two results compare
/// equal regardless of row order (the templates have no total order).
fn binding_multiset(body: &str) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(body).expect("result json");
    let mut rows: Vec<String> = json["results"]["bindings"]
        .as_array()
        .expect("bindings array")
        .iter()
        .map(|b| serde_json::to_string(b).expect("binding json"))
        .collect();
    rows.sort();
    rows
}

/// Every recognized SynBioHub template, as `(name, query)`. The `sbolType`/`role`
/// columns carry their BioPAX/Sequence-Ontology `STRSTARTS` filters so the
/// generic engine narrows them the same way the accelerator does.
fn templates() -> Vec<(&'static str, String)> {
    let prefixes = "\
        PREFIX sbol2: <http://sbols.org/v2#>\n\
        PREFIX dcterms: <http://purl.org/dc/terms/>\n\
        PREFIX dc: <http://purl.org/dc/elements/1.1/>\n\
        PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>\n";
    let search = format!(
        "{prefixes}\
         SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type ?sbolType ?role \
         WHERE {{ ?subject a ?type . ?subject sbh:topLevel ?subject . \
         OPTIONAL{{?subject sbol2:displayId ?displayId}} \
         OPTIONAL{{?subject sbol2:version ?version}} \
         OPTIONAL{{?subject dcterms:title ?name}} \
         OPTIONAL{{?subject dcterms:description ?description}} \
         OPTIONAL{{?subject sbol2:type ?sbolType . \
           FILTER(STRSTARTS(str(?sbolType),'http://www.biopax.org/release/biopax-level3.owl'))}} \
         OPTIONAL{{?subject sbol2:role ?role . \
           FILTER(STRSTARTS(str(?role),'http://identifiers.org/so/'))}} }} LIMIT 50 OFFSET 0"
    );
    let search_count = format!(
        "{prefixes}\
         SELECT (sum(?tc) AS ?count) WHERE {{ {{ SELECT (count(distinct ?subject) AS ?tc) WHERE {{ \
         ?subject a ?type . ?subject sbh:topLevel ?subject . \
         OPTIONAL{{?subject dcterms:title ?n}} }} }} }}"
    );
    let get_collections = format!(
        "{prefixes}\
         SELECT DISTINCT ?subject ?displayId ?name WHERE {{ ?subject a sbol2:Collection . \
         OPTIONAL{{?subject sbol2:displayId ?displayId}} OPTIONAL{{?subject dcterms:title ?name}} }}"
    );
    let count_by_type = format!(
        "{prefixes}\
         SELECT (COUNT(DISTINCT ?cd) AS ?count) WHERE {{ ?cd a sbol2:ComponentDefinition }}"
    );
    let facet_types = format!(
        "{prefixes}\
         SELECT DISTINCT ?object WHERE {{ ?subject a ?object . ?subject sbh:topLevel ?subject }}"
    );
    let facet_roles = format!(
        "{prefixes}\
         SELECT DISTINCT ?object WHERE {{ ?tl sbol2:role ?object . ?tl sbh:topLevel ?tl }}"
    );
    let facet_creators =
        format!("{prefixes}SELECT DISTINCT ?object WHERE {{ ?tl dc:creator ?object }}");
    let members_all = format!(
        "{prefixes}\
         SELECT DISTINCT ?uri ?displayId ?name ?description ?type ?sbolType ?role WHERE {{ \
         <{COLLECTION}> a sbol2:Collection . <{COLLECTION}> sbol2:member ?uri . \
         OPTIONAL{{?uri a ?type}} OPTIONAL{{?uri sbol2:displayId ?displayId}} \
         OPTIONAL{{?uri dcterms:title ?name}} OPTIONAL{{?uri dcterms:description ?description}} \
         OPTIONAL{{?uri sbol2:type ?sbolType . \
           FILTER(STRSTARTS(str(?sbolType),'http://www.biopax.org/release/biopax-level3.owl'))}} \
         OPTIONAL{{?uri sbol2:role ?role . \
           FILTER(STRSTARTS(str(?role),'http://identifiers.org/so/'))}} }} LIMIT 50 OFFSET 0"
    );
    let members_root = format!(
        "{prefixes}\
         SELECT DISTINCT ?uri ?displayId ?name ?description ?type ?sbolType ?role WHERE {{ \
         <{COLLECTION}> a sbol2:Collection . <{COLLECTION}> sbol2:member ?uri . \
         OPTIONAL{{?uri a ?type}} OPTIONAL{{?uri sbol2:displayId ?displayId}} \
         OPTIONAL{{?uri dcterms:title ?name}} OPTIONAL{{?uri dcterms:description ?description}} \
         OPTIONAL{{?uri sbol2:type ?sbolType . \
           FILTER(STRSTARTS(str(?sbolType),'http://www.biopax.org/release/biopax-level3.owl'))}} \
         OPTIONAL{{?uri sbol2:role ?role . \
           FILTER(STRSTARTS(str(?role),'http://identifiers.org/so/'))}} \
         FILTER(STRSTARTS(str(?uri), '{MEMBER_PREFIX}')) \
         FILTER NOT EXISTS {{ <{COLLECTION}> sbol2:member ?om . \
           {{ ?om ?r ?uri }} UNION {{ ?om ?r ?c . ?c ?cr ?uri }} FILTER(?om != ?uri) }} }} \
         LIMIT 50 OFFSET 0"
    );
    let count_members = format!(
        "{prefixes}\
         SELECT (COUNT(DISTINCT ?uri) AS ?count) WHERE {{ <{COLLECTION}> sbol2:member ?uri . \
         FILTER NOT EXISTS {{ <{COLLECTION}> sbol2:member ?om . \
           {{ ?om ?r ?uri }} UNION {{ ?om ?r ?c . ?c ?cr ?uri }} FILTER(?om != ?uri) }} }}"
    );
    // getMetadata: a constant subject, required title, optional description.
    let metadata = |subject: &str| {
        format!(
            "{prefixes}SELECT ?name ?description WHERE {{ \
             <{subject}> dcterms:title ?name . \
             OPTIONAL{{<{subject}> dcterms:description ?description}} }}"
        )
    };
    vec![
        ("search", search),
        ("search_count", search_count),
        ("get_collections", get_collections),
        ("count_by_type", count_by_type),
        ("facet_types", facet_types),
        ("facet_roles", facet_roles),
        ("facet_creators", facet_creators),
        ("members_all", members_all),
        ("members_root", members_root),
        ("count_members", count_members),
        ("metadata_full", metadata(META_FULL)),
        ("metadata_no_desc", metadata(META_NO_DESC)),
        ("metadata_no_title", metadata(META_NO_TITLE)),
        ("metadata_missing", metadata(META_MISSING)),
    ]
}

/// Seed the fixture through the real Graph Store write path, then assert the
/// accelerator answers each template exactly as the generic engine does, and
/// that the accelerator was actually used.
async fn assert_accel_matches_generic(store: &dyn SbolStore, source: Arc<dyn TripleSource>) {
    store
        .graph_store_write(
            PUBLIC_GRAPH,
            SBOL2,
            sbol_db_core::SerializationFormat::Turtle,
            GraphWriteMode::Replace,
        )
        .await
        .expect("seed public graph");
    compare_templates(source).await;
}

/// Regression: re-posting a document with `Merge` must not inflate the
/// accelerator. A backend that reconstructs a graph's post-write triple set by
/// concatenating the committed scan with the batch's inserts (RocksDB) counts a
/// re-posted, already-committed triple twice unless it dedups, so
/// `build_accel_index` derives duplicate metadata (a doubled `dcterms:title`
/// yields duplicate rows from a single-row metadata query). After two Merge
/// re-writes of the same document the accelerator must still match the generic
/// engine — the triple store holds a set, and the indexes derive from that set.
async fn assert_accel_stable_across_merge_rewrite(
    store: &dyn SbolStore,
    source: Arc<dyn TripleSource>,
) {
    for mode in [
        GraphWriteMode::Replace,
        GraphWriteMode::Merge,
        GraphWriteMode::Merge,
    ] {
        store
            .graph_store_write(
                PUBLIC_GRAPH,
                SBOL2,
                sbol_db_core::SerializationFormat::Turtle,
                mode,
            )
            .await
            .expect("seed public graph");
    }
    compare_templates(source).await;
}

/// Build an accelerator-backed engine and a generic engine over the same source
/// and assert every recognized template returns identical bindings, with the
/// accelerator actually engaged.
async fn compare_templates(source: Arc<dyn TripleSource>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let accel_engine = SparqlEngine::new(Arc::new(Probe {
        inner: Arc::clone(&source),
        hits: Arc::clone(&hits),
    }));
    let generic_engine = SparqlEngine::new(Arc::new(NoAccel(source)));

    for (name, query) in templates() {
        let before = hits.load(Ordering::SeqCst);
        let accel = run(&accel_engine, &query).await;
        let generic = run(&generic_engine, &query).await;
        assert_eq!(
            binding_multiset(&accel),
            binding_multiset(&generic),
            "accelerator diverged from generic engine for `{name}`\n\
             accel:   {accel}\ngeneric: {generic}"
        );
        assert!(
            hits.load(Ordering::SeqCst) > before,
            "`{name}` was not served by the accelerator (recognizer missed it)"
        );
    }
}

async fn run(engine: &SparqlEngine, query: &str) -> String {
    let outcome = engine
        .execute(query, Some(ResultFormat::Json), &options())
        .await
        .unwrap_or_else(|e| panic!("execute failed:\n{query}\nerror: {e:?}"));
    String::from_utf8(outcome.payload.body).expect("utf8")
}

#[tokio::test]
async fn sqlite_accelerator_matches_generic_engine() {
    use sbol_db_sqlite::{connect_and_migrate, SqliteStore};
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite://{}", dir.path().join("accel.db").display());
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");
    let store = SqliteStore::new(pool);
    let source = store.triple_source();
    assert_accel_matches_generic(&store, source).await;
}

#[tokio::test]
async fn rocksdb_accelerator_matches_generic_engine() {
    use sbol_db_rocksdb::{connect, RocksdbStore};
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("rocksdb://{}", dir.path().join("accel.rocksdb").display());
    let db = connect(&url).expect("open rocksdb");
    let store = RocksdbStore::new(db);
    let source = store.triple_source();
    assert_accel_matches_generic(&store, source).await;
}

#[tokio::test]
async fn sqlite_accelerator_stable_across_merge_rewrite() {
    use sbol_db_sqlite::{connect_and_migrate, SqliteStore};
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite://{}", dir.path().join("accel.db").display());
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");
    let store = SqliteStore::new(pool);
    let source = store.triple_source();
    assert_accel_stable_across_merge_rewrite(&store, source).await;
}

#[tokio::test]
async fn rocksdb_accelerator_stable_across_merge_rewrite() {
    use sbol_db_rocksdb::{connect, RocksdbStore};
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("rocksdb://{}", dir.path().join("accel.rocksdb").display());
    let db = connect(&url).expect("open rocksdb");
    let store = RocksdbStore::new(db);
    let source = store.triple_source();
    assert_accel_stable_across_merge_rewrite(&store, source).await;
}

/// Opt-in (it truncates the shared Postgres database, so it must not run
/// alongside the other Postgres-backed test binaries): `ACCEL_DIFF_PG=1`.
#[tokio::test]
async fn postgres_accelerator_matches_generic_engine() {
    if std::env::var("ACCEL_DIFF_PG").is_err() {
        eprintln!("skipping postgres differential test; set ACCEL_DIFF_PG=1 to run");
        return;
    }
    use sbol_db_postgres::{connect, run_migrations, SbolObjectService};
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_triples, accel_dirty, accel_object, accel_type, \
         accel_member, accel_facet RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    let svc = SbolObjectService::new(pool);
    let source = svc.triple_source();
    assert_accel_matches_generic(&svc, source).await;
}
