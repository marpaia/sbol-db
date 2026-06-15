//! Integration tests for the Tier 1 bulk read surfaces: `get_by_iris`,
//! `list`, `exists_by_hash`, and `search_many`.

use std::sync::OnceLock;

use sbol_db_core::SerializationFormat;
use sbol_db_postgres::{
    connect, run_migrations, ImportInput, ListObjectsFilter, SbolObjectService,
    SequenceSearchOptions,
};
use sbol_db_rdf::hash_bytes;
use tokio::sync::{Mutex, MutexGuard};

const FIXTURE: &str = include_str!("fixtures/simple_component.ttl");
const NESTED: &str = include_str!("fixtures/nested_construct.ttl");

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
        "TRUNCATE sbol_graphs, sbol_objects, sbol_triples, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations, sbol_sequence_kmers \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    SbolObjectService::new(pool)
}

async fn import_fixture(svc: &SbolObjectService, body: &str) {
    svc.import_document(ImportInput {
        body: body.to_owned(),
        format: SerializationFormat::Turtle,
        namespace: None,
        source_uri: None,
        document_iri: None,
        created_by: None,
        name: None,
        description: None,
    })
    .await
    .expect("import");
}

#[tokio::test]
async fn get_by_iris_returns_matching_subset() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    import_fixture(&svc, FIXTURE).await;

    let promoter = "https://example.org/sbol-db/test/promoter_j23119";
    let missing = "https://example.org/sbol-db/test/does-not-exist";
    let records = svc
        .objects()
        .get_by_iris(&[promoter, missing])
        .await
        .expect("get_by_iris");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].iri.as_str(), promoter);
}

#[tokio::test]
async fn get_by_iris_empty_input_returns_empty() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    let records = svc.objects().get_by_iris(&[]).await.expect("empty");
    assert!(records.is_empty());
}

#[tokio::test]
async fn list_paginates_with_keyset_cursor() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    import_fixture(&svc, NESTED).await;

    // First page: limit 1 across the 2 components in the nested fixture.
    let page1 = svc
        .objects()
        .list(&ListObjectsFilter {
            sbol_class: Some("http://sbols.org/v3#Component".to_owned()),
            limit: 1,
            ..ListObjectsFilter::default()
        })
        .await
        .expect("list page 1");
    assert_eq!(page1.len(), 1);

    let cursor = page1[0].iri.as_str().to_owned();
    let page2 = svc
        .objects()
        .list(&ListObjectsFilter {
            sbol_class: Some("http://sbols.org/v3#Component".to_owned()),
            after_iri: Some(cursor.clone()),
            limit: 100,
            ..ListObjectsFilter::default()
        })
        .await
        .expect("list page 2");
    assert!(
        !page2.is_empty(),
        "second page should contain remaining components"
    );
    assert!(
        page2.iter().all(|r| r.iri.as_str() > cursor.as_str()),
        "cursor should be strictly past page 1"
    );
}

#[tokio::test]
async fn list_filters_by_role() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    import_fixture(&svc, FIXTURE).await;

    let promoter_role = "https://identifiers.org/SO:0000167";
    let hits = svc
        .objects()
        .list(&ListObjectsFilter {
            role: Some(promoter_role.to_owned()),
            limit: 100,
            ..ListObjectsFilter::default()
        })
        .await
        .expect("list by role");
    assert!(
        hits.iter()
            .all(|r| r.roles.iter().any(|role| role == promoter_role)),
        "every returned row must carry the role filter, got {hits:?}"
    );
    assert!(!hits.is_empty(), "fixture has at least one promoter");
}

#[tokio::test]
async fn exists_by_hash_round_trips() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    let hash_before = hash_bytes(FIXTURE.as_bytes());
    assert!(
        !svc.graphs().exists_by_hash(&hash_before).await.unwrap(),
        "fixture should not yet be present"
    );
    import_fixture(&svc, FIXTURE).await;
    assert!(
        svc.graphs().exists_by_hash(&hash_before).await.unwrap(),
        "fixture should be visible after import"
    );
}

#[tokio::test]
async fn import_documents_commits_all_when_every_doc_is_valid() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    let inputs = vec![
        ImportInput {
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("bulk://simple".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        },
        ImportInput {
            body: NESTED.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("bulk://nested".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        },
    ];
    let reports = svc.import_documents(inputs).await.expect("bulk import");
    assert_eq!(reports.len(), 2);

    let doc_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sbol_graphs WHERE kind = 'sbol3'")
            .fetch_one(svc.pool())
            .await
            .expect("count documents");
    assert_eq!(doc_count, 2, "both documents should be visible");
}

#[tokio::test]
async fn import_documents_rolls_back_entire_batch_on_failure() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    // The middle input is unparseable Turtle. The first and third are valid.
    // Whole-batch atomicity means *nothing* should land.
    let inputs = vec![
        ImportInput {
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("bulk://good-1".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        },
        ImportInput {
            body: "this is not turtle at all".to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("bulk://bad".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        },
        ImportInput {
            body: NESTED.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("bulk://good-2".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        },
    ];
    let err = svc
        .import_documents(inputs)
        .await
        .expect_err("batch must fail");
    // The parse error from the middle doc surfaces verbatim.
    assert!(
        format!("{err}").to_ascii_lowercase().contains("parse")
            || matches!(err, sbol_db_core::DomainError::Parse(_))
    );

    let doc_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sbol_graphs WHERE kind = 'sbol3'")
            .fetch_one(svc.pool())
            .await
            .expect("count documents");
    assert_eq!(
        doc_count, 0,
        "rolled-back batch must leave the documents table empty"
    );
    let obj_count: i64 = sqlx::query_scalar("SELECT count(*) FROM sbol_objects")
        .fetch_one(svc.pool())
        .await
        .expect("count objects");
    assert_eq!(obj_count, 0, "rolled-back batch must leave no objects");
}

#[tokio::test]
async fn search_many_preserves_pattern_order() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    import_fixture(&svc, NESTED).await;

    let patterns = vec![
        "CCAGGCAT".to_owned(), // present
        "NNNNNNNN".to_owned(), // never matches (ambiguous)
        "ATGCCTGG".to_owned(), // RC of pattern 1
    ];
    let results = svc
        .sequence_search()
        .search_many(&patterns, SequenceSearchOptions::default())
        .await
        .expect("search_many");

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].pattern, "CCAGGCAT");
    assert!(!results[0].matches.is_empty());
    assert_eq!(results[1].pattern, "NNNNNNNN");
    assert!(results[1].matches.is_empty());
    assert_eq!(results[2].pattern, "ATGCCTGG");
    assert!(results[2].matches.iter().any(|m| m.strand == '-'));
}
