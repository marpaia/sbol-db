//! HTTP-level integration tests for the SynBioHub/Virtuoso-compatible write
//! surface: `/sparql-auth` (SPARQL Update), `/sparql-graph-crud-auth/` (Graph
//! Store CRUD), Digest/Basic auth, and `default-graph-uri`-scoped reads on
//! `/sparql`. These drive the actual axum router via `oneshot`, exercising the
//! same wire shapes SynBioHub sends to Virtuoso.

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use sbol_db_app::AppServices;
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
    let pool_console = pool.clone();
    let pool_stats = pool.clone();
    let metrics = Metrics::install(Some(pool), env!("CARGO_PKG_VERSION"));
    let config = ServerConfig {
        admin_api_auth_required: false,
        ..ServerConfig::default()
    };
    let state = AppState {
        lab: service.clone(),
        app: Arc::new(AppServices::new(
            service.clone(),
            sparql.clone(),
            sparql_update.clone(),
            jobs.clone(),
            service.clone(),
        )),
        service,
        sparql,
        sparql_update,
        metrics,
        jobs,
        config: config.clone(),
        backend_kind: sbol_db_server::BackendKind::Postgres,
        sql_console: Some(Arc::new(sbol_db_postgres::PgSqlConsole::new(pool_console))),
        db_stats: Some(Arc::new(sbol_db_postgres::PgStatsRepository::new(
            pool_stats,
        ))),
        lsm_stats: None,
        schema_cache: Arc::new(sbol_db_server::SchemaCache::new()),
    };
    router(state, config)
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

    // No credentials → 401 with a Virtuoso-shaped Digest challenge.
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
    let www = res
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .expect("challenge header")
        .to_owned();
    assert!(
        www.starts_with("Digest realm=\"SPARQL\""),
        "expected Digest challenge, got: {www}"
    );
    assert!(
        www.contains("qop=\"auth\"") && www.contains("algorithm=MD5"),
        "expected qop/algorithm in challenge: {www}"
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

/// Extract a quoted parameter value from a `WWW-Authenticate: Digest` header.
fn challenge_param(header: &str, key: &str) -> String {
    let marker = format!("{key}=\"");
    let start = header.find(&marker).expect("param present") + marker.len();
    let rest = &header[start..];
    rest[..rest.find('"').expect("closing quote")].to_owned()
}

/// Build the `Authorization: Digest` header a challenge-driven client (curl
/// `--digest`, SynBioHub's HTTP libraries) sends in response to a challenge.
fn digest_authorization(method: &str, uri: &str, realm: &str, nonce: &str) -> String {
    use md5::{Digest as _, Md5};
    let md5_hex = |s: &str| hex::encode(Md5::digest(s.as_bytes()));
    let ha1 = md5_hex(&format!("dba:{realm}:dba"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let nc = "00000001";
    let cnonce = "deadbeef";
    let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
    format!(
        "Digest username=\"dba\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", \
         qop=auth, nc={nc}, cnonce=\"{cnonce}\", response=\"{response}\", algorithm=MD5"
    )
}

/// Full Virtuoso-style handshake: unauthenticated request → 401 Digest
/// challenge → retried request with the computed Digest response → stored.
#[tokio::test]
async fn graph_store_digest_handshake_stores() {
    let _g = db_lock().await;
    let app = fresh_app().await;
    let uri = format!("/sparql-graph-crud-auth/?{}", form(&[("graph-uri", GRAPH)]));

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
        .expect("challenge request");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let www = res
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .expect("challenge header")
        .to_owned();
    let realm = challenge_param(&www, "realm");
    let nonce = challenge_param(&www, "nonce");

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header(
                    "authorization",
                    digest_authorization("POST", &uri, &realm, &nonce),
                )
                .header("content-type", "text/turtle")
                .body(Body::from(TTL_X))
                .unwrap(),
        )
        .await
        .expect("authenticated request");
    assert_eq!(res.status(), StatusCode::OK, "digest retry should store");

    let body = read(
        &app,
        "PREFIX sbol2: <http://sbols.org/v2#> \
         SELECT ?s WHERE { ?s a sbol2:ComponentDefinition }",
        GRAPH,
    )
    .await;
    assert!(
        body.contains("https://synbiohub.org/public/x/1"),
        "digest-authed write should be queryable: {body}"
    );
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
