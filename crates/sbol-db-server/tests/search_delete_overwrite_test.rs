//! HTTP-level integration tests for the native write/query surface added for
//! the Python client: text search (`GET /search`), delete (`DELETE /graphs/:id`
//! and `DELETE /graphs?document_iri=`), and submit-overwrite (`overwrite` 0/1/2
//! on `POST /graphs`). These drive the axum router via `oneshot` against
//! Postgres, so they need the compose Postgres up (`docker compose up -d
//! postgres`).

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::Value;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const NS: &str = "https://sbol-db.test/rust-it";

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

/// A fresh router with the derived object view also truncated, so search counts
/// start from zero (the object view survives a `sbol_graphs` truncate via its
/// `ON DELETE SET NULL` FK).
async fn fresh_app() -> axum::Router {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query("TRUNCATE sbol_objects, sbol_graphs, sbol_triples RESTART IDENTITY CASCADE")
        .execute(&pool)
        .await
        .expect("truncate");
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let sparql = Arc::new(SparqlEngine::new(service.triple_source()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        service.triple_source(),
        service.triple_writer(),
    ));
    let jobs = Arc::new(JobRepository::new(pool.clone()));
    let pool_console = pool.clone();
    let pool_stats = pool.clone();
    let metrics = Metrics::install(Some(pool), env!("CARGO_PKG_VERSION"));
    let state = AppState {
        lab: service.clone(),
        service,
        sparql,
        sparql_update,
        metrics,
        jobs,
        config: ServerConfig::default(),
        backend_kind: sbol_db_server::BackendKind::Postgres,
        sql_console: Some(Arc::new(sbol_db_postgres::PgSqlConsole::new(pool_console))),
        db_stats: Some(Arc::new(sbol_db_postgres::PgStatsRepository::new(
            pool_stats,
        ))),
        lsm_stats: None,
        schema_cache: Arc::new(sbol_db_server::SchemaCache::new()),
    };
    router(state, ServerConfig::default())
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

async fn body_json(res: axum::response::Response) -> Value {
    serde_json::from_str(&body_string(res).await).expect("json")
}

fn fasta(display_id: &str, sequence: &str) -> String {
    format!(">{display_id} rust integration fixture\n{sequence}\n")
}

/// POST a FASTA document as an import, returning the raw response.
async fn import(
    app: &axum::Router,
    display_id: &str,
    doc_iri: &str,
    overwrite: u8,
) -> axum::response::Response {
    let qs = serde_urlencoded::to_string([
        ("format", "fasta"),
        ("namespace", NS),
        ("document_iri", doc_iri),
        ("overwrite", &overwrite.to_string()),
    ])
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(format!("/graphs?{qs}"))
        .header("content-type", "text/plain")
        .body(Body::from(fasta(display_id, "ttgacggctagctcagtcctaggt")))
        .unwrap();
    app.clone().oneshot(req).await.expect("import")
}

async fn get(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .expect("get")
}

async fn delete(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("delete")
}

async fn search_total(app: &axum::Router, q: &str) -> i64 {
    let res = get(app, &format!("/search?q={q}&limit=0")).await;
    assert_eq!(res.status(), StatusCode::OK);
    body_json(res).await["total"].as_i64().expect("total")
}

#[tokio::test]
async fn search_matches_object_type_and_property() {
    let _guard = db_lock().await;
    let app = fresh_app().await;
    let res = import(&app, "pSearch", &format!("{NS}/search"), 0).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Plain text search finds it and reports a total.
    let res = get(&app, "/search?q=pSearch").await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert!(body["total"].as_i64().unwrap() >= 1);
    assert!(!body["objects"].as_array().unwrap().is_empty());

    // limit=0 is the count-only contract: total set, no rows.
    let res = get(&app, "/search?q=pSearch&limit=0").await;
    let body = body_json(res).await;
    assert!(body["total"].as_i64().unwrap() >= 1);
    assert!(body["objects"].as_array().unwrap().is_empty());

    // Scoped to the displayId predicate literal.
    let prop = "http%3A%2F%2Fsbols.org%2Fv3%23displayId";
    assert!(search_total(&app, &format!("pSearch&property_uri={prop}")).await >= 1);

    // No match, and the empty-query guard.
    assert_eq!(search_total(&app, "no-such-object").await, 0);
    assert_eq!(
        get(&app, "/search?q=").await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn delete_by_id_and_by_document_iri() {
    let _guard = db_lock().await;
    let app = fresh_app().await;
    let doc_iri = format!("{NS}/delete");
    let report = body_json(import(&app, "pDelete", &doc_iri, 0).await).await;
    let graph_id = report["graph_id"].as_str().unwrap().to_owned();

    assert!(search_total(&app, "pDelete").await >= 1);
    assert_eq!(
        get(&app, &format!("/graphs/{graph_id}")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        delete(&app, &format!("/graphs/{graph_id}")).await.status(),
        StatusCode::NO_CONTENT
    );
    // The graph's derived objects are gone from the search view, not just the
    // graph registry row.
    assert_eq!(search_total(&app, "pDelete").await, 0);
    assert_eq!(
        get(&app, &format!("/graphs/{graph_id}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        delete(&app, &format!("/graphs/{graph_id}")).await.status(),
        StatusCode::NOT_FOUND
    );

    // Re-import, then delete by document IRI.
    let doc_iri2 = format!("{NS}/delete-by-iri");
    import(&app, "pDeleteIri", &doc_iri2, 0).await;
    let enc = serde_urlencoded::to_string([("document_iri", &doc_iri2)]).unwrap();
    assert_eq!(
        delete(&app, &format!("/graphs?{enc}")).await.status(),
        StatusCode::NO_CONTENT
    );
    let missing = serde_urlencoded::to_string([("document_iri", &format!("{NS}/nope"))]).unwrap();
    assert_eq!(
        delete(&app, &format!("/graphs?{missing}")).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn overwrite_fail_replace_merge() {
    let _guard = db_lock().await;
    let app = fresh_app().await;
    let doc_iri = format!("{NS}/overwrite");

    // First import succeeds; a second with overwrite=0 collides on the unique
    // document_iri and fails.
    assert_eq!(
        import(&app, "ovV1", &doc_iri, 0).await.status(),
        StatusCode::OK
    );
    assert!(!import(&app, "ovV1", &doc_iri, 0)
        .await
        .status()
        .is_success());

    // Replace swaps the content under the same document IRI: the new object is
    // searchable and the old one is gone from the view.
    assert_eq!(
        import(&app, "ovV2", &doc_iri, 1).await.status(),
        StatusCode::OK
    );
    assert!(search_total(&app, "ovV2").await >= 1);
    assert_eq!(search_total(&app, "ovV1").await, 0);

    // Replace/merge require document_iri; an invalid mode is rejected.
    let no_iri =
        serde_urlencoded::to_string([("format", "fasta"), ("namespace", NS), ("overwrite", "1")])
            .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(format!("/graphs?{no_iri}"))
        .header("content-type", "text/plain")
        .body(Body::from(fasta("ovNoIri", "acgtacgt")))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    // Merge unions the incoming document with the existing one.
    assert_eq!(
        import(&app, "ovMerge", &doc_iri, 2).await.status(),
        StatusCode::OK
    );
    assert!(search_total(&app, "ovMerge").await >= 1);
    assert!(search_total(&app, "ovV2").await >= 1);
}
