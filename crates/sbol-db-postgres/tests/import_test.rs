use std::sync::OnceLock;

use sbol_db_core::{IriString, SerializationFormat, ValidationStatus};
use sbol_db_postgres::{connect, run_migrations, ImportInput, SbolObjectService};
use sqlx::Row;
use tokio::sync::{Mutex, MutexGuard};

const FIXTURE: &str = include_str!("fixtures/simple_component.ttl");
const NESTED: &str = include_str!("fixtures/nested_construct.ttl");

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// Tests share one Postgres instance, so we serialize them on a process-wide
/// mutex. Each test still truncates inside its critical section.
async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

async fn fresh_service() -> SbolObjectService {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    // Reset between tests so they're isolated.
    sqlx::query(
        "TRUNCATE sbol_documents, sbol_objects, sbol_quads, validation_findings, \
         validation_runs, object_revisions, rdf_projection_events RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    SbolObjectService::new(pool)
}

#[tokio::test]
async fn imports_simple_component_fixture() {
    let _guard = db_lock().await;
    let svc = fresh_service().await;
    let report = svc
        .import_document(ImportInput {
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            source_uri: Some("test://simple_component.ttl".to_owned()),
            document_iri: None,
            created_by: Some("test".to_owned()),
            name: Some("simple_component".to_owned()),
            description: None,
        })
        .await
        .expect("import");

    assert_eq!(report.object_count, 2);
    assert!(report.quad_count >= 10);
    assert_eq!(report.validation_status, ValidationStatus::Passed);

    let obj = svc
        .objects()
        .get_by_iri("https://example.org/sbol-db/test/promoter_j23119")
        .await
        .expect("get")
        .expect("promoter exists");
    assert_eq!(obj.sbol_class, "http://sbols.org/v3#Component");
    assert!(obj
        .types
        .contains(&"https://identifiers.org/SBO:0000251".to_owned()));
    assert!(obj
        .roles
        .contains(&"https://identifiers.org/SO:0000167".to_owned()));
}

#[tokio::test]
async fn reimport_is_idempotent_and_bumps_revision() {
    let _guard = db_lock().await;
    let svc = fresh_service().await;
    let input = || ImportInput {
        body: FIXTURE.to_owned(),
        format: SerializationFormat::Turtle,
        source_uri: None,
        document_iri: None,
        created_by: None,
        name: None,
        description: None,
    };
    let first = svc.import_document(input()).await.expect("first import");
    let second = svc.import_document(input()).await.expect("second import");
    assert_ne!(first.document_id, second.document_id);

    // Second import should leave only the second document's quads behind,
    // since each import owns its own document graph.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sbol_quads WHERE document_id = $1")
        .bind(second.document_id.as_uuid())
        .fetch_one(svc.pool())
        .await
        .expect("count");
    assert_eq!(count as usize, second.quad_count);

    // The object's revision_number should be >= 2 because the upsert created
    // a fresh revision on each import.
    let max_rev: i64 =
        sqlx::query_scalar("SELECT MAX(revision_number) FROM object_revisions WHERE iri = $1")
            .bind("https://example.org/sbol-db/test/promoter_j23119")
            .fetch_one(svc.pool())
            .await
            .expect("max rev");
    assert!(max_rev >= 2, "expected at least 2 revisions, got {max_rev}");
}

#[tokio::test]
async fn validation_findings_are_persisted() {
    let _guard = db_lock().await;
    let svc = fresh_service().await;
    let report = svc
        .import_document(ImportInput {
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            source_uri: None,
            document_iri: Some(IriString::new("https://example.org/sbol-db/test/doc/1").unwrap()),
            created_by: None,
            name: None,
            description: None,
        })
        .await
        .expect("import");

    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM validation_runs WHERE target_document_id = $1")
            .bind(report.document_id.as_uuid())
            .fetch_one(svc.pool())
            .await
            .expect("count");
    assert_eq!(runs, 1);

    let summary_row =
        sqlx::query("SELECT summary FROM validation_runs WHERE target_document_id = $1")
            .bind(report.document_id.as_uuid())
            .fetch_one(svc.pool())
            .await
            .expect("summary");
    let summary: serde_json::Value = summary_row.try_get("summary").unwrap();
    assert!(summary.get("issue_count").is_some());
}

#[tokio::test]
async fn quads_round_trip_through_export() {
    let _guard = db_lock().await;
    let svc = fresh_service().await;
    let _ = svc
        .import_document(ImportInput {
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            source_uri: None,
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        })
        .await
        .expect("import");

    let quads = svc
        .quads()
        .quads_for_subject("https://example.org/sbol-db/test/promoter_j23119")
        .await
        .expect("quads");
    assert!(!quads.is_empty(), "promoter should have quads");

    let turtle = sbol_db_rdf::quads_to_rdf(&quads, SerializationFormat::Turtle).expect("serialize");
    // Re-parse to verify the output is valid Turtle the upstream crate accepts.
    let _ = sbol::Document::read(&turtle, sbol::RdfFormat::Turtle).expect("re-parse");
}

#[tokio::test]
async fn nested_fixture_populates_typed_projections() {
    let _guard = db_lock().await;
    let svc = fresh_service().await;
    let _ = svc
        .import_document(ImportInput {
            body: NESTED.to_owned(),
            format: SerializationFormat::Turtle,
            source_uri: Some("test://nested_construct.ttl".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        })
        .await
        .expect("import");

    let counts: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT 'components' AS k, count(*)::bigint FROM sbol_components
        UNION ALL SELECT 'sequences', count(*) FROM sbol_sequences
        UNION ALL SELECT 'features', count(*) FROM sbol_features
        UNION ALL SELECT 'locations', count(*) FROM sbol_locations
        UNION ALL SELECT 'constraints', count(*) FROM sbol_constraints
        UNION ALL SELECT 'interactions', count(*) FROM sbol_interactions
        UNION ALL SELECT 'participations', count(*) FROM sbol_participations
        ORDER BY 1
        "#,
    )
    .fetch_all(svc.pool())
    .await
    .expect("counts");
    let map: std::collections::BTreeMap<_, _> = counts.into_iter().collect();
    assert_eq!(map["components"], 2, "B0015 + i13504");
    assert_eq!(map["sequences"], 2);
    assert_eq!(map["features"], 1, "i13504/SubComponent1");
    assert_eq!(map["locations"], 1, "i13504/SubComponent1/Range1");

    // Verify the Range location landed with correct positions.
    let (start, end): (i32, i32) = sqlx::query_as(
        "SELECT start_pos, end_pos FROM sbol_locations WHERE location_kind = 'Range' LIMIT 1",
    )
    .fetch_one(svc.pool())
    .await
    .expect("range row");
    assert_eq!((start, end), (1, 80));

    // Verify the SubComponent's parent and instance_of.
    let (parent, instance_of): (String, String) = sqlx::query_as(
        "SELECT parent_component_iri::text, instance_of_iri::text FROM sbol_features \
         WHERE feature_kind = 'SubComponent' LIMIT 1",
    )
    .fetch_one(svc.pool())
    .await
    .expect("feature row");
    assert_eq!(parent, "https://synbiohub.org/public/igem/i13504");
    assert_eq!(instance_of, "https://synbiohub.org/public/igem/B0015");

    // And the inferred alphabet.
    let alphabets: Vec<String> = sqlx::query_scalar("SELECT alphabet FROM sbol_sequences")
        .fetch_all(svc.pool())
        .await
        .expect("alphabets");
    assert!(
        alphabets.iter().all(|a| a == "DNA"),
        "alphabets={alphabets:?}"
    );
}
