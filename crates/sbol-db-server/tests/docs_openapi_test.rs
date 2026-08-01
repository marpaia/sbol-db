//! Contract tests for the `/docs` render surface and the OpenAPI documents it
//! embeds. These drive the real axum router over a SQLite backend to prove that
//! the docs page and all three specs (native sbol-db, SynBioHub v1, SynBioHub
//! v2) are served, parse as valid JSON, and each documents its own surface
//! without carrying the others.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const V1_OPENAPI: &str = include_str!("../src/synbiohub_openapi.json");
const V1_PUBLIC_ROUTES: &str = include_str!("../src/synbiohub/mod.rs");
const V1_ADMIN_ROUTES: &str = include_str!("../src/synbiohub/admin/mod.rs");
const V1_ALIASES: &str = include_str!("../../../docs/synbiohub-compatibility-aliases.txt");

const V1_TAGS: &[&str] = &[
    "SynBioHub v1 Admin",
    "SynBioHub v1 Attachments",
    "SynBioHub v1 Auth",
    "SynBioHub v1 Downloads",
    "SynBioHub v1 Edit",
    "SynBioHub v1 Permissions",
    "SynBioHub v1 Plugins",
    "SynBioHub v1 Query",
    "SynBioHub v1 SPARQL",
    "SynBioHub v1 Submission",
];

/// Build a router over a fresh SQLite backend. The returned `TempDir` owns the
/// database file and must outlive the router.
async fn app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("docs.db");
    let url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite backend has a migrator")
        .run_migrations()
        .await
        .expect("run migrations");

    let sparql = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        backend.triple_source.clone(),
        backend.triple_writer.clone(),
    ));
    let config = ServerConfig::default();
    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app: Arc::new(AppServices::from_backend(&backend)),
        metrics: Metrics::install(None, env!("CARGO_PKG_VERSION")),
        jobs: backend.jobs.clone(),
        lab: backend.lab.clone(),
        config: config.clone(),
        backend_kind: backend.kind,
        sql_console: backend.sql_console.clone(),
        db_stats: backend.db_stats.clone(),
        lsm_stats: backend.lsm_stats.clone(),
        schema_cache: Arc::new(SchemaCache::new()),
    };
    (router(state, config), dir)
}

async fn body_string(res: Response<Body>) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

/// Issue one GET, returning the status and body.
async fn get(app: &axum::Router, uri: &str) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .expect("request");
    let status = res.status();
    (status, body_string(res).await)
}

#[tokio::test]
async fn docs_page_renders_three_surfaces_in_one_reference() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/docs").await;
    assert_eq!(status, StatusCode::OK, "the docs page is served");
    assert!(
        body.contains("Scalar.createApiReference"),
        "the docs page mounts via the explicit createApiReference call that drives multi-source"
    );
    assert!(
        body.contains("data-sbol-docs-shell") && body.contains("theme: \"none\""),
        "the docs page uses the shared SBOL DB reference shell"
    );
    for source in [
        "/openapi.json",
        "/synbiohub/openapi.json",
        "/api/v2/openapi.json",
    ] {
        assert!(
            body.contains(source),
            "the docs page lists {source} as a switcher source"
        );
    }
    // The V2 surface also stays directly reachable at its own page.
    let (v2_status, v2_body) = get(&app, "/api/v2/docs").await;
    assert_eq!(v2_status, StatusCode::OK, "the V2 docs page is served");
    assert!(
        v2_body.contains("data-url=\"/api/v2/openapi.json\""),
        "the V2 docs page renders the V2 spec"
    );
    assert!(
        v2_body.contains("data-sbol-docs-shell") && v2_body.contains("V2 API reference"),
        "the V2 docs page uses the same SBOL DB reference shell"
    );
}

#[tokio::test]
async fn native_spec_is_served_and_excludes_the_synbiohub_surface() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK, "the native spec is served");
    let spec: Value = serde_json::from_str(&body).expect("native spec is valid JSON");
    assert_eq!(spec["openapi"], "3.1.0", "declares OpenAPI 3.1");

    let paths = spec["paths"].as_object().expect("paths object");
    for path in ["/graphs", "/objects", "/sparql"] {
        assert!(paths.contains_key(path), "the native spec documents {path}");
    }
    assert!(
        !paths.contains_key("/login") && !paths.contains_key("/submit"),
        "the native spec does not carry the SynBioHub v1 surface"
    );
}

#[tokio::test]
async fn synbiohub_v1_spec_is_served_and_documents_representative_paths() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/synbiohub/openapi.json").await;
    assert_eq!(status, StatusCode::OK, "the SynBioHub v1 spec is served");
    let spec: Value = serde_json::from_str(&body).expect("SynBioHub v1 spec is valid JSON");
    assert_eq!(spec["openapi"], "3.1.0", "declares OpenAPI 3.1");

    let paths = spec["paths"].as_object().expect("paths object");
    for path in ["/login", "/logout", "/register", "/submit", "/search"] {
        assert!(
            paths.contains_key(path),
            "the SynBioHub v1 spec documents {path}"
        );
    }
    // The SBOL download lives under the object route hierarchy.
    assert!(
        paths.keys().any(|p| p.ends_with("/sbol")),
        "the SynBioHub v1 spec documents an SBOL download route"
    );
    assert!(
        !paths.contains_key("/graphs"),
        "the SynBioHub v1 spec does not carry the native surface"
    );
}

#[tokio::test]
async fn v2_spec_is_served_and_documents_representative_paths() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/api/v2/openapi.json").await;
    assert_eq!(status, StatusCode::OK, "the V2 spec is served");
    let spec: Value = serde_json::from_str(&body).expect("V2 spec is valid JSON");
    assert_eq!(spec["openapi"], "3.1.0", "declares OpenAPI 3.1");
    assert_eq!(spec["servers"][0]["url"], "/api/v2", "based at /api/v2");

    let paths = spec["paths"].as_object().expect("paths object");
    for path in ["/objects", "/search"] {
        assert!(paths.contains_key(path), "the V2 spec documents {path}");
    }
}

#[tokio::test]
async fn all_spec_routes_serve_valid_json() {
    let (app, _dir) = app().await;
    for uri in [
        "/openapi.json",
        "/synbiohub/openapi.json",
        "/api/v2/openapi.json",
    ] {
        let (status, body) = get(&app, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri} is served");
        serde_json::from_str::<Value>(&body).unwrap_or_else(|e| panic!("{uri} is valid JSON: {e}"));
    }
}

#[test]
fn synbiohub_inventory_covers_every_runtime_compatibility_route() {
    let spec: Value = serde_json::from_str(V1_OPENAPI).expect("SynBioHub OpenAPI parses");
    let spec_paths: BTreeSet<String> = spec["paths"]
        .as_object()
        .expect("paths object")
        .keys()
        .cloned()
        .collect();
    let runtime_paths = runtime_v1_paths();
    let alias_paths: BTreeSet<String> = V1_ALIASES
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    let protocol_paths: BTreeSet<String> = [
        "/sparql-auth".to_owned(),
        "/sparql-graph-crud-auth".to_owned(),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        spec_paths.len(),
        109,
        "the primary catalog path count changed"
    );
    assert_eq!(
        alias_paths.len(),
        61,
        "the supplemental alias count changed"
    );
    assert_eq!(
        runtime_paths.len(),
        168,
        "the runtime V1 route count changed"
    );
    assert!(
        spec_paths.is_disjoint(&alias_paths),
        "a compatibility alias moved into OpenAPI without leaving the supplemental list"
    );

    let undocumented: BTreeSet<_> = runtime_paths.difference(&spec_paths).cloned().collect();
    assert_eq!(
        undocumented, alias_paths,
        "every runtime route outside the primary OpenAPI catalog must be classified in the supplemental inventory"
    );
    let non_runtime_spec: BTreeSet<_> = spec_paths.difference(&runtime_paths).cloned().collect();
    assert_eq!(
        non_runtime_spec, protocol_paths,
        "only the two top-level Virtuoso compatibility routes live outside the V1 router"
    );
}

#[test]
fn every_primary_v1_operation_has_one_classified_family() {
    let spec: Value = serde_json::from_str(V1_OPENAPI).expect("SynBioHub OpenAPI parses");
    let known: BTreeSet<&str> = V1_TAGS.iter().copied().collect();
    let mut operations = 0usize;

    for (path, item) in spec["paths"].as_object().expect("paths object") {
        for (method, operation) in item.as_object().expect("path item") {
            if method == "parameters" {
                continue;
            }
            operations += 1;
            let tags = operation["tags"]
                .as_array()
                .unwrap_or_else(|| panic!("{method} {path} has tags"));
            assert_eq!(tags.len(), 1, "{method} {path} has exactly one family");
            let tag = tags[0]
                .as_str()
                .unwrap_or_else(|| panic!("{method} {path} tag is text"));
            assert!(
                known.contains(tag),
                "{method} {path} has unknown family {tag}"
            );
        }
    }
    assert_eq!(operations, 127, "the classified V1 operation count changed");
}

fn runtime_v1_paths() -> BTreeSet<String> {
    [V1_PUBLIC_ROUTES, V1_ADMIN_ROUTES]
        .into_iter()
        .flat_map(extract_route_literals)
        .collect()
}

fn extract_route_literals(source: &str) -> Vec<String> {
    // Test-only routers in these files are not part of the deployed surface.
    let mut rest = source.split("#[cfg(test)]").next().unwrap_or(source);
    let mut paths = Vec::new();
    while let Some(route_start) = rest.find(".route(") {
        rest = &rest[route_start + ".route(".len()..];
        let quote_start = rest.find('"').expect("route begins with a string literal");
        rest = &rest[quote_start + 1..];
        let quote_end = rest.find('"').expect("route string closes");
        paths.push(normalize_axum_path(&rest[..quote_end]));
        rest = &rest[quote_end + 1..];
    }
    paths
}

fn normalize_axum_path(path: &str) -> String {
    let mut normalized = path
        .split('/')
        .map(|segment| {
            segment
                .strip_prefix(':')
                .or_else(|| segment.strip_prefix('*'))
                .map_or_else(|| segment.to_owned(), |name| format!("{{{name}}}"))
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    normalized
}
