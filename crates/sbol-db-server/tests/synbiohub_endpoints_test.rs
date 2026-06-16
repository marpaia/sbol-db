//! HTTP-level integration tests for the SynBioHub/Virtuoso-compatible write
//! surface: `/sparql-auth` (SPARQL Update), `/sparql-graph-crud-auth/` (Graph
//! Store CRUD), HTTP Basic auth, and `default-graph-uri`-scoped reads on
//! `/sparql`. These drive the actual axum router via `oneshot`, exercising the
//! same wire shapes SynBioHub sends to Virtuoso.

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const GRAPH: &str = "https://synbiohub.org/public";

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

/// Truncate and build a fresh router with default config (auth enabled,
/// `dba`/`dba`). The returned `Router` is cloned per request.
async fn fresh_app() -> axum::Router {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query("TRUNCATE sbol_graphs, sbol_triples RESTART IDENTITY CASCADE")
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
    let pg_pool = pool.clone();
    let metrics = Metrics::install(pool, env!("CARGO_PKG_VERSION"));
    let state = AppState {
        service,
        sparql,
        sparql_update,
        metrics,
        jobs,
        config: ServerConfig::default(),
        pg_pool,
        schema_cache: Arc::new(sbol_db_server::SchemaCache::new()),
    };
    router(state, ServerConfig::default())
}

fn basic_auth() -> String {
    let token = base64::engine::general_purpose::STANDARD.encode("dba:dba");
    format!("Basic {token}")
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

fn form(pairs: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(pairs).expect("encode form")
}

/// POST a SPARQL read as a form (avoids URL-encoding the query into the URI).
async fn read(app: &axum::Router, query: &str, graph: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/sparql")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/sparql-results+json")
        .body(Body::from(form(&[
            ("query", query),
            ("default-graph-uri", graph),
        ])))
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("read");
    assert_eq!(res.status(), StatusCode::OK, "read should succeed");
    body_string(res).await
}

const TTL_X: &str = "@prefix sbol2: <http://sbols.org/v2#> .\n\
     <https://synbiohub.org/public/x/1> a sbol2:ComponentDefinition ; sbol2:displayId \"x\" .";

#[tokio::test]
async fn graph_store_write_requires_auth() {
    let _g = db_lock().await;
    let app = fresh_app().await;
    let uri = format!("/sparql-graph-crud-auth/?{}", form(&[("graph-uri", GRAPH)]));

    // No credentials → 401 with a Basic challenge.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("content-type", "text/turtle")
                .body(Body::from(TTL_X))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(
        res.headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("Basic"))
            .unwrap_or(false),
        "expected Basic challenge"
    );

    // Wrong credentials → 401.
    let bad = base64::engine::general_purpose::STANDARD.encode("dba:wrong");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("authorization", format!("Basic {bad}"))
                .header("content-type", "text/turtle")
                .body(Body::from(TTL_X))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn graph_store_post_merges_and_is_readable() {
    let _g = db_lock().await;
    let app = fresh_app().await;
    let uri = format!("/sparql-graph-crud-auth/?{}", form(&[("graph-uri", GRAPH)]));

    // Authenticated POST stores the triples verbatim.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("authorization", basic_auth())
                .header("content-type", "text/turtle")
                .body(Body::from(TTL_X))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK, "{}", "post should store");

    // Read it back through /sparql scoped to the graph.
    let body = read(
        &app,
        "PREFIX sbol2: <http://sbols.org/v2#> \
         SELECT ?s WHERE { ?s a sbol2:ComponentDefinition }",
        GRAPH,
    )
    .await;
    assert!(
        body.contains("https://synbiohub.org/public/x/1"),
        "stored subject should be queryable: {body}"
    );

    // A second POST to the same graph accumulates (merge, not replace).
    let ttl_y = "@prefix sbol2: <http://sbols.org/v2#> .\n\
         <https://synbiohub.org/public/y/1> a sbol2:ComponentDefinition .";
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("authorization", basic_auth())
                .header("content-type", "text/turtle")
                .body(Body::from(ttl_y))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = read(
        &app,
        "PREFIX sbol2: <http://sbols.org/v2#> \
         SELECT ?s WHERE { ?s a sbol2:ComponentDefinition }",
        GRAPH,
    )
    .await;
    assert!(
        body.contains("/x/1") && body.contains("/y/1"),
        "both kept: {body}"
    );
}

#[tokio::test]
async fn graph_store_put_replaces_and_delete_clears() {
    let _g = db_lock().await;
    let app = fresh_app().await;
    let uri = format!("/sparql-graph-crud-auth/?{}", form(&[("graph-uri", GRAPH)]));

    let post = |ttl: &'static str| {
        let uri = uri.clone();
        let app = app.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&uri)
                    .header("authorization", basic_auth())
                    .header("content-type", "text/turtle")
                    .body(Body::from(ttl))
                    .unwrap(),
            )
            .await
            .expect("post")
        }
    };

    assert_eq!(post(TTL_X).await.status(), StatusCode::OK);

    // PUT replaces the whole graph with a different triple.
    let ttl_z = "@prefix sbol2: <http://sbols.org/v2#> .\n\
         <https://synbiohub.org/public/z/1> a sbol2:ComponentDefinition .";
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&uri)
                .header("authorization", basic_auth())
                .header("content-type", "text/turtle")
                .body(Body::from(ttl_z))
                .unwrap(),
        )
        .await
        .expect("put");
    assert_eq!(res.status(), StatusCode::OK);

    let body = read(
        &app,
        "PREFIX sbol2: <http://sbols.org/v2#> SELECT ?s WHERE { ?s a sbol2:ComponentDefinition }",
        GRAPH,
    )
    .await;
    assert!(body.contains("/z/1"), "PUT'd triple present: {body}");
    assert!(!body.contains("/x/1"), "old triple replaced: {body}");

    // DELETE clears the graph.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&uri)
                .header("authorization", basic_auth())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("delete");
    assert_eq!(res.status(), StatusCode::OK);
    let body = read(
        &app,
        "PREFIX sbol2: <http://sbols.org/v2#> SELECT ?s WHERE { ?s a sbol2:ComponentDefinition }",
        GRAPH,
    )
    .await;
    assert!(!body.contains("/z/1"), "graph cleared: {body}");
}

#[tokio::test]
async fn sparql_auth_insert_update_then_read() {
    let _g = db_lock().await;
    let app = fresh_app().await;

    // Unauthenticated update is challenged.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sparql-auth")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form(&[(
                    "query",
                    "INSERT DATA { <a:s> <a:p> <a:o> }",
                )])))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Authenticated INSERT DATA (update string in the `query=` param, the
    // Virtuoso convention), scoped to the graph via default-graph-uri.
    let update = "INSERT DATA { <https://synbiohub.org/public/note/1> \
                  <http://purl.org/dc/terms/title> \"hello\" . }";
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sparql-auth")
                .header("authorization", basic_auth())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form(&[
                    ("query", update),
                    ("default-graph-uri", GRAPH),
                ])))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK, "update should succeed");

    let body = read(
        &app,
        "SELECT ?o WHERE { <https://synbiohub.org/public/note/1> \
         <http://purl.org/dc/terms/title> ?o }",
        GRAPH,
    )
    .await;
    assert!(body.contains("hello"), "inserted triple readable: {body}");
}

#[tokio::test]
async fn lab_graph_triples_returns_verbatim_triples() {
    let _g = db_lock().await;
    let app = fresh_app().await;
    let crud = format!("/sparql-graph-crud-auth/?{}", form(&[("graph-uri", GRAPH)]));

    // Write a verbatim graph (2 triples) through the Graph Store endpoint.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&crud)
                .header("authorization", basic_auth())
                .header("content-type", "text/turtle")
                .body(Body::from(TTL_X))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);

    // The graph shows up in the graph-native listing as `verbatim`.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/lab/api/graphs?kind=verbatim")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let list: serde_json::Value =
        serde_json::from_str(&body_string(res).await).expect("graphs json");
    let graph = list["graphs"]
        .as_array()
        .and_then(|gs| gs.iter().find(|g| g["iri"] == GRAPH))
        .expect("verbatim graph present in listing");
    assert_eq!(graph["kind"], "verbatim");
    let id = graph["id"].as_str().expect("graph id");

    // Its raw triples are browsable through the per-graph triples endpoint.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/lab/api/graphs/{id}/triples"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let triples: serde_json::Value =
        serde_json::from_str(&body_string(res).await).expect("triples json");
    assert_eq!(triples["total"], 2, "TTL_X has two triples");
    let rows = triples["triples"].as_array().expect("triples array");
    assert!(
        rows.iter().any(|q| {
            q["subject"]["value"] == "https://synbiohub.org/public/x/1"
                && q["predicate"]["value"] == "http://sbols.org/v2#displayId"
                && q["object"]["type"] == "literal"
                && q["object"]["value"] == "x"
        }),
        "expected the displayId literal triple: {rows:?}"
    );

    // Unknown graph id → 404.
    let res = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/lab/api/graphs/00000000-0000-0000-0000-000000000000/triples")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
