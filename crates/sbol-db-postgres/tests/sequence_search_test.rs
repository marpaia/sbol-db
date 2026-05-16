//! Integration tests for nucleotide substring search via the k-mer index.

use std::sync::OnceLock;

use sbol_db_core::SerializationFormat;
use sbol_db_postgres::{
    connect, run_migrations, ImportInput, SbolObjectService, SequenceSearchOptions,
};
use tokio::sync::{Mutex, MutexGuard};

const NESTED: &str = include_str!("fixtures/nested_construct.ttl");
const SEQUENCE_TEXT: &str =
    "ccaggcatcaaataaaacgaaaggctcagtcgaaagactgggcctttcgttttatctgttgtttgtcggtgaacgctctc";

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

async fn fresh_service() -> SbolObjectService {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_documents, sbol_objects, sbol_quads, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations, sbol_sequence_kmers, sbol_ontologies, \
         sbol_ontology_terms, sbol_ontology_term_aliases, sbol_ontology_closure \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    let svc = SbolObjectService::new(pool);
    svc.import_document(ImportInput {
        body: NESTED.to_owned(),
        format: SerializationFormat::Turtle,
        source_uri: None,
        document_iri: None,
        created_by: None,
        name: None,
        description: None,
    })
    .await
    .expect("import");
    svc
}

#[tokio::test]
async fn forward_seed_hits_known_substring() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    // Use a span the fixture contains. CCAGGCAT lives at position 0.
    let matches = svc
        .sequence_search()
        .search("CCAGGCAT", SequenceSearchOptions::default())
        .await
        .expect("search");
    assert!(
        !matches.is_empty(),
        "expected at least one forward match, got {matches:?}"
    );
    let zero_starts: Vec<_> = matches.iter().filter(|m| m.start == 0).collect();
    assert!(
        !zero_starts.is_empty(),
        "expected a match at position 0, got {matches:?}"
    );
    assert!(zero_starts.iter().any(|m| m.strand == '+'));
    let positions: Vec<i32> = matches.iter().map(|m| m.start).collect();
    assert!(positions
        .iter()
        .all(|p| (*p as usize + 8) <= SEQUENCE_TEXT.len()));
}

#[tokio::test]
async fn reverse_complement_hits_canonical() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    // RC of CCAGGCAT is ATGCCTGG. The forward sequence has CCAGGCAT at
    // position 0, so a query for ATGCCTGG must match position 0 with
    // strand='-'.
    let matches = svc
        .sequence_search()
        .search("ATGCCTGG", SequenceSearchOptions::default())
        .await
        .expect("search");
    assert!(
        matches.iter().any(|m| m.strand == '-' && m.start == 0),
        "expected RC match at position 0, got {matches:?}"
    );
}

#[tokio::test]
async fn forward_only_excludes_rc_strand() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    let matches = svc
        .sequence_search()
        .search(
            "ATGCCTGG",
            SequenceSearchOptions {
                forward_only: Some(true),
                ..SequenceSearchOptions::default()
            },
        )
        .await
        .expect("search");
    assert!(
        matches.is_empty(),
        "RC hit should be suppressed when forward_only=true, got {matches:?}"
    );
}

#[tokio::test]
async fn short_pattern_falls_back_to_substring_scan() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    // Query shorter than k=8 -- exercise the ILIKE fallback path.
    let matches = svc
        .sequence_search()
        .search("CCAGG", SequenceSearchOptions::default())
        .await
        .expect("search");
    assert!(
        matches.iter().any(|m| m.start == 0 && m.strand == '+'),
        "short query should still hit substring at position 0, got {matches:?}"
    );
}

#[tokio::test]
async fn missing_pattern_returns_empty() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    // A pattern with bases not present anywhere -- there are no Ns in the
    // fixture so a stretch of Ts followed by Cs that doesn't occur works.
    let matches = svc
        .sequence_search()
        .search("AAAAAAAAGGGGGGGG", SequenceSearchOptions::default())
        .await
        .expect("search");
    assert!(matches.is_empty(), "unexpected hits: {matches:?}");
}

#[tokio::test]
async fn ambiguous_bases_skip_window() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    // Query contains an N -- our index drops windows with ambiguous bases,
    // so the seed lookup yields nothing. The verification pass also fails.
    let matches = svc
        .sequence_search()
        .search("NNNNNNNN", SequenceSearchOptions::default())
        .await
        .expect("search");
    assert!(matches.is_empty(), "ambiguous query should not match");
}
