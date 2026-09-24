//! The public listing endpoint passes browser filters to the storage contract.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_core::SerializationFormat;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{ImportInput, ImportOverwrite};
use serde_json::Value;
use tower::ServiceExt;

async fn get(app: &axum::Router, params: &[(&str, String)]) -> Value {
    let query = serde_urlencoded::to_string(params).unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/objects/list?{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn listing_filters_shared_imports_and_preserves_cursor_response() {
    let directory = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", directory.path().join("objects.db").display());
    let backend = Backend::open(&url).await.unwrap();
    backend
        .migrator
        .as_ref()
        .unwrap()
        .run_migrations()
        .await
        .unwrap();
    let body = ["a_ITEM%25", "b_item", "c_unrelated"]
        .map(|id| {
            format!(
                "<https://example.org/{id}> a <http://sbols.org/v3#Component> ; \
         <http://sbols.org/v3#hasNamespace> <https://example.org/> ; \
         <http://sbols.org/v3#type> <https://identifiers.org/SBO:0000251> ; \
         <http://sbols.org/v3#role> <https://identifiers.org/SO:0000167> .\n"
            )
        })
        .concat();
    let mut graphs = Vec::new();
    for _ in 0..2 {
        graphs.push(
            backend
                .store
                .import_document(ImportInput {
                    body: body.clone(),
                    format: SerializationFormat::Turtle,
                    namespace: None,
                    source_uri: None,
                    document_iri: None,
                    created_by: None,
                    name: None,
                    description: None,
                    overwrite: ImportOverwrite::Fail,
                })
                .await
                .unwrap()
                .graph_id,
        );
    }
    let config = ServerConfig::default();
    let app = router(
        AppState {
            service: backend.store.clone(),
            sparql: Arc::new(SparqlEngine::new(backend.triple_source.clone())),
            sparql_update: Arc::new(SparqlUpdateEngine::new(
                backend.triple_source.clone(),
                backend.triple_writer.clone(),
            )),
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
        },
        config,
    );

    for graph in graphs {
        let mut params = vec![
            ("graph_id", graph.0.to_string()),
            ("sbol_class", "http://sbols.org/v3#Component".to_owned()),
            ("role", "https://identifiers.org/SO:0000167".to_owned()),
            ("iri_contains", "ItEm".to_owned()),
            ("limit", "1".to_owned()),
        ];
        let first = get(&app, &params).await;
        assert_eq!(first["objects"].as_array().unwrap().len(), 1);
        assert_eq!(first["objects"][0]["iri"], "https://example.org/a_ITEM%25");
        assert_eq!(first["next_cursor"], first["objects"][0]["iri"]);
        params.push(("after", first["next_cursor"].as_str().unwrap().to_owned()));
        let second = get(&app, &params).await;
        assert_eq!(second["objects"][0]["iri"], "https://example.org/b_item");
        params.last_mut().unwrap().1 = second["next_cursor"].as_str().unwrap().to_owned();
        let exhausted = get(&app, &params).await;
        assert_eq!(exhausted["objects"], serde_json::json!([]));
        assert!(exhausted["next_cursor"].is_null());
        params.pop();
        params
            .iter_mut()
            .find(|(key, _)| *key == "iri_contains")
            .unwrap()
            .1 = "%".to_owned();
        let literal = get(&app, &params).await;
        assert_eq!(literal["objects"].as_array().unwrap().len(), 1);
        assert_eq!(
            literal["objects"][0]["iri"],
            "https://example.org/a_ITEM%25"
        );
    }
    let absent = get(&app, &[("graph_id", uuid::Uuid::new_v4().to_string())]).await;
    assert_eq!(absent["objects"], serde_json::json!([]));
    assert!(absent["next_cursor"].is_null());
}
