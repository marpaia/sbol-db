//! Integration tests for graph neighborhood traversal against the live
//! Postgres in docker-compose.

use std::sync::OnceLock;

use sbol_db_core::{Direction, EdgeObject, IriString, NeighborhoodQuery, SerializationFormat};
use sbol_db_postgres::{connect, run_migrations, ImportInput, SbolObjectService};
use tokio::sync::{Mutex, MutexGuard};

const NESTED: &str = include_str!("fixtures/nested_construct.ttl");

const I13504: &str = "https://synbiohub.org/public/igem/i13504";
const SUBCOMP: &str = "https://synbiohub.org/public/igem/i13504/SubComponent1";
const B0015: &str = "https://synbiohub.org/public/igem/B0015";
const RANGE1: &str = "https://synbiohub.org/public/igem/i13504/SubComponent1/Range1";

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

async fn fresh_service_with_fixture() -> SbolObjectService {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_objects, sbol_triples, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    let svc = SbolObjectService::new(pool);
    svc.import_document(ImportInput {
        body: NESTED.to_owned(),
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
    svc
}

fn node_ids(result: &sbol_db_core::NeighborhoodResult) -> Vec<&str> {
    let mut ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn depth_0_returns_only_the_root() {
    let _g = db_lock().await;
    let svc = fresh_service_with_fixture().await;
    let result = svc
        .neighborhood()
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(I13504),
            depth: 0,
            direction: Direction::Forward,
            predicate_allowlist: vec![],
            max_nodes: Some(64),
            include_literals: false,
        })
        .await
        .expect("walk");
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].id, I13504);
    assert_eq!(result.max_depth_reached, 0);
    assert!(result.edges.is_empty());
}

#[tokio::test]
async fn forward_depth_1_reaches_typed_neighbors() {
    let _g = db_lock().await;
    let svc = fresh_service_with_fixture().await;
    let result = svc
        .neighborhood()
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(I13504),
            depth: 1,
            direction: Direction::Forward,
            predicate_allowlist: vec![],
            max_nodes: Some(64),
            include_literals: false,
        })
        .await
        .expect("walk");
    let ids = node_ids(&result);
    assert!(ids.contains(&I13504));
    assert!(ids.contains(&SUBCOMP), "should reach SubComponent at d=1");
    assert!(
        ids.contains(&"https://synbiohub.org/public/igem/i13504_Sequence1"),
        "should reach its Sequence at d=1"
    );
    // Class IRI from rdf:type should also appear as a reachable node.
    assert!(ids.contains(&"http://sbols.org/v3#Component"));
}

#[tokio::test]
async fn backward_walk_finds_users_of_a_component() {
    let _g = db_lock().await;
    let svc = fresh_service_with_fixture().await;
    let result = svc
        .neighborhood()
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(B0015),
            depth: 2,
            direction: Direction::Backward,
            predicate_allowlist: vec![],
            max_nodes: Some(64),
            include_literals: false,
        })
        .await
        .expect("walk");
    let ids = node_ids(&result);
    // B0015 is instanceOf-pointed-to by SubComponent1, which is hasFeature'd by i13504.
    assert!(ids.contains(&B0015));
    assert!(ids.contains(&SUBCOMP));
    assert!(
        ids.contains(&I13504),
        "should reach the parent component at d=2"
    );
}

#[tokio::test]
async fn predicate_allowlist_restricts_traversal() {
    let _g = db_lock().await;
    let svc = fresh_service_with_fixture().await;
    let result = svc
        .neighborhood()
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(I13504),
            depth: 3,
            direction: Direction::Forward,
            predicate_allowlist: vec![IriString::unchecked("http://sbols.org/v3#hasFeature")],
            max_nodes: Some(64),
            include_literals: false,
        })
        .await
        .expect("walk");
    let ids = node_ids(&result);
    // Only `hasFeature` is followed, so we should reach SubComponent1 but
    // not B0015 (which is reached via `instanceOf`) and not the Sequence.
    assert!(ids.contains(&I13504));
    assert!(ids.contains(&SUBCOMP));
    assert!(!ids.contains(&B0015));
    assert!(!ids.contains(&"https://synbiohub.org/public/igem/i13504_Sequence1"));
}

#[tokio::test]
async fn literals_pass_emits_literal_edges_for_visited_nodes() {
    let _g = db_lock().await;
    let svc = fresh_service_with_fixture().await;
    let result = svc
        .neighborhood()
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(I13504),
            depth: 1,
            direction: Direction::Forward,
            predicate_allowlist: vec![],
            max_nodes: Some(64),
            include_literals: true,
        })
        .await
        .expect("walk");
    let literal_edges: Vec<_> = result
        .edges
        .iter()
        .filter(|e| matches!(e.object, EdgeObject::Literal { .. }))
        .collect();
    assert!(
        !literal_edges.is_empty(),
        "expected literal edges (sbol:displayId, etc.) for the root"
    );
}

#[tokio::test]
async fn forward_walk_produces_round_trip_turtle() {
    let _g = db_lock().await;
    let svc = fresh_service_with_fixture().await;
    let result = svc
        .neighborhood()
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(I13504),
            depth: 3,
            direction: Direction::Forward,
            predicate_allowlist: vec![],
            max_nodes: Some(256),
            include_literals: true,
        })
        .await
        .expect("walk");
    assert!(result.nodes.iter().any(|n| n.id == RANGE1));
    let turtle =
        sbol_db_rdf::neighborhood_to_rdf(&result, SerializationFormat::Turtle).expect("serialize");
    // `sbol-rs` should accept the output.
    let _ = sbol::Document::read(&turtle, sbol::RdfFormat::Turtle).expect("re-parse");
}

#[tokio::test]
async fn nodes_decorated_with_sbol_class_and_display_id() {
    let _g = db_lock().await;
    let svc = fresh_service_with_fixture().await;
    let result = svc
        .neighborhood()
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(I13504),
            depth: 1,
            direction: Direction::Forward,
            predicate_allowlist: vec![],
            max_nodes: Some(64),
            include_literals: false,
        })
        .await
        .expect("walk");
    let subcomp = result
        .nodes
        .iter()
        .find(|n| n.id == SUBCOMP)
        .expect("subcomp node");
    assert_eq!(
        subcomp.sbol_class.as_deref(),
        Some("http://sbols.org/v3#SubComponent")
    );
    assert_eq!(subcomp.display_id.as_deref(), Some("SubComponent1"));
}
