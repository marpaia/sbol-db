#![cfg(feature = "native")]

use std::sync::Arc;

use sbol_db_search_faiss::{FaissBackendConfig, FaissVectorBackend};
use sbol_db_vector_conformance::run_all;

#[tokio::test]
async fn vector_backend_conformance() {
    let directory = tempfile::tempdir().expect("temporary FAISS store");
    let backend = FaissVectorBackend::open(FaissBackendConfig::new(
        "faiss-conformance",
        directory.path(),
    ))
    .expect("open FAISS backend");
    run_all(Arc::new(backend)).await;
}
