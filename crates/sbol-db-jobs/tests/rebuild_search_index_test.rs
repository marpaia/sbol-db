//! Integration test for the `rebuild_search_index` job handler on SQLite.
//!
//! Imports a SynBioHub-shaped SBOL2 graph verbatim (so the `sbh:topLevel`
//! self-edges survive), runs the handler against a real SQLite store with an
//! in-RAM ranked text index, and asserts that PageRank was persisted and the
//! rebuilt index ranks a known display id first.

use std::sync::Arc;

use sbol_db_core::SerializationFormat;
use sbol_db_jobs::handlers::rebuild_search_index::RebuildSearchIndexHandler;
use sbol_db_jobs::{JobContext, JobHandler, SearchIndexHandles};
use sbol_db_search::ranked_text::{ClusterMap, GraphFilter, RankedTextIndex};
use sbol_db_sqlite::{connect_and_migrate, SqliteJobRepository, SqlitePageRankStore, SqliteStore};
use sbol_db_storage::{GraphWriteMode, JobQueue, PageRankStore, SbolStore, TripleSource};
use tokio_util::sync::CancellationToken;

const PUBLIC_GRAPH: &str = "https://synbiohub.org/public";
const J23100: &str = "https://synbiohub.org/public/igem/BBa_J23100/1";

/// A minimal SynBioHub SBOL2 corpus: a Collection with one member
/// ComponentDefinition that in turn references a Sequence. Every object carries
/// its `sbh:topLevel` self-edge, so the link graph and PageRank see them.
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
    sbol2:member <https://synbiohub.org/public/igem/BBa_J23100/1> .

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
    let pagerank: Arc<dyn PageRankStore> = Arc::new(SqlitePageRankStore::new(pool.clone()));
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
            pagerank: pagerank.clone(),
            text_index: text_index.clone(),
            triples,
        }),
    };

    RebuildSearchIndexHandler
        .run(ctx, serde_json::json!({}))
        .await
        .expect("rebuild handler runs");

    // PageRank was computed and persisted for the top-level objects.
    let ranks = pagerank.all_ranks().await.expect("all ranks");
    assert!(!ranks.is_empty(), "rebuild must persist PageRank scores");

    // The rebuilt index ranks the known display id's object first; the Sequence
    // that shares the "BBa_J23100" token trails behind its divide-by-10 penalty.
    let hits = text_index
        .search("BBa_J23100", 0, 10, &GraphFilter::Any, &ClusterMap::new())
        .expect("search");
    assert!(!hits.is_empty(), "search must return the indexed object");
    assert_eq!(
        hits[0].subject, J23100,
        "the ComponentDefinition with the matching display id ranks first"
    );
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
    };

    let err = RebuildSearchIndexHandler
        .run(ctx, serde_json::json!({}))
        .await
        .expect_err("without a search handle the rebuild must fail");
    assert!(err.to_string().contains("search index handle"));
}
