//! Scale bench for `rebuild_search_index` clustering + `/similar`.
//!
//! Ignored by default; run explicitly to measure the near-linear rebuild and the
//! cluster-mate read latency on a realistically large, SynBioHub-shaped corpus:
//!
//! ```text
//! cargo test -p sbol-db-jobs --test scale_bench -- --ignored --nocapture
//! ```
//!
//! The corpus is generated in the verbatim SBOL2 shape SynBioHub stores (each
//! top level carries its `sbh:topLevel` self-edge, a `sbol2:sequence` link, and
//! a `sbol2:elements` literal), loaded verbatim through `graph_store_write` so
//! the rebuild handler's clustering, PageRank, and text stages all see it. It
//! mixes near-duplicate families (a base sequence plus point-mutated variants
//! that share a cluster) with random singletons, so clustering does real work.
//! `BENCH_FAMILIES`, `BENCH_VARIANTS`, and `BENCH_SINGLETONS` size the run.

use std::sync::Arc;
use std::time::Instant;

use sbol_db_core::SerializationFormat;
use sbol_db_jobs::handlers::rebuild_search_index::RebuildSearchIndexHandler;
use sbol_db_jobs::{JobContext, JobHandler, SearchIndexHandles};
use sbol_db_search::ranked_text::RankedTextIndex;
use sbol_db_sqlite::{
    connect_and_migrate, SqliteClusterStore, SqliteJobRepository, SqlitePageRankStore,
    SqliteSketchStore, SqliteStore,
};
use sbol_db_storage::{
    ClusterStore, GraphWriteMode, JobQueue, PageRankStore, SbolStore, SketchStore, TripleSource,
};
use tokio_util::sync::CancellationToken;

const PUBLIC_GRAPH: &str = "https://synbiohub.org/public";
const BASE: &str = "https://synbiohub.org/public/bench";

/// SplitMix64 finalizer: a deterministic 64-bit mixer for reproducible corpora.
fn splitmix64(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A deterministic pseudo-random uppercase ACGT sequence of length `len`.
fn rand_seq(state: &mut u64, len: usize) -> String {
    let bases = *b"ACGT";
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        let r = splitmix64(state);
        s.push(bases[(r >> 40) as usize % 4] as char);
    }
    s
}

/// Flip `count` bases of `seq`, keeping a near-duplicate that still shares a
/// high k-mer Jaccard with the base (so it collides in the LSH sketch and the
/// aligner accepts it into the base's cluster).
fn mutate(seq: &str, state: &mut u64, count: usize) -> String {
    let mut v: Vec<u8> = seq.bytes().collect();
    for _ in 0..count {
        let r = splitmix64(state);
        let at = (r >> 40) as usize % v.len();
        v[at] = if v[at] == b'A' { b'C' } else { b'A' };
    }
    String::from_utf8(v).unwrap()
}

/// One SynBioHub-shaped ComponentDefinition + its Sequence, verbatim SBOL2.
fn emit_part(out: &mut String, id: usize, elements: &str) {
    let cd = format!("{BASE}/cd_{id}/1");
    let seq = format!("{BASE}/cd_{id}_seq/1");
    out.push_str(&format!(
        "<{cd}>\n    a sbol2:ComponentDefinition ;\n    sbh:topLevel <{cd}> ;\n    \
         sbol2:displayId \"cd_{id}\" ;\n    sbol2:version \"1\" ;\n    \
         dcterms:title \"part {id}\" ;\n    sbol2:type biopax:DnaRegion ;\n    \
         sbol2:role so:0000167 ;\n    sbol2:sequence <{seq}> .\n\n"
    ));
    out.push_str(&format!(
        "<{seq}>\n    a sbol2:Sequence ;\n    sbh:topLevel <{seq}> ;\n    \
         sbol2:displayId \"cd_{id}_seq\" ;\n    sbol2:version \"1\" ;\n    \
         sbol2:elements \"{elements}\" .\n\n"
    ));
}

/// Build the corpus and return `(turtle, part_iris, family_reps)` where
/// `family_reps` maps a family's parts so co-clustering can be checked.
fn build_corpus(
    families: usize,
    variants: usize,
    singletons: usize,
) -> (String, Vec<String>, Vec<Vec<String>>) {
    let mut state: u64 = 0x5EED_5EED_5EED_5EED;
    let mut out = String::new();
    out.push_str(
        "@prefix sbol2: <http://sbols.org/v2#> .\n\
         @prefix dcterms: <http://purl.org/dc/terms/> .\n\
         @prefix sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#> .\n\
         @prefix biopax: <http://www.biopax.org/release/biopax-level3.owl#> .\n\
         @prefix so: <http://identifiers.org/so/> .\n\n",
    );
    let mut part_iris = Vec::new();
    let mut family_groups = Vec::new();
    let mut id = 0usize;

    for _ in 0..families {
        let base_len = 600 + (splitmix64(&mut state) as usize % 400);
        let base = rand_seq(&mut state, base_len);
        let mut group = Vec::new();
        for _ in 0..variants {
            let elements = mutate(&base, &mut state, 1);
            emit_part(&mut out, id, &elements);
            let iri = format!("{BASE}/cd_{id}/1");
            group.push(iri.clone());
            part_iris.push(iri);
            id += 1;
        }
        family_groups.push(group);
    }
    for _ in 0..singletons {
        let len = 400 + (splitmix64(&mut state) as usize % 400);
        let elements = rand_seq(&mut state, len);
        emit_part(&mut out, id, &elements);
        part_iris.push(format!("{BASE}/cd_{id}/1"));
        id += 1;
    }

    (out, part_iris, family_groups)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale bench; run with --ignored --nocapture"]
async fn rebuild_and_similar_scale() {
    let families = env_usize("BENCH_FAMILIES", 200);
    let variants = env_usize("BENCH_VARIANTS", 20);
    let singletons = env_usize("BENCH_SINGLETONS", 1000);
    let total = families * variants + singletons;

    let (corpus, part_iris, family_groups) = build_corpus(families, variants, singletons);

    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite://{}", dir.path().join("bench.db").display());
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");

    let store: Arc<dyn SbolStore> = Arc::new(SqliteStore::new(pool.clone()));
    let load_start = Instant::now();
    store
        .graph_store_write(
            PUBLIC_GRAPH,
            &corpus,
            SerializationFormat::Turtle,
            GraphWriteMode::Replace,
        )
        .await
        .expect("load corpus verbatim");
    let load_secs = load_start.elapsed().as_secs_f64();

    let sqlite_store = SqliteStore::new(pool.clone());
    let cluster: Arc<dyn ClusterStore> = Arc::new(SqliteClusterStore::new(pool.clone()));
    let pagerank: Arc<dyn PageRankStore> = Arc::new(SqlitePageRankStore::new(pool.clone()));
    let sketch: Arc<dyn SketchStore> = Arc::new(SqliteSketchStore::new(pool.clone()));
    let text_index = Arc::new(RankedTextIndex::in_ram().expect("in-ram index"));
    let triples: Arc<dyn TripleSource> = sqlite_store.triple_source();
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool.clone()));

    let ctx = JobContext {
        job_id: sbol_db_core::JobId::new(),
        worker_id: Arc::from("bench-worker"),
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
        config: None,
    };

    let rebuild_start = Instant::now();
    let outcome = RebuildSearchIndexHandler
        .run(ctx, serde_json::json!({}))
        .await
        .expect("rebuild handler runs");
    let rebuild_secs = rebuild_start.elapsed().as_secs_f64();

    let assignments = cluster.all_assignments().await.expect("all assignments");
    let distinct: std::collections::HashSet<_> = assignments.iter().map(|(_, c)| c.0).collect();

    // Each near-duplicate family collapses into a single cluster.
    let mut collapsed_families = 0usize;
    for group in &family_groups {
        let ids: std::collections::HashSet<_> = group
            .iter()
            .filter_map(|iri| assignments.iter().find(|(a, _)| a == iri).map(|(_, c)| c.0))
            .collect();
        if ids.len() == 1 {
            collapsed_families += 1;
        }
    }

    // Cluster-mate read latency: the `/similar` core, a plain indexed lookup.
    let sample: Vec<&String> = part_iris
        .iter()
        .step_by(part_iris.len() / 200 + 1)
        .collect();
    let reads = sample.len().max(1);
    let similar_start = Instant::now();
    let mut mate_total = 0usize;
    for iri in &sample {
        mate_total += cluster
            .cluster_mates(iri)
            .await
            .expect("cluster mates")
            .len();
    }
    let similar_ms = similar_start.elapsed().as_secs_f64() * 1000.0 / reads as f64;

    println!(
        "SCALE_BENCH result={}",
        outcome.result.unwrap_or_else(|| serde_json::json!({}))
    );
    println!(
        "SCALE_BENCH sequences={total} parts={} load_secs={load_secs:.3} \
         rebuild_secs={rebuild_secs:.3} clusters={} assignments={} \
         families={families} collapsed_families={collapsed_families} \
         similar_ms={similar_ms:.4} sample_reads={reads} sample_mate_total={mate_total}",
        part_iris.len(),
        distinct.len(),
        assignments.len(),
    );

    assert!(
        !assignments.is_empty(),
        "clustering must assign parts on a verbatim SBOL2 corpus"
    );
    assert!(
        collapsed_families * 5 >= families * 4,
        "at least 80% of near-duplicate families collapse to one cluster: \
         {collapsed_families}/{families}"
    );
}
