//! Coverage for `/lab/api/schema/{sql,sparql}`.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::Value;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned())
}

async fn state() -> AppState {
    let pool = connect(&database_url()).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let sparql = Arc::new(SparqlEngine::new(service.triple_source()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        service.triple_source(),
        service.triple_writer(),
    ));
    let jobs = Arc::new(JobRepository::new(pool.clone()));
    let metrics = Metrics::install(Some(pool.clone()), env!("CARGO_PKG_VERSION"));
    AppState {
        lab: service.clone(),
        service,
        sparql,
        sparql_update,
        metrics,
        jobs,
        config: ServerConfig::default(),
        backend_kind: sbol_db_server::BackendKind::Postgres,
        sql_console: Some(Arc::new(sbol_db_postgres::PgSqlConsole::new(pool.clone()))),
        db_stats: Some(Arc::new(sbol_db_postgres::PgStatsRepository::new(pool))),
        lsm_stats: None,
        schema_cache: Arc::new(SchemaCache::new()),
    }
}

async fn get(path: &str) -> (StatusCode, Value) {
    let app = router(state().await, ServerConfig::default());
    let res = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    let status = res.status();
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    let json: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, json)
}

#[tokio::test]
async fn sql_schema_lists_project_tables() {
    let (status, body) = get("/lab/api/schema/sql").await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let tables = body
        .get("tables")
        .and_then(Value::as_array)
        .expect("tables");
    let names: Vec<&str> = tables
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    // We don't enumerate every project table; just check the canonical
    // ones are present.
    assert!(
        names.contains(&"sbol_objects"),
        "expected sbol_objects in tables, got {names:?}"
    );
    assert!(
        names.contains(&"sbol_graphs"),
        "expected sbol_graphs in tables, got {names:?}"
    );
    assert!(
        names.contains(&"sbol_triples"),
        "expected sbol_triples in tables, got {names:?}"
    );

    // Verify column shape on sbol_objects.
    let objects = tables
        .iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some("sbol_objects"))
        .expect("sbol_objects entry");
    let cols = objects
        .get("columns")
        .and_then(Value::as_array)
        .expect("columns");
    assert!(!cols.is_empty());
    let iri_col = cols
        .iter()
        .find(|c| c.get("name").and_then(Value::as_str) == Some("iri"))
        .expect("iri column");
    assert!(iri_col.get("column_type").and_then(Value::as_str).is_some());
    assert!(iri_col.get("nullable").and_then(Value::as_bool).is_some());
}

#[tokio::test]
async fn sparql_schema_includes_curated_prefixes_and_classes() {
    let (status, body) = get("/lab/api/schema/sparql").await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let prefixes = body
        .get("prefixes")
        .and_then(Value::as_array)
        .expect("prefixes");
    let pfx_names: Vec<&str> = prefixes
        .iter()
        .filter_map(|p| p.get("prefix").and_then(Value::as_str))
        .collect();
    for required in ["sbol", "rdf", "rdfs", "owl", "xsd"] {
        assert!(
            pfx_names.contains(&required),
            "expected curated prefix {required}, got {pfx_names:?}"
        );
    }
    // top_classes may be empty if the test DB is fresh, so just check
    // the response shape.
    assert!(body.get("top_classes").and_then(Value::as_array).is_some());
}
