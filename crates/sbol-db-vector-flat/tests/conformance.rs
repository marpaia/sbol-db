use std::sync::Arc;

use sbol_db_vector_conformance::run_all;
use sbol_db_vector_flat::ExactFlatVectorBackend;

#[tokio::test]
async fn vector_backend_conformance() {
    run_all(Arc::new(ExactFlatVectorBackend::new("flat-conformance"))).await;
}
