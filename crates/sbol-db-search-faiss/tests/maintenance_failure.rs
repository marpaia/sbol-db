#![cfg(feature = "native")]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_search::{VectorDocument, VectorIndexMaintainer, VectorRebuildSpec};
use sbol_db_search_faiss::{FaissBackendConfig, FaissVectorBackend};
use sbol_db_search_sdk::{
    DataEgress, DistanceMetric, DocumentId, EmbeddingBatch, EmbeddingDescriptor, EmbeddingOutput,
    EmbeddingProvider, EmbeddingVector, Normalization, SearchError, VectorIndexAdmin, VectorQuery,
    VectorSearcher, VectorValue,
};
use serde_json::json;

struct InjectedFailureEmbedding {
    descriptor: EmbeddingDescriptor,
    fail_on: Option<String>,
}

impl InjectedFailureEmbedding {
    fn new(fail_on: Option<&str>) -> Self {
        Self {
            descriptor: EmbeddingDescriptor {
                id: "failure-test.embedding.v1".to_owned(),
                provider: "test".to_owned(),
                model: "fixed-two-axis".to_owned(),
                revision: "sha256:fixed".to_owned(),
                dimension: 2,
                normalization: Normalization::L2,
                data_egress: DataEgress::None,
            },
            fail_on: fail_on.map(str::to_owned),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for InjectedFailureEmbedding {
    fn descriptor(&self) -> &EmbeddingDescriptor {
        &self.descriptor
    }

    async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError> {
        if batch
            .inputs
            .iter()
            .any(|input| self.fail_on.as_ref() == Some(&input.text))
        {
            return Err(SearchError::Backend(
                "injected embedding failure".to_owned(),
            ));
        }
        Ok(EmbeddingOutput {
            vectors: batch
                .inputs
                .into_iter()
                .map(|_| EmbeddingVector::Dense(vec![1.0, 0.0]))
                .collect(),
        })
    }
}

fn spec(generation: &str) -> VectorRebuildSpec {
    VectorRebuildSpec {
        artifact_id: "parts".to_owned(),
        generation: generation.to_owned(),
        vector_name: "content".to_owned(),
        embedding_profile: "failure-test.embedding.v1".to_owned(),
        distance: DistanceMetric::Cosine,
        batch_size: 1,
        backend_parameters: BTreeMap::new(),
    }
}

fn document(id: &str, text: &str) -> VectorDocument {
    VectorDocument {
        document_id: DocumentId(id.to_owned()),
        text: text.to_owned(),
        payload: BTreeMap::from([("graph".to_owned(), json!("public"))]),
    }
}

#[tokio::test]
async fn partial_rebuild_failure_preserves_the_active_faiss_generation() {
    let directory = tempfile::tempdir().unwrap();
    let backend = Arc::new(
        FaissVectorBackend::open(FaissBackendConfig::new("faiss", directory.path())).unwrap(),
    );
    VectorIndexMaintainer::new(
        Arc::new(InjectedFailureEmbedding::new(None)),
        backend.clone(),
    )
    .rebuild(spec("stable"), [document("stable", "stable")])
    .await
    .unwrap();

    let result = VectorIndexMaintainer::new(
        Arc::new(InjectedFailureEmbedding::new(Some("fail"))),
        backend.clone(),
    )
    .rebuild(
        spec("incomplete"),
        [document("partial", "partial"), document("fail", "fail")],
    )
    .await;
    assert!(matches!(result, Err(SearchError::Backend(_))));

    let generations = backend.generations("parts").await.unwrap();
    assert_eq!(generations.len(), 1);
    assert_eq!(generations[0].handle.generation, "stable");
    assert!(generations[0].active);
    assert!(!directory
        .path()
        .join("generations/parts/incomplete")
        .exists());

    let page = backend
        .query(VectorQuery {
            index: "parts".to_owned(),
            vector_name: "content".to_owned(),
            vector: VectorValue::Dense(vec![1.0, 0.0]),
            filter: None,
            limit: 10,
            cursor: None,
            score_threshold: None,
            parameters: BTreeMap::new(),
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].document_id, DocumentId("stable".to_owned()));
}
