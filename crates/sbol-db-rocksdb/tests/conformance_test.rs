//! Runs the `sbol-db-conformance` scenarios against the RocksDB backend, the
//! same contract the SQLite and Postgres backends pass.

use std::sync::Arc;

use chrono::Utc;
use sbol_db_app::AppServices;
use sbol_db_core::{IriString, ObjectTerm, SubjectTerm, Triple, User, UserId};
use sbol_db_rocksdb::{
    connect, AccelCountImport, AccelCountKind, AccelFacetImport, AccelMemberImport,
    AccelObjectImport, Db, RocksdbBulkLoader, RocksdbClusterStore, RocksdbConfigStore, RocksdbJobs,
    RocksdbPageRankStore, RocksdbSketchStore, RocksdbStore, RocksdbTokenStore, RocksdbUserStore,
};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{
    AccelSolutions, AcceleratedQuery, AclStore, ClusterStore, ConfigStore, FacetKind, Field,
    JobQueue, MetaRecord, PageRankStore, SbolStore, Scope, SketchStore, TermValue, TokenStore,
    UserStore,
};
use tempfile::TempDir;

fn fresh_db() -> (Db, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("conformance.rocksdb");
    let url = format!("rocksdb://{}", path.display());
    let db = connect(&url).expect("open rocksdb");
    (db, dir)
}

#[tokio::test]
async fn rocksdb_passes_import_and_read_back() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::import_and_read_back(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_graph_set_semantics() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::graph_set_semantics(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_ontology_roundtrip() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::ontology_roundtrip(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_neighborhood_walk() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::neighborhood_walk(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_sequence_search() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::sequence_search(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_job_queue_lifecycle() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::job_queue_lifecycle(&RocksdbJobs::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_full_conformance_suite() {
    let (db, _dir) = fresh_db();
    let store = Arc::new(RocksdbStore::new(db.clone()));
    let sparql = Arc::new(SparqlEngine::new(store.triple_source()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        store.triple_source(),
        store.triple_writer(),
    ));
    let jobs: Arc<dyn JobQueue> = Arc::new(RocksdbJobs::new(db.clone()));
    let store_dyn: Arc<dyn SbolStore> = store.clone();
    let acl: Arc<dyn AclStore> = store;
    let users: Arc<dyn UserStore> = Arc::new(RocksdbUserStore::new(db.clone()));
    let tokens: Arc<dyn TokenStore> = Arc::new(RocksdbTokenStore::new(db.clone()));
    let pagerank: Arc<dyn PageRankStore> = Arc::new(RocksdbPageRankStore::new(db.clone()));
    let cluster: Arc<dyn ClusterStore> = Arc::new(RocksdbClusterStore::new(db.clone()));
    let sketch: Arc<dyn SketchStore> = Arc::new(RocksdbSketchStore::new(db.clone()));
    let config: Arc<dyn ConfigStore> = Arc::new(RocksdbConfigStore::new(db));
    let app = AppServices::new(store_dyn, sparql, sparql_update, jobs, acl)
        .with_identity(users, tokens)
        .with_sequence_stores(pagerank, cluster, sketch)
        .with_config(config);
    sbol_db_conformance::run_all(&app).await;
}

#[tokio::test]
async fn rocksdb_exact_identity_import_preserves_duplicate_email_semantics() {
    let (db, _dir) = fresh_db();
    let store = RocksdbUserStore::new(db);
    let now = Utc::now();
    let users = ["first", "second"].map(|username| User {
        id: UserId::new(),
        username: username.to_owned(),
        name: username.to_owned(),
        email: "shared@example.org".to_owned(),
        affiliation: None,
        password_hash: format!("hash-{username}"),
        graph_uri: format!("https://example.org/user/{username}"),
        is_admin: username == "first",
        is_curator: false,
        is_member: true,
        reset_password_link: None,
        created_at: now,
        updated_at: now,
    });
    store
        .import_exact(users.to_vec())
        .await
        .expect("exact import");

    for expected in &users {
        let actual = store
            .find_by_email_or_username(&expected.username)
            .await
            .expect("username lookup")
            .expect("imported user");
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.created_at, expected.created_at);
    }
    let error = store
        .find_by_email_or_username("shared@example.org")
        .await
        .expect_err("duplicate email must be ambiguous");
    assert!(error.to_string().contains("multiple accounts"));
}

#[tokio::test]
async fn rocksdb_bulk_loader_is_resumable_and_serves_intersection_indexes() {
    let (db, _dir) = fresh_db();
    let loader = RocksdbBulkLoader::new(db.clone());
    loader.prepare("fixture:one").await.expect("prepare");
    let graph = "https://example.org/public";
    let triple = Triple {
        graph_iri: Some(IriString::unchecked(graph)),
        subject: SubjectTerm::Iri(IriString::unchecked("https://example.org/root")),
        predicate: IriString::unchecked("https://example.org/p"),
        object: ObjectTerm::Iri(IriString::unchecked("https://example.org/o")),
    };
    loader
        .write_triples(vec![triple.clone()], "1".to_owned())
        .await
        .expect("first triple page");
    loader
        .write_triples(vec![triple], "1".to_owned())
        .await
        .expect("replayed triple page");
    assert_eq!(loader.count("gspo").await.unwrap(), 1);
    assert_eq!(
        loader.checkpoint("triples").await.unwrap().as_deref(),
        Some("1")
    );

    let ty = "http://sbols.org/v2#ComponentDefinition";
    let role = "http://identifiers.org/so/SO:0000167";
    let root = "https://example.org/root";
    let member = "https://example.org/member";
    let metadata = |display_id: &str| MetaRecord {
        display_id: vec![sbol_db_storage::LitVal {
            value: display_id.to_owned(),
            datatype: "http://www.w3.org/2001/XMLSchema#string".to_owned(),
            language: None,
        }],
        types: vec![ty.to_owned()],
        roles: vec![role.to_owned()],
        top_level: true,
        ..MetaRecord::default()
    };
    loader
        .write_accel_objects(
            vec![
                AccelObjectImport {
                    graph: graph.to_owned(),
                    iri: root.to_owned(),
                    meta: metadata("a-root"),
                },
                AccelObjectImport {
                    graph: graph.to_owned(),
                    iri: member.to_owned(),
                    meta: metadata("b-member"),
                },
            ],
            "done".to_owned(),
        )
        .await
        .unwrap();
    loader
        .write_accel_members(
            vec![AccelMemberImport {
                graph: graph.to_owned(),
                collection: "https://example.org/collection".to_owned(),
                member: member.to_owned(),
                sort_key: "b-member".to_owned(),
                is_root: true,
            }],
            "done".to_owned(),
        )
        .await
        .unwrap();
    loader
        .write_accel_facets(vec![AccelFacetImport {
            graph: graph.to_owned(),
            kind: FacetKind::Types,
            value: ty.to_owned(),
            subject_count: 2,
        }])
        .await
        .unwrap();
    loader
        .write_accel_counts(vec![
            AccelCountImport {
                graph: graph.to_owned(),
                kind: AccelCountKind::RootType(ty.to_owned()),
                count: 1,
            },
            AccelCountImport {
                graph: graph.to_owned(),
                kind: AccelCountKind::TopLevelTypeRole {
                    object_type: ty.to_owned(),
                    role: role.to_owned(),
                },
                count: 2,
            },
        ])
        .await
        .unwrap();

    let source = RocksdbStore::new(db).triple_source();
    let root_rows = source
        .run_accelerated(&AcceleratedQuery::ObjectList {
            graph: graph.to_owned(),
            scope: Scope::RootByType(ty.to_owned()),
            projection: vec![("subject".to_owned(), Field::Subject)],
            offset: 0,
            limit: Some(10),
            subject_prefix: None,
        })
        .unwrap()
        .unwrap();
    assert_eq!(root_rows.rows.len(), 1);
    match &root_rows.rows[0][0] {
        Some(TermValue::Iri(iri)) => assert_eq!(iri, root),
        other => panic!("expected root IRI, got {other:?}"),
    }

    let intersection_count = source
        .run_accelerated(&AcceleratedQuery::Count {
            graph: graph.to_owned(),
            scope: Scope::TopLevelByTypeAndRole {
                object_type: ty.to_owned(),
                role: role.to_owned(),
            },
            var: "count".to_owned(),
            subject_prefix: None,
        })
        .unwrap()
        .unwrap();
    assert_integer(&intersection_count, 2);
    let facet_counts = source
        .run_accelerated(&AcceleratedQuery::FacetCounts {
            graph: graph.to_owned(),
            kind: FacetKind::Types,
            value_var: "value".to_owned(),
            count_var: "count".to_owned(),
        })
        .unwrap()
        .unwrap();
    assert_integer_cell(&facet_counts, 0, 1, 2);

    let other = loader.prepare("fixture:other").await.unwrap_err();
    assert!(other.to_string().contains("belongs to source"));
}

fn assert_integer(solutions: &AccelSolutions, expected: u64) {
    assert_integer_cell(solutions, 0, 0, expected);
}

fn assert_integer_cell(solutions: &AccelSolutions, row: usize, column: usize, expected: u64) {
    match &solutions.rows[row][column] {
        Some(TermValue::Literal { value, .. }) => assert_eq!(value, &expected.to_string()),
        other => panic!("expected integer literal, got {other:?}"),
    }
}
