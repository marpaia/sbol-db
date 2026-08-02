//! Integration test for the `rebuild_search_index` job handler on SQLite.
//!
//! Imports a SynBioHub-shaped SBOL2 graph verbatim (so the `sbh:topLevel`
//! self-edges survive), runs the handler against a real SQLite store with an
//! in-RAM ranked text index, and asserts that the clustering stage persisted
//! the near-identical promoters into one cluster, that PageRank was persisted,
//! that a matching ComponentDefinition ranks ahead of its Sequence, and that
//! feeding the persisted clusters back into the search activates the
//! divide-by-2 duplicate penalty.

use std::sync::Arc;

use sbol_db_core::{IriString, SerializationFormat};
use sbol_db_jobs::handlers::rebuild_search_index::RebuildSearchIndexHandler;
use sbol_db_jobs::{JobContext, JobHandler, SearchIndexHandles};
use sbol_db_search::ranked_text::{cluster_map, ClusterMap, GraphFilter, Hit, RankedTextIndex};
use sbol_db_sqlite::{
    connect_and_migrate, SqliteClusterStore, SqliteJobRepository, SqlitePageRankStore,
    SqliteSketchStore, SqliteStore,
};
use sbol_db_storage::{
    ClusterStore, GraphWriteMode, ImportInput, ImportOverwrite, JobQueue, NewJob, PageRankStore,
    SbolStore, SketchStore, TripleSource,
};
use tokio_util::sync::CancellationToken;

const PUBLIC_GRAPH: &str = "https://synbiohub.org/public";
const J23100: &str = "https://synbiohub.org/public/igem/BBa_J23100/1";
const J23101: &str = "https://synbiohub.org/public/igem/BBa_J23101/1";
const NATIVE_GRAPH: &str = "https://example.org/native/";
const NATIVE_ALPHA: &str = "https://example.org/native/alpha";
const NATIVE_BETA: &str = "https://example.org/native/beta";
const NATIVE_COMPONENT: &str = "http://sbols.org/v3#Component";

/// A minimal SynBioHub SBOL2 corpus: a Collection with two member
/// ComponentDefinitions, each referencing a Sequence. The two promoter
/// sequences differ by a single base (~0.97 identity) so greedy clustering
/// gathers them into one cluster; every object carries its `sbh:topLevel`
/// self-edge, so the link graph and PageRank see them.
const CORPUS: &str = r#"
@prefix sbol2: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#> .
@prefix biopax: <http://www.biopax.org/release/biopax-level3.owl#> .
@prefix so: <http://identifiers.org/so/> .

<https://synbiohub.org/public/igem/igem_collection/1>
    a sbol2:Collection ;
    sbh:topLevel <https://synbiohub.org/public/igem/igem_collection/1> ;
    sbol2:displayId "igem_collection" ;
    sbol2:version "1" ;
    dcterms:title "iGEM Parts Collection" ;
    sbol2:member <https://synbiohub.org/public/igem/BBa_J23100/1> ,
                 <https://synbiohub.org/public/igem/BBa_J23101/1> .

<https://synbiohub.org/public/igem/BBa_J23100/1>
    a sbol2:ComponentDefinition ;
    sbh:topLevel <https://synbiohub.org/public/igem/BBa_J23100/1> ;
    sbol2:displayId "BBa_J23100" ;
    sbol2:version "1" ;
    dcterms:title "Anderson promoter J23100" ;
    dcterms:description "constitutive promoter" ;
    sbol2:type biopax:DnaRegion ;
    sbol2:role so:0000167 ;
    sbol2:sequence <https://synbiohub.org/public/igem/BBa_J23100_sequence/1> .

<https://synbiohub.org/public/igem/BBa_J23100_sequence/1>
    a sbol2:Sequence ;
    sbh:topLevel <https://synbiohub.org/public/igem/BBa_J23100_sequence/1> ;
    sbol2:displayId "BBa_J23100_sequence" ;
    sbol2:version "1" ;
    sbol2:elements "ttgacggctagctcagtcctaggtacagtgctagc" .

<https://synbiohub.org/public/igem/BBa_J23101/1>
    a sbol2:ComponentDefinition ;
    sbh:topLevel <https://synbiohub.org/public/igem/BBa_J23101/1> ;
    sbol2:displayId "BBa_J23101" ;
    sbol2:version "1" ;
    dcterms:title "Anderson promoter J23101" ;
    dcterms:description "constitutive promoter" ;
    sbol2:type biopax:DnaRegion ;
    sbol2:role so:0000167 ;
    sbol2:sequence <https://synbiohub.org/public/igem/BBa_J23101_sequence/1> .

<https://synbiohub.org/public/igem/BBa_J23101_sequence/1>
    a sbol2:Sequence ;
    sbh:topLevel <https://synbiohub.org/public/igem/BBa_J23101_sequence/1> ;
    sbol2:displayId "BBa_J23101_sequence" ;
    sbol2:version "1" ;
    sbol2:elements "ttgacggctagctcagtcctaggtacagtgctaga" .
"#;

/// A normal SBOL DB import has authoritative object rows but deliberately no
/// compatibility-only `sbh:topLevel` annotations. Rebuilds must union those
/// rows with verbatim compatibility top-levels before indexing and clustering.
const NATIVE_CORPUS: &str = r#"
@prefix sbol: <http://sbols.org/v3#> .
@prefix SBO: <https://identifiers.org/SBO:> .
@prefix SO: <https://identifiers.org/SO:> .
@prefix EDAM: <https://identifiers.org/edam:> .

<https://example.org/native/alpha>
    a sbol:Component ;
    sbol:displayId "alpha" ;
    sbol:name "Native alpha promoter" ;
    sbol:description "A native SBOL DB import" ;
    sbol:hasNamespace <https://example.org/native/> ;
    sbol:type SBO:0000251 ;
    sbol:role SO:0000167 ;
    sbol:hasSequence <https://example.org/native/alpha-sequence> .

<https://example.org/native/alpha-sequence>
    a sbol:Sequence ;
    sbol:displayId "alpha-sequence" ;
    sbol:hasNamespace <https://example.org/native/> ;
    sbol:elements "ttgacggctagctcagtcctaggtacagtgctagc" ;
    sbol:encoding EDAM:format_1207 .

<https://example.org/native/beta>
    a sbol:Component ;
    sbol:displayId "beta" ;
    sbol:name "Native beta promoter" ;
    sbol:description "A second native SBOL DB import" ;
    sbol:hasNamespace <https://example.org/native/> ;
    sbol:type SBO:0000251 ;
    sbol:role SO:0000167 ;
    sbol:hasSequence <https://example.org/native/beta-sequence> .

<https://example.org/native/beta-sequence>
    a sbol:Sequence ;
    sbol:displayId "beta-sequence" ;
    sbol:hasNamespace <https://example.org/native/> ;
    sbol:elements "ttgacggctagctcagtcctaggtacagtgctaga" ;
    sbol:encoding EDAM:format_1207 .
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_populates_ranks_and_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite://{}", dir.path().join("reindex.db").display());
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");

    let store: Arc<dyn SbolStore> = Arc::new(SqliteStore::new(pool.clone()));
    store
        .graph_store_write(
            PUBLIC_GRAPH,
            CORPUS,
            SerializationFormat::Turtle,
            GraphWriteMode::Replace,
        )
        .await
        .expect("load corpus verbatim");

    let sqlite_store = SqliteStore::new(pool.clone());
    let cluster: Arc<dyn ClusterStore> = Arc::new(SqliteClusterStore::new(pool.clone()));
    let pagerank: Arc<dyn PageRankStore> = Arc::new(SqlitePageRankStore::new(pool.clone()));
    let sketch: Arc<dyn SketchStore> = Arc::new(SqliteSketchStore::new(pool.clone()));
    let text_index = Arc::new(RankedTextIndex::in_ram().expect("in-ram index"));
    let triples: Arc<dyn TripleSource> = sqlite_store.triple_source();
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool.clone()));

    let ctx = JobContext {
        job_id: sbol_db_core::JobId::new(),
        worker_id: Arc::from("test-worker"),
        attempt: 1,
        service: store.clone(),
        jobs,
        cancel: CancellationToken::new(),
        search: Some(SearchIndexHandles {
            cluster: cluster.clone(),
            pagerank: pagerank.clone(),
            sketch: sketch.clone(),
            text_index: text_index.clone(),
            triples,
        }),
        vector_indexes: None,
        config: None,
        backups: None,
    };

    RebuildSearchIndexHandler
        .run(ctx, serde_json::json!({}))
        .await
        .expect("rebuild handler runs");

    // PageRank was computed and persisted for the top-level objects.
    let ranks = pagerank.all_ranks().await.expect("all ranks");
    assert!(!ranks.is_empty(), "rebuild must persist PageRank scores");

    // The clustering stage persisted the two near-identical promoters into one
    // cluster, keyed by the ComponentDefinition (part) IRI.
    let c100 = cluster
        .cluster_id_of(J23100)
        .await
        .expect("cluster lookup")
        .expect("BBa_J23100 is clustered");
    let c101 = cluster
        .cluster_id_of(J23101)
        .await
        .expect("cluster lookup")
        .expect("BBa_J23101 is clustered");
    assert_eq!(
        c100, c101,
        "the two near-identical promoters share a cluster"
    );
    let mates = cluster.cluster_mates(J23100).await.expect("cluster mates");
    assert_eq!(
        mates,
        vec![J23101.to_owned()],
        "BBa_J23100's only cluster mate is BBa_J23101"
    );

    // The rebuilt index ranks a matching ComponentDefinition first; the Sequence
    // objects that share the "BBa_J23100" token trail behind their divide-by-10
    // penalty.
    let hits = text_index
        .search("BBa_J23100", 0, 10, &GraphFilter::Any, &ClusterMap::new())
        .expect("search");
    assert!(!hits.is_empty(), "search must return the indexed object");
    assert!(
        hits[0].subject == J23100 || hits[0].subject == J23101,
        "a matching ComponentDefinition ranks first, not a Sequence: got {}",
        hits[0].subject
    );

    // The sketch stage sourced sequences from the verbatim SBOL2 triples, so the
    // persisted MinHash/LSH index is populated even though the derived typed view
    // is empty for a graph-store submission. This is the version-agnostic rebuild
    // path: verbatim SBOL2 sketches just like an upgraded SBOL3 import, so the
    // incremental sequence-search align path has a durable index on this corpus.
    let sketches = sketch.all_sketches().await.expect("all sketches");
    assert!(
        !sketches.is_empty(),
        "rebuild must persist sketches for verbatim SBOL2 sequences"
    );

    // Feeding the persisted clusters into the search activates the divide-by-2
    // duplicate penalty: of the two clustered promoters, whichever ranks first
    // keeps its score and its cluster mate is halved. The two match the query
    // symmetrically, so the leader is whichever tantivy orders first; the
    // invariant is that exactly one is demoted.
    let clusters = cluster_map(vec![(J23100.to_owned(), c100), (J23101.to_owned(), c101)]);
    let baseline = text_index
        .search(
            "BBa_J23100 BBa_J23101",
            0,
            10,
            &GraphFilter::Any,
            &ClusterMap::new(),
        )
        .expect("search");
    let penalized = text_index
        .search("BBa_J23100 BBa_J23101", 0, 10, &GraphFilter::Any, &clusters)
        .expect("search");

    let ratios: Vec<f64> = [J23100, J23101]
        .iter()
        .map(|cd| score_of(&baseline, cd) / score_of(&penalized, cd))
        .collect();
    let halved = ratios.iter().filter(|r| (**r - 2.0).abs() < 1e-6).count();
    let kept = ratios.iter().filter(|r| (**r - 1.0).abs() < 1e-6).count();
    assert_eq!(
        (halved, kept),
        (1, 1),
        "exactly one clustered promoter is halved and the other kept: ratios {ratios:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_indexes_and_clusters_native_imports_without_compatibility_markers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!(
        "sqlite://{}",
        dir.path().join("native-reindex.db").display()
    );
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");

    let sqlite_store = Arc::new(SqliteStore::new(pool.clone()));
    sqlite_store
        .import_document(ImportInput {
            body: NATIVE_CORPUS.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: None,
            document_iri: Some(IriString::new(NATIVE_GRAPH.to_owned()).expect("document IRI")),
            created_by: None,
            name: None,
            description: None,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("import native corpus");

    let service: Arc<dyn SbolStore> = sqlite_store.clone();
    let cluster: Arc<dyn ClusterStore> = Arc::new(SqliteClusterStore::new(pool.clone()));
    let pagerank: Arc<dyn PageRankStore> = Arc::new(SqlitePageRankStore::new(pool.clone()));
    let sketch: Arc<dyn SketchStore> = Arc::new(SqliteSketchStore::new(pool.clone()));
    let text_index = Arc::new(RankedTextIndex::in_ram().expect("in-ram index"));
    let triples: Arc<dyn TripleSource> = sqlite_store.triple_source();
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool));
    let ctx = JobContext {
        job_id: sbol_db_core::JobId::new(),
        worker_id: Arc::from("test-worker"),
        attempt: 1,
        service,
        jobs,
        cancel: CancellationToken::new(),
        search: Some(SearchIndexHandles {
            cluster: cluster.clone(),
            pagerank: pagerank.clone(),
            sketch: sketch.clone(),
            text_index: text_index.clone(),
            triples,
        }),
        vector_indexes: None,
        config: None,
        backups: None,
    };

    let outcome = RebuildSearchIndexHandler
        .run(ctx, serde_json::json!({}))
        .await
        .expect("native rebuild runs");
    assert_eq!(outcome.result.expect("result")["indexed"], 4);

    let hits = text_index
        .search(
            "Native alpha promoter",
            0,
            10,
            &GraphFilter::Only(vec![NATIVE_GRAPH.to_owned()]),
            &ClusterMap::new(),
        )
        .expect("search native index");
    let alpha = hits
        .iter()
        .find(|hit| hit.subject == NATIVE_ALPHA)
        .expect("native component is indexed");
    assert!(alpha.type_iris.iter().any(|iri| iri == NATIVE_COMPONENT));

    let alpha_cluster = cluster
        .cluster_id_of(NATIVE_ALPHA)
        .await
        .expect("alpha cluster lookup")
        .expect("alpha is clustered");
    let beta_cluster = cluster
        .cluster_id_of(NATIVE_BETA)
        .await
        .expect("beta cluster lookup")
        .expect("beta is clustered");
    assert_eq!(alpha_cluster, beta_cluster);

    let ranks = pagerank.all_ranks().await.expect("native ranks");
    assert!(ranks.iter().any(|row| row.iri == NATIVE_ALPHA));
    assert_eq!(
        sketch.all_sketches().await.expect("native sketches").len(),
        2
    );
}

/// The final combined score of a hit with the given subject.
fn score_of(hits: &[Hit], subject: &str) -> f64 {
    hits.iter()
        .find(|h| h.subject == subject)
        .map(|h| h.score)
        .expect("subject present among hits")
}

#[tokio::test]
async fn rebuild_without_search_handle_fails_clearly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite://{}", dir.path().join("noidx.db").display());
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");

    let store: Arc<dyn SbolStore> = Arc::new(SqliteStore::new(pool.clone()));
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool));

    let ctx = JobContext {
        job_id: sbol_db_core::JobId::new(),
        worker_id: Arc::from("test-worker"),
        attempt: 1,
        service: store,
        jobs,
        cancel: CancellationToken::new(),
        search: None,
        vector_indexes: None,
        config: None,
        backups: None,
    };

    let err = RebuildSearchIndexHandler
        .run(ctx, serde_json::json!({}))
        .await
        .expect_err("without a search handle the rebuild must fail");
    assert!(err.to_string().contains("search index handle"));
}

#[tokio::test]
async fn older_rebuild_signal_coalesces_into_the_newest_pending_job() {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite://{}", dir.path().join("coalesce.db").display());
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");

    let store: Arc<dyn SbolStore> = Arc::new(SqliteStore::new(pool.clone()));
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool));
    let older = jobs
        .enqueue(NewJob::new("rebuild_search_index", serde_json::json!({})))
        .await
        .expect("enqueue older")
        .into_job();
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let newer = jobs
        .enqueue(NewJob::new("rebuild_search_index", serde_json::json!({})))
        .await
        .expect("enqueue newer")
        .into_job();

    let ctx = JobContext {
        job_id: older.id,
        worker_id: Arc::from("test-worker"),
        attempt: 1,
        service: store,
        jobs,
        cancel: CancellationToken::new(),
        // Coalescing happens before the expensive handler needs these handles.
        search: None,
        vector_indexes: None,
        config: None,
        backups: None,
    };
    let outcome = RebuildSearchIndexHandler
        .run(ctx, serde_json::json!({}))
        .await
        .expect("older signal coalesces");
    let result = outcome.result.expect("coalescing result");
    assert_eq!(result["coalesced"], true);
    assert_eq!(result["newerJobId"], newer.id.to_string());
}
