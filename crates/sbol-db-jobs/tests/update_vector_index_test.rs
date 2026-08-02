use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_core::SerializationFormat;
use sbol_db_jobs::handlers::{UpdateVectorIndexHandler, UpdateVectorIndexPayload};
use sbol_db_jobs::{JobContext, JobHandler};
use sbol_db_search::{
    SearchDeploymentBuilder, SearchTopologyConfig, VectorIndexBindingConfig, VectorRebuildSpec,
};
use sbol_db_search_sdk::{
    DataEgress, DistanceMetric, EmbeddingBatch, EmbeddingDescriptor, EmbeddingOutput,
    EmbeddingProvider, EmbeddingVector, Normalization, SearchError, VectorIndexAdmin,
};
use sbol_db_sqlite::{connect_and_migrate, SqliteJobRepository, SqliteStore};
use sbol_db_storage::{GraphStore, ImportInput, ImportOverwrite, JobQueue, ObjectStore, SbolStore};
use sbol_db_vector_flat::ExactFlatVectorBackend;
use tokio_util::sync::CancellationToken;

const COMPONENT: &str = "https://example.org/components/p1";
const DOCUMENT: &str = r#"
@prefix sbol: <http://sbols.org/v3#> .
@prefix SBO: <https://identifiers.org/SBO:> .

<https://example.org/components/p1>
    a sbol:Component ;
    sbol:displayId "p1" ;
    sbol:hasNamespace <https://example.org/components/> ;
    sbol:name "Strong promoter" ;
    sbol:description "Constitutive promoter" ;
    sbol:type SBO:0000251 .
"#;

struct StubEmbedding {
    descriptor: EmbeddingDescriptor,
}

impl StubEmbedding {
    fn new() -> Self {
        Self {
            descriptor: EmbeddingDescriptor {
                id: "test.embedding.v1".to_owned(),
                provider: "test".to_owned(),
                model: "two-axis".to_owned(),
                revision: "sha256:test".to_owned(),
                dimension: 2,
                normalization: Normalization::L2,
                data_egress: DataEgress::None,
            },
        }
    }
}

#[async_trait]
impl EmbeddingProvider for StubEmbedding {
    fn descriptor(&self) -> &EmbeddingDescriptor {
        &self.descriptor
    }

    async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError> {
        Ok(EmbeddingOutput {
            vectors: batch
                .inputs
                .into_iter()
                .map(|_| EmbeddingVector::Dense(vec![1.0, 0.0]))
                .collect(),
        })
    }
}

#[tokio::test]
async fn job_projects_current_state_as_idempotent_upsert_then_delete() {
    let directory = tempfile::tempdir().expect("tempdir");
    let url = format!(
        "sqlite://{}",
        directory.path().join("incremental-vector.db").display()
    );
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let report = store
        .import_document(ImportInput {
            body: DOCUMENT.to_owned(),
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
        .expect("import component");
    assert!(store
        .get_object_by_iri(COMPONENT)
        .await
        .expect("object lookup")
        .is_some());

    let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
    let maintainers = SearchDeploymentBuilder::new(SearchTopologyConfig {
        default_strategy: "unused".to_owned(),
        indexes: vec![VectorIndexBindingConfig {
            index: "components".to_owned(),
            backend: "flat".to_owned(),
            embedding_profile: "test.embedding.v1".to_owned(),
            vector_name: "content".to_owned(),
            graph_payload_field: "graph".to_owned(),
            maintenance: None,
        }],
        embedding_strategies: Vec::new(),
    })
    .register_embedding(Arc::new(StubEmbedding::new()))
    .expect("register embedding")
    .register_vector_backend(backend.clone())
    .expect("register backend")
    .build_maintenance()
    .expect("build maintenance");
    maintainers
        .get("components")
        .expect("components maintainer")
        .rebuild(
            VectorRebuildSpec {
                artifact_id: "components".to_owned(),
                generation: "g1".to_owned(),
                vector_name: "content".to_owned(),
                embedding_profile: "test.embedding.v1".to_owned(),
                distance: DistanceMetric::Cosine,
                batch_size: 4,
                backend_parameters: BTreeMap::new(),
            },
            Vec::new(),
        )
        .await
        .expect("activate empty generation");

    let service: Arc<dyn SbolStore> = store.clone();
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool));
    let context = JobContext {
        job_id: sbol_db_core::JobId::new(),
        worker_id: Arc::from("test-worker"),
        attempt: 1,
        service,
        jobs,
        cancel: CancellationToken::new(),
        search: None,
        vector_indexes: Some(maintainers),
        config: None,
        backups: None,
    };
    let payload = UpdateVectorIndexPayload {
        artifact_id: "components".to_owned(),
        generation: "g1".to_owned(),
        document_ids: vec![COMPONENT.to_owned()],
        batch_size: 4,
    };

    let upsert = UpdateVectorIndexHandler
        .run(context.clone(), payload.clone())
        .await
        .expect("incremental upsert")
        .result
        .expect("upsert report");
    assert_eq!(upsert["upserts"], 1);
    assert_eq!(upsert["deletes"], 0);
    assert_eq!(upsert["vector_count"], 1);

    assert!(store
        .delete_graph(report.graph_id)
        .await
        .expect("delete graph"));
    let delete = UpdateVectorIndexHandler
        .run(context, payload)
        .await
        .expect("incremental delete")
        .result
        .expect("delete report");
    assert_eq!(delete["upserts"], 0);
    assert_eq!(delete["deletes"], 1);
    assert_eq!(delete["vector_count"], 0);
    assert_eq!(
        backend.generations("components").await.unwrap()[0].vector_count,
        0
    );
}
