use std::env;
use std::sync::Arc;

use sbol_db_vector_conformance::run_all;
use sbol_db_vector_qdrant::{QdrantRemoteBackend, QdrantRemoteConfig};
use uuid::Uuid;

#[tokio::test]
async fn live_vector_backend_conformance() {
    let Ok(grpc_url) = env::var("SBOL_DB_QDRANT_TEST_GRPC_URL") else {
        eprintln!("skipping live Qdrant conformance; SBOL_DB_QDRANT_TEST_GRPC_URL is unset");
        return;
    };
    let rest_url = env::var("SBOL_DB_QDRANT_TEST_REST_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:6333".to_owned());
    let collection_prefix = format!("sboltest{}", Uuid::new_v4().simple());
    let backend = QdrantRemoteBackend::new(QdrantRemoteConfig {
        id: "qdrant-conformance".to_owned(),
        grpc_url,
        rest_url,
        api_key: None,
        collection_prefix,
        timeout_seconds: 30,
    })
    .expect("construct Qdrant backend");

    run_all(Arc::new(backend)).await;
}
