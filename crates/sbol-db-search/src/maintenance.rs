//! Backend-neutral vector index construction.
//!
//! A rebuild is staged into a new immutable generation. The prior generation
//! remains active until every document has been embedded and written and the
//! backend has flushed and optimized the new generation. Failed builds are
//! removed on a best-effort basis and never activate.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use sbol_db_search_sdk::{
    DistanceMetric, DocumentId, EmbeddingBatch, EmbeddingDescriptor, EmbeddingInput,
    EmbeddingInputKind, EmbeddingProvenance, EmbeddingProvider, EmbeddingVector, GenerationHandle,
    GenerationStatus, IndexGenerationSpec, SearchError, VectorBackend, VectorChange, VectorValue,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

/// One canonical corpus projection. The primary store owns the document; the
/// vector payload contains only fields needed for backend-native narrowing.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorDocument {
    pub document_id: DocumentId,
    pub text: String,
    pub payload: BTreeMap<String, Value>,
}

/// Reproducible inputs to one full vector generation build.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorRebuildSpec {
    pub artifact_id: String,
    pub generation: String,
    pub vector_name: String,
    pub embedding_profile: String,
    pub distance: DistanceMetric,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub backend_parameters: BTreeMap<String, Value>,
}

/// One idempotent desired-state change to an active vector generation.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorDocumentChange {
    Upsert(VectorDocument),
    Delete { document_id: DocumentId },
}

impl VectorDocumentChange {
    fn document_id(&self) -> &DocumentId {
        match self {
            Self::Upsert(document) => &document.document_id,
            Self::Delete { document_id } => document_id,
        }
    }
}

/// Reproducible target and batching policy for one incremental update.
///
/// The generation is explicit so a delayed or retried job cannot silently
/// apply to a replacement generation. Callers can re-project the same desired
/// document state and submit it against the newly active generation instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorUpdateSpec {
    pub artifact_id: String,
    pub generation: String,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

const fn default_batch_size() -> usize {
    64
}

/// Durable job output/provenance suitable for persistence in a job result or
/// an artifact catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexBuildReport {
    pub artifact_id: String,
    pub generation: String,
    pub locator: String,
    pub backend_id: String,
    pub embedding_profile: String,
    pub embedding_model: String,
    pub embedding_revision: String,
    pub documents: usize,
    pub batches: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_generation: Option<String>,
}

/// Durable result of one incremental synchronization into an active
/// generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexUpdateReport {
    pub artifact_id: String,
    pub generation: String,
    pub backend_id: String,
    pub embedding_profile: String,
    pub embedding_model: String,
    pub embedding_revision: String,
    pub changes: usize,
    pub upserts: usize,
    pub deletes: usize,
    pub batches: usize,
    pub vector_count: usize,
}

/// Coordinates an embedding provider and a vector backend without knowing
/// either implementation. Share it through an [`Arc`] with durable job
/// handlers; rebuilds and updates are serialized per maintainer instance.
pub struct VectorIndexMaintainer {
    embedding: Arc<dyn EmbeddingProvider>,
    backend: Arc<dyn VectorBackend>,
    operation: Mutex<()>,
}

impl VectorIndexMaintainer {
    pub fn new(embedding: Arc<dyn EmbeddingProvider>, backend: Arc<dyn VectorBackend>) -> Self {
        Self {
            embedding,
            backend,
            operation: Mutex::new(()),
        }
    }

    pub fn embedding(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.embedding
    }

    pub fn backend(&self) -> &Arc<dyn VectorBackend> {
        &self.backend
    }

    /// Return the generation currently serving `artifact_id`, if any. The
    /// automatic maintenance scheduler resolves this at execution time so a
    /// delayed desired-state update follows a freshly rebuilt generation rather
    /// than failing permanently on a stale generation name.
    pub async fn active_generation(
        &self,
        artifact_id: &str,
    ) -> Result<Option<GenerationStatus>, SearchError> {
        Ok(self
            .backend
            .generations(artifact_id)
            .await?
            .into_iter()
            .find(|generation| generation.active))
    }

    /// Build and activate a complete generation. The document iterator is
    /// materialized so duplicate identities are rejected before any backend
    /// mutation and every embedding batch has a stable input/output ordering.
    pub async fn rebuild(
        &self,
        spec: VectorRebuildSpec,
        documents: impl IntoIterator<Item = VectorDocument>,
    ) -> Result<IndexBuildReport, SearchError> {
        let _operation = self.operation.lock().await;
        validate_rebuild_spec(&spec, self.embedding.as_ref(), self.backend.as_ref())?;
        let documents: Vec<_> = documents.into_iter().collect();
        reject_duplicate_documents(&documents)?;

        let prior = self
            .backend
            .generations(&spec.artifact_id)
            .await?
            .into_iter()
            .find(|generation| generation.active)
            .map(|generation| generation.handle.generation);
        let embedding = self.embedding.descriptor().clone();
        let handle = self
            .backend
            .create_generation(IndexGenerationSpec {
                artifact_id: spec.artifact_id.clone(),
                generation: spec.generation.clone(),
                vector_name: spec.vector_name.clone(),
                dimension: embedding.dimension,
                distance: spec.distance,
                embedding: Some(embedding_provenance(&embedding)),
                parameters: spec.backend_parameters.clone(),
            })
            .await?;

        let result = self.build_generation(&spec, &handle, &documents).await;
        if let Err(error) = result {
            if let Err(cleanup) = self.backend.delete_generation(&handle).await {
                return Err(SearchError::Backend(format!(
                    "{error}; additionally failed to remove incomplete generation: {cleanup}"
                )));
            }
            return Err(error);
        }

        Ok(IndexBuildReport {
            artifact_id: handle.artifact_id,
            generation: handle.generation,
            locator: handle.locator,
            backend_id: self.backend.descriptor().id.clone(),
            embedding_profile: embedding.id,
            embedding_model: embedding.model,
            embedding_revision: embedding.revision,
            documents: documents.len(),
            batches: documents.len().div_ceil(spec.batch_size),
            replaced_generation: prior,
        })
    }

    /// Synchronize selected documents into one explicitly named active
    /// generation without re-embedding the rest of the corpus.
    ///
    /// Each change expresses desired state, so replaying an upsert or deleting
    /// an already absent document is safe. A failure may leave a prefix of
    /// batches applied; retrying the complete update converges to the same
    /// result. Full rebuild remains the reconciliation and model-migration
    /// path.
    pub async fn update(
        &self,
        spec: VectorUpdateSpec,
        changes: impl IntoIterator<Item = VectorDocumentChange>,
    ) -> Result<IndexUpdateReport, SearchError> {
        let _operation = self.operation.lock().await;
        validate_update_spec(&spec, self.embedding.as_ref(), self.backend.as_ref())?;
        let changes = changes.into_iter().collect::<Vec<_>>();
        if changes.is_empty() {
            return Err(SearchError::InvalidRequest(
                "an incremental vector update requires at least one document change".to_owned(),
            ));
        }
        reject_duplicate_changes(&changes)?;

        let generation =
            active_generation(self.backend.as_ref(), &spec.artifact_id, &spec.generation).await?;
        validate_embedding_space(&generation, self.embedding.descriptor())?;

        let upserts = changes
            .iter()
            .filter(|change| matches!(change, VectorDocumentChange::Upsert(_)))
            .count();
        let deletes = changes.len() - upserts;
        if deletes > 0 && !self.backend.descriptor().capabilities.deletes {
            return Err(SearchError::Unsupported(format!(
                "vector backend {:?} does not support deletes",
                self.backend.descriptor().id
            )));
        }
        let batches = changes.len().div_ceil(spec.batch_size);

        for chunk in changes.chunks(spec.batch_size) {
            let documents = chunk
                .iter()
                .filter_map(|change| match change {
                    VectorDocumentChange::Upsert(document) => Some(document),
                    VectorDocumentChange::Delete { .. } => None,
                })
                .collect::<Vec<_>>();
            let vectors = if documents.is_empty() {
                Vec::new()
            } else {
                let output = self
                    .embedding
                    .embed(EmbeddingBatch {
                        profile: self.embedding.descriptor().id.clone(),
                        inputs: documents
                            .iter()
                            .map(|document| EmbeddingInput {
                                kind: EmbeddingInputKind::Document,
                                text: document.text.clone(),
                            })
                            .collect(),
                    })
                    .await?;
                if output.vectors.len() != documents.len() {
                    return Err(SearchError::Backend(format!(
                        "embedding profile {:?} returned {} vectors for {} inputs",
                        self.embedding.descriptor().id,
                        output.vectors.len(),
                        documents.len()
                    )));
                }
                output.vectors
            };

            let mut vectors = vectors.into_iter();
            let backend_changes = chunk
                .iter()
                .map(|change| match change {
                    VectorDocumentChange::Upsert(document) => VectorChange::Upsert {
                        document_id: document.document_id.clone(),
                        vectors: BTreeMap::from([(
                            generation.spec.vector_name.clone(),
                            convert_vector(
                                vectors
                                    .next()
                                    .expect("one embedding per validated upsert document"),
                            ),
                        )]),
                        payload: document.payload.clone(),
                    },
                    VectorDocumentChange::Delete { document_id } => VectorChange::Delete {
                        document_id: document_id.clone(),
                    },
                })
                .collect::<Vec<_>>();
            let receipt = self
                .backend
                .apply(&generation.handle, backend_changes)
                .await?;
            if receipt.applied != chunk.len() {
                return Err(SearchError::Backend(format!(
                    "vector backend {:?} acknowledged {} of {} incremental changes",
                    self.backend.descriptor().id,
                    receipt.applied,
                    chunk.len()
                )));
            }
        }
        self.backend.flush(&generation.handle).await?;

        let completed = active_generation(
            self.backend.as_ref(),
            &spec.artifact_id,
            &spec.generation,
        )
        .await
        .map_err(|error| {
            SearchError::Backend(format!(
                "incremental changes were applied, but the target generation was no longer active at completion: {error}"
            ))
        })?;
        let embedding = self.embedding.descriptor();
        Ok(IndexUpdateReport {
            artifact_id: spec.artifact_id,
            generation: spec.generation,
            backend_id: self.backend.descriptor().id.clone(),
            embedding_profile: embedding.id.clone(),
            embedding_model: embedding.model.clone(),
            embedding_revision: embedding.revision.clone(),
            changes: changes.len(),
            upserts,
            deletes,
            batches,
            vector_count: completed.vector_count,
        })
    }

    async fn build_generation(
        &self,
        spec: &VectorRebuildSpec,
        handle: &GenerationHandle,
        documents: &[VectorDocument],
    ) -> Result<(), SearchError> {
        for chunk in documents.chunks(spec.batch_size) {
            let output = self
                .embedding
                .embed(EmbeddingBatch {
                    profile: spec.embedding_profile.clone(),
                    inputs: chunk
                        .iter()
                        .map(|document| EmbeddingInput {
                            kind: EmbeddingInputKind::Document,
                            text: document.text.clone(),
                        })
                        .collect(),
                })
                .await?;
            if output.vectors.len() != chunk.len() {
                return Err(SearchError::Backend(format!(
                    "embedding profile {:?} returned {} vectors for {} inputs",
                    spec.embedding_profile,
                    output.vectors.len(),
                    chunk.len()
                )));
            }

            let changes = chunk
                .iter()
                .zip(output.vectors)
                .map(|(document, vector)| VectorChange::Upsert {
                    document_id: document.document_id.clone(),
                    vectors: BTreeMap::from([(spec.vector_name.clone(), convert_vector(vector))]),
                    payload: document.payload.clone(),
                })
                .collect();
            let receipt = self.backend.apply(handle, changes).await?;
            if receipt.applied != chunk.len() {
                return Err(SearchError::Backend(format!(
                    "vector backend {:?} acknowledged {} of {} upserts",
                    self.backend.descriptor().id,
                    receipt.applied,
                    chunk.len()
                )));
            }
        }

        self.backend.flush(handle).await?;
        self.backend.optimize(handle).await?;
        self.backend.activate(handle).await?;
        Ok(())
    }
}

fn validate_rebuild_spec(
    spec: &VectorRebuildSpec,
    embedding: &dyn EmbeddingProvider,
    backend: &dyn VectorBackend,
) -> Result<(), SearchError> {
    if spec.artifact_id.trim().is_empty()
        || spec.generation.trim().is_empty()
        || spec.vector_name.trim().is_empty()
    {
        return Err(SearchError::InvalidRequest(
            "artifact_id, generation, and vector_name cannot be empty".to_owned(),
        ));
    }
    if spec.batch_size == 0 {
        return Err(SearchError::InvalidRequest(
            "embedding batch_size must be greater than zero".to_owned(),
        ));
    }
    let descriptor = embedding.descriptor();
    if descriptor.id != spec.embedding_profile {
        return Err(SearchError::Configuration(format!(
            "rebuild requests embedding profile {:?}, but provider exposes {:?}",
            spec.embedding_profile, descriptor.id
        )));
    }
    if descriptor.dimension == 0 {
        return Err(SearchError::Configuration(format!(
            "embedding profile {:?} declares a zero dimension",
            descriptor.id
        )));
    }
    if !backend
        .descriptor()
        .capabilities
        .distances
        .contains(&spec.distance)
    {
        return Err(SearchError::Configuration(format!(
            "vector backend {:?} does not support {:?}",
            backend.descriptor().id,
            spec.distance
        )));
    }
    Ok(())
}

fn validate_update_spec(
    spec: &VectorUpdateSpec,
    embedding: &dyn EmbeddingProvider,
    backend: &dyn VectorBackend,
) -> Result<(), SearchError> {
    if spec.artifact_id.trim().is_empty() || spec.generation.trim().is_empty() {
        return Err(SearchError::InvalidRequest(
            "artifact_id and generation cannot be empty".to_owned(),
        ));
    }
    if spec.batch_size == 0 {
        return Err(SearchError::InvalidRequest(
            "embedding batch_size must be greater than zero".to_owned(),
        ));
    }
    if !backend.descriptor().capabilities.incremental_updates {
        return Err(SearchError::Unsupported(format!(
            "vector backend {:?} does not support incremental updates",
            backend.descriptor().id
        )));
    }
    if embedding.descriptor().dimension == 0 {
        return Err(SearchError::Configuration(format!(
            "embedding profile {:?} declares a zero dimension",
            embedding.descriptor().id
        )));
    }
    Ok(())
}

async fn active_generation(
    backend: &dyn VectorBackend,
    artifact_id: &str,
    generation: &str,
) -> Result<GenerationStatus, SearchError> {
    let statuses = backend.generations(artifact_id).await?;
    let target = statuses
        .iter()
        .find(|status| status.handle.generation == generation)
        .cloned()
        .ok_or_else(|| {
            SearchError::InvalidRequest(format!(
                "vector generation {artifact_id:?}/{generation:?} does not exist"
            ))
        })?;
    if !target.active {
        let active = statuses
            .iter()
            .find(|status| status.active)
            .map(|status| status.handle.generation.as_str());
        return Err(SearchError::InvalidRequest(format!(
            "vector generation {artifact_id:?}/{generation:?} is not active; active generation is {active:?}"
        )));
    }
    Ok(target)
}

fn validate_embedding_space(
    generation: &GenerationStatus,
    embedding: &EmbeddingDescriptor,
) -> Result<(), SearchError> {
    let expected = embedding_provenance(embedding);
    let actual = generation.spec.embedding.as_ref().ok_or_else(|| {
        SearchError::Configuration(format!(
            "vector generation {:?}/{:?} has no embedding provenance; rebuild it before applying incremental updates",
            generation.handle.artifact_id, generation.handle.generation
        ))
    })?;
    if actual != &expected || generation.spec.dimension != embedding.dimension {
        return Err(SearchError::Configuration(format!(
            "vector generation {:?}/{:?} uses embedding {:?}, dimension {}, but the maintainer provides {:?}, dimension {}",
            generation.handle.artifact_id,
            generation.handle.generation,
            actual,
            generation.spec.dimension,
            expected,
            embedding.dimension
        )));
    }
    Ok(())
}

fn embedding_provenance(descriptor: &EmbeddingDescriptor) -> EmbeddingProvenance {
    EmbeddingProvenance {
        profile: descriptor.id.clone(),
        provider: descriptor.provider.clone(),
        model: descriptor.model.clone(),
        revision: descriptor.revision.clone(),
        normalization: descriptor.normalization,
    }
}

fn reject_duplicate_documents(documents: &[VectorDocument]) -> Result<(), SearchError> {
    let mut seen = HashSet::with_capacity(documents.len());
    if let Some(duplicate) = documents
        .iter()
        .map(|document| &document.document_id)
        .find(|id| !seen.insert((*id).clone()))
    {
        return Err(SearchError::InvalidRequest(format!(
            "duplicate vector document id {:?}",
            duplicate.0
        )));
    }
    Ok(())
}

fn reject_duplicate_changes(changes: &[VectorDocumentChange]) -> Result<(), SearchError> {
    let mut seen = HashSet::with_capacity(changes.len());
    if let Some(duplicate) = changes
        .iter()
        .map(VectorDocumentChange::document_id)
        .find(|id| !seen.insert((*id).clone()))
    {
        return Err(SearchError::InvalidRequest(format!(
            "duplicate incremental vector document id {:?}",
            duplicate.0
        )));
    }
    Ok(())
}

fn convert_vector(vector: EmbeddingVector) -> VectorValue {
    match vector {
        EmbeddingVector::Dense(vector) => VectorValue::Dense(vector),
        EmbeddingVector::Sparse(vector) => VectorValue::Sparse(vector),
        EmbeddingVector::MultiDense(vector) => VectorValue::MultiDense(vector),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use sbol_db_search_sdk::{
        DataEgress, EmbeddingDescriptor, EmbeddingOutput, Normalization, VectorIndexAdmin,
    };
    use sbol_db_vector_flat::ExactFlatVectorBackend;
    use serde_json::json;

    use super::*;

    struct StubEmbedding {
        descriptor: EmbeddingDescriptor,
        batch_sizes: Mutex<Vec<usize>>,
        fail_on: Option<String>,
    }

    impl StubEmbedding {
        fn new(fail_on: Option<&str>) -> Self {
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
                batch_sizes: Mutex::new(Vec::new()),
                fail_on: fail_on.map(str::to_owned),
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for StubEmbedding {
        fn descriptor(&self) -> &EmbeddingDescriptor {
            &self.descriptor
        }

        async fn embed(
            &self,
            batch: EmbeddingBatch,
        ) -> Result<sbol_db_search_sdk::EmbeddingOutput, SearchError> {
            self.batch_sizes.lock().unwrap().push(batch.inputs.len());
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
                    .map(|input| {
                        if input.text.contains("promoter") {
                            EmbeddingVector::Dense(vec![1.0, 0.0])
                        } else {
                            EmbeddingVector::Dense(vec![0.0, 1.0])
                        }
                    })
                    .collect(),
            })
        }
    }

    fn spec(generation: &str) -> VectorRebuildSpec {
        VectorRebuildSpec {
            artifact_id: "parts".to_owned(),
            generation: generation.to_owned(),
            vector_name: "content".to_owned(),
            embedding_profile: "test.embedding.v1".to_owned(),
            distance: DistanceMetric::Cosine,
            batch_size: 2,
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
    async fn rebuild_batches_documents_and_activates_only_when_complete() {
        let embedding = Arc::new(StubEmbedding::new(None));
        let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
        let maintainer = VectorIndexMaintainer::new(embedding.clone(), backend.clone());
        let report = maintainer
            .rebuild(
                spec("g1"),
                vec![
                    document("a", "promoter alpha"),
                    document("b", "coding beta"),
                    document("c", "promoter gamma"),
                ],
            )
            .await
            .unwrap();

        assert_eq!(*embedding.batch_sizes.lock().unwrap(), vec![2, 1]);
        assert_eq!(report.documents, 3);
        assert_eq!(report.batches, 2);
        assert_eq!(report.embedding_revision, "sha256:test");
        assert!(backend.generations("parts").await.unwrap()[0].active);
    }

    #[tokio::test]
    async fn failed_rebuild_preserves_prior_generation_and_cleans_up() {
        let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
        let initial =
            VectorIndexMaintainer::new(Arc::new(StubEmbedding::new(None)), backend.clone());
        initial
            .rebuild(spec("g1"), vec![document("a", "promoter")])
            .await
            .unwrap();

        let failing =
            VectorIndexMaintainer::new(Arc::new(StubEmbedding::new(Some("fail"))), backend.clone());
        assert!(failing
            .rebuild(
                spec("g2"),
                vec![document("b", "coding"), document("c", "fail")],
            )
            .await
            .is_err());

        let generations = backend.generations("parts").await.unwrap();
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0].handle.generation, "g1");
        assert!(generations[0].active);
    }

    #[tokio::test]
    async fn invalid_or_duplicate_inputs_fail_before_creating_a_generation() {
        let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
        let maintainer =
            VectorIndexMaintainer::new(Arc::new(StubEmbedding::new(None)), backend.clone());
        let mut invalid_profile = spec("g1");
        invalid_profile.embedding_profile = "missing".to_owned();
        assert!(matches!(
            maintainer.rebuild(invalid_profile, Vec::new()).await,
            Err(SearchError::Configuration(_))
        ));
        assert!(maintainer
            .rebuild(
                spec("g1"),
                vec![document("same", "one"), document("same", "two")],
            )
            .await
            .is_err());
        assert!(backend.generations("parts").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_embeds_upserts_deletes_idempotently_and_reports_active_state() {
        let embedding = Arc::new(StubEmbedding::new(None));
        let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
        let maintainer = VectorIndexMaintainer::new(embedding.clone(), backend.clone());
        maintainer
            .rebuild(
                spec("g1"),
                vec![document("a", "promoter"), document("b", "coding")],
            )
            .await
            .unwrap();

        let report = maintainer
            .update(
                VectorUpdateSpec {
                    artifact_id: "parts".to_owned(),
                    generation: "g1".to_owned(),
                    batch_size: 2,
                },
                vec![
                    VectorDocumentChange::Upsert(document("a", "coding revised")),
                    VectorDocumentChange::Delete {
                        document_id: DocumentId("b".to_owned()),
                    },
                    VectorDocumentChange::Delete {
                        document_id: DocumentId("already-absent".to_owned()),
                    },
                    VectorDocumentChange::Upsert(document("d", "promoter new")),
                ],
            )
            .await
            .unwrap();

        assert_eq!(*embedding.batch_sizes.lock().unwrap(), vec![2, 1, 1]);
        assert_eq!(report.changes, 4);
        assert_eq!(report.upserts, 2);
        assert_eq!(report.deletes, 2);
        assert_eq!(report.batches, 2);
        assert_eq!(report.vector_count, 2);
        assert_eq!(report.generation, "g1");
        assert_eq!(report.embedding_revision, "sha256:test");
    }

    #[tokio::test]
    async fn update_rejects_stale_generation_and_duplicate_ids_before_embedding() {
        let embedding = Arc::new(StubEmbedding::new(None));
        let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
        let maintainer = VectorIndexMaintainer::new(embedding.clone(), backend);
        maintainer
            .rebuild(spec("g1"), vec![document("a", "promoter")])
            .await
            .unwrap();
        maintainer
            .rebuild(spec("g2"), vec![document("a", "promoter")])
            .await
            .unwrap();

        let stale = maintainer
            .update(
                VectorUpdateSpec {
                    artifact_id: "parts".to_owned(),
                    generation: "g1".to_owned(),
                    batch_size: 2,
                },
                vec![VectorDocumentChange::Upsert(document("b", "coding"))],
            )
            .await;
        assert!(matches!(stale, Err(SearchError::InvalidRequest(_))));

        let duplicate = maintainer
            .update(
                VectorUpdateSpec {
                    artifact_id: "parts".to_owned(),
                    generation: "g2".to_owned(),
                    batch_size: 2,
                },
                vec![
                    VectorDocumentChange::Upsert(document("same", "one")),
                    VectorDocumentChange::Delete {
                        document_id: DocumentId("same".to_owned()),
                    },
                ],
            )
            .await;
        assert!(matches!(duplicate, Err(SearchError::InvalidRequest(_))));
        assert_eq!(*embedding.batch_sizes.lock().unwrap(), vec![1, 1]);
    }
}
