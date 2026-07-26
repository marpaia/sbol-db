use std::collections::{BTreeMap, HashMap};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_trait::async_trait;
use sbol_db_search_sdk::{
    ApplyReceipt, DistanceMetric, FilterCapability, GenerationHandle, GenerationStatus,
    IndexGenerationSpec, SnapshotRef, VectorBackendDescriptor, VectorCapabilities, VectorChange,
    VectorError, VectorIndexAdmin, VectorQuery, VectorSearchHit, VectorSearchPage, VectorSearcher,
    VectorValue,
};
use serde_json::Value;

use crate::filter;

type GenerationKey = (String, String);

#[derive(Clone)]
struct StoredPoint {
    vectors: BTreeMap<String, Vec<f32>>,
    payload: BTreeMap<String, Value>,
}

struct Generation {
    handle: GenerationHandle,
    spec: IndexGenerationSpec,
    points: BTreeMap<sbol_db_search_sdk::DocumentId, StoredPoint>,
}

#[derive(Default)]
struct State {
    generations: HashMap<GenerationKey, Generation>,
    active: HashMap<String, String>,
}

/// An exact, deterministic, in-memory implementation of the vector plugin
/// contract. It is suitable as an embedded backend for small corpora and as a
/// recall oracle for approximate backends. It is not persistent.
pub struct ExactFlatVectorBackend {
    descriptor: VectorBackendDescriptor,
    state: RwLock<State>,
}

impl ExactFlatVectorBackend {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            descriptor: VectorBackendDescriptor {
                id: id.into(),
                kind: "exact_flat".to_owned(),
                remote: false,
                capabilities: VectorCapabilities {
                    dense: true,
                    sparse: false,
                    multi_dense: false,
                    distances: vec![
                        DistanceMetric::Cosine,
                        DistanceMetric::Dot,
                        DistanceMetric::Euclidean,
                        DistanceMetric::Manhattan,
                    ],
                    filter_execution: FilterCapability::Native,
                    persistent: false,
                    incremental_updates: true,
                    deletes: true,
                    atomic_activation: true,
                    snapshots: false,
                },
            },
            state: RwLock::new(State::default()),
        }
    }

    fn read_state(&self) -> Result<RwLockReadGuard<'_, State>, VectorError> {
        self.state
            .read()
            .map_err(|_| VectorError::Backend("exact-flat state lock is poisoned".to_owned()))
    }

    fn write_state(&self) -> Result<RwLockWriteGuard<'_, State>, VectorError> {
        self.state
            .write()
            .map_err(|_| VectorError::Backend("exact-flat state lock is poisoned".to_owned()))
    }
}

#[async_trait]
impl VectorSearcher for ExactFlatVectorBackend {
    fn descriptor(&self) -> &VectorBackendDescriptor {
        &self.descriptor
    }

    async fn query(&self, query: VectorQuery) -> Result<VectorSearchPage, VectorError> {
        validate_query(&query)?;
        let query_vector = match &query.vector {
            VectorValue::Dense(vector) => vector,
            _ => {
                return Err(VectorError::Unsupported(
                    "exact-flat only supports dense query vectors".to_owned(),
                ));
            }
        };

        let state = self.read_state()?;
        let active_generation = state.active.get(&query.index).ok_or_else(|| {
            VectorError::Configuration(format!(
                "vector artifact {:?} has no active generation",
                query.index
            ))
        })?;
        let key = (query.index.clone(), active_generation.clone());
        let generation = state.generations.get(&key).ok_or_else(|| {
            VectorError::Backend(format!(
                "active generation {:?}/{:?} is missing",
                key.0, key.1
            ))
        })?;

        validate_dense_vector(query_vector, generation.spec.dimension, "query")?;
        validate_metric_vector(query_vector, generation.spec.distance, "query")?;

        let mut hits = Vec::new();
        for (document_id, point) in &generation.points {
            if let Some(filter) = &query.filter {
                if !filter::matches(&point.payload, filter)? {
                    continue;
                }
            }
            let Some(vector) = point.vectors.get(&query.vector_name) else {
                continue;
            };
            let score = score(query_vector, vector, generation.spec.distance);
            if query
                .score_threshold
                .is_some_and(|threshold| score < threshold)
            {
                continue;
            }
            hits.push(VectorSearchHit {
                document_id: document_id.clone(),
                score,
            });
        }

        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.document_id.cmp(&right.document_id))
        });

        let offset = query
            .cursor
            .as_deref()
            .map(parse_cursor)
            .transpose()?
            .unwrap_or(0);
        let end = offset.saturating_add(query.limit).min(hits.len());
        let items = hits.get(offset..end).unwrap_or_default().to_vec();
        let next_cursor = (end < hits.len()).then(|| end.to_string());

        Ok(VectorSearchPage { items, next_cursor })
    }
}

#[async_trait]
impl VectorIndexAdmin for ExactFlatVectorBackend {
    async fn create_generation(
        &self,
        spec: IndexGenerationSpec,
    ) -> Result<GenerationHandle, VectorError> {
        validate_spec(&spec, &self.descriptor)?;
        let key = (spec.artifact_id.clone(), spec.generation.clone());
        let handle = GenerationHandle {
            artifact_id: key.0.clone(),
            generation: key.1.clone(),
            locator: format!("memory://{}/{}", key.0, key.1),
        };
        let generation = Generation {
            handle: handle.clone(),
            spec,
            points: BTreeMap::new(),
        };
        let mut state = self.write_state()?;
        if state.generations.contains_key(&key) {
            return Err(VectorError::InvalidRequest(format!(
                "generation {:?}/{:?} already exists",
                key.0, key.1
            )));
        }
        state.generations.insert(key, generation);
        Ok(handle)
    }

    async fn apply(
        &self,
        generation: &GenerationHandle,
        changes: Vec<VectorChange>,
    ) -> Result<ApplyReceipt, VectorError> {
        let mut state = self.write_state()?;
        let stored_generation = generation_mut(&mut state, generation)?;
        let mut updated = stored_generation.points.clone();
        let mut applied = 0;

        for change in changes {
            match change {
                VectorChange::Upsert {
                    document_id,
                    vectors,
                    payload,
                } => {
                    if vectors.is_empty() {
                        return Err(VectorError::InvalidRequest(format!(
                            "document {:?} has no vectors",
                            document_id.0
                        )));
                    }
                    let mut dense_vectors = BTreeMap::new();
                    for (name, value) in vectors {
                        if name.trim().is_empty() {
                            return Err(VectorError::InvalidRequest(
                                "vector name cannot be empty".to_owned(),
                            ));
                        }
                        let VectorValue::Dense(vector) = value else {
                            return Err(VectorError::Unsupported(
                                "exact-flat only supports dense stored vectors".to_owned(),
                            ));
                        };
                        validate_dense_vector(&vector, stored_generation.spec.dimension, "stored")?;
                        validate_metric_vector(&vector, stored_generation.spec.distance, "stored")?;
                        dense_vectors.insert(name, vector);
                    }
                    updated.insert(
                        document_id,
                        StoredPoint {
                            vectors: dense_vectors,
                            payload,
                        },
                    );
                    applied += 1;
                }
                VectorChange::Delete { document_id } => {
                    if updated.remove(&document_id).is_some() {
                        applied += 1;
                    }
                }
            }
        }

        stored_generation.points = updated;
        Ok(ApplyReceipt { applied })
    }

    async fn flush(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let state = self.read_state()?;
        generation_ref(&state, generation)?;
        Ok(())
    }

    async fn optimize(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let state = self.read_state()?;
        generation_ref(&state, generation)?;
        Ok(())
    }

    async fn snapshot(&self, _generation: &GenerationHandle) -> Result<SnapshotRef, VectorError> {
        Err(VectorError::Unsupported(
            "exact-flat is in-memory and cannot create durable snapshots".to_owned(),
        ))
    }

    async fn activate(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let mut state = self.write_state()?;
        generation_ref(&state, generation)?;
        state.active.insert(
            generation.artifact_id.clone(),
            generation.generation.clone(),
        );
        Ok(())
    }

    async fn generations(&self, artifact_id: &str) -> Result<Vec<GenerationStatus>, VectorError> {
        let state = self.read_state()?;
        let active = state.active.get(artifact_id);
        let mut statuses: Vec<_> = state
            .generations
            .values()
            .filter(|generation| generation.spec.artifact_id == artifact_id)
            .map(|generation| GenerationStatus {
                handle: generation.handle.clone(),
                spec: generation.spec.clone(),
                active: active == Some(&generation.spec.generation),
                vector_count: generation.points.len(),
            })
            .collect();
        statuses.sort_by(|left, right| left.handle.generation.cmp(&right.handle.generation));
        Ok(statuses)
    }

    async fn delete_generation(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let mut state = self.write_state()?;
        generation_ref(&state, generation)?;
        if state.active.get(&generation.artifact_id) == Some(&generation.generation) {
            return Err(VectorError::InvalidRequest(format!(
                "cannot delete active generation {:?}/{:?}",
                generation.artifact_id, generation.generation
            )));
        }
        state.generations.remove(&generation_key(generation));
        Ok(())
    }
}

fn validate_spec(
    spec: &IndexGenerationSpec,
    descriptor: &VectorBackendDescriptor,
) -> Result<(), VectorError> {
    if spec.artifact_id.trim().is_empty() || spec.generation.trim().is_empty() {
        return Err(VectorError::InvalidRequest(
            "artifact_id and generation cannot be empty".to_owned(),
        ));
    }
    if spec.dimension == 0 {
        return Err(VectorError::InvalidRequest(
            "vector dimension must be greater than zero".to_owned(),
        ));
    }
    if !descriptor.capabilities.distances.contains(&spec.distance) {
        return Err(VectorError::Unsupported(format!(
            "distance {:?} is not supported by exact-flat",
            spec.distance
        )));
    }
    if !spec.parameters.is_empty() {
        return Err(VectorError::Unsupported(format!(
            "unsupported exact-flat generation parameters: {:?}",
            spec.parameters.keys().collect::<Vec<_>>()
        )));
    }
    Ok(())
}

fn validate_query(query: &VectorQuery) -> Result<(), VectorError> {
    if query.index.trim().is_empty() || query.vector_name.trim().is_empty() {
        return Err(VectorError::InvalidRequest(
            "index and vector_name cannot be empty".to_owned(),
        ));
    }
    if query.limit == 0 {
        return Err(VectorError::InvalidRequest(
            "vector query limit must be greater than zero".to_owned(),
        ));
    }
    if query
        .score_threshold
        .is_some_and(|value| !value.is_finite())
    {
        return Err(VectorError::InvalidRequest(
            "score_threshold must be finite".to_owned(),
        ));
    }
    if !query.parameters.is_empty() {
        return Err(VectorError::Unsupported(format!(
            "unsupported exact-flat query parameters: {:?}",
            query.parameters.keys().collect::<Vec<_>>()
        )));
    }
    Ok(())
}

fn validate_dense_vector(
    vector: &[f32],
    expected_dimension: usize,
    label: &str,
) -> Result<(), VectorError> {
    if vector.len() != expected_dimension {
        return Err(VectorError::InvalidRequest(format!(
            "{label} vector dimension {} does not match index dimension {expected_dimension}",
            vector.len()
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(VectorError::InvalidRequest(format!(
            "{label} vector contains a non-finite value"
        )));
    }
    Ok(())
}

fn validate_metric_vector(
    vector: &[f32],
    metric: DistanceMetric,
    label: &str,
) -> Result<(), VectorError> {
    if metric == DistanceMetric::Cosine && squared_norm(vector) == 0.0 {
        return Err(VectorError::InvalidRequest(format!(
            "{label} cosine vector cannot have zero magnitude"
        )));
    }
    Ok(())
}

fn parse_cursor(value: &str) -> Result<usize, VectorError> {
    value.parse().map_err(|_| {
        VectorError::InvalidRequest("exact-flat cursor must be a non-negative integer".to_owned())
    })
}

fn generation_key(handle: &GenerationHandle) -> GenerationKey {
    (handle.artifact_id.clone(), handle.generation.clone())
}

fn generation_ref<'a>(
    state: &'a State,
    handle: &GenerationHandle,
) -> Result<&'a Generation, VectorError> {
    let generation = state
        .generations
        .get(&generation_key(handle))
        .ok_or_else(|| {
            VectorError::InvalidRequest(format!(
                "unknown generation {:?}/{:?}",
                handle.artifact_id, handle.generation
            ))
        })?;
    if generation.handle != *handle {
        return Err(VectorError::InvalidRequest(
            "generation handle locator does not match".to_owned(),
        ));
    }
    Ok(generation)
}

fn generation_mut<'a>(
    state: &'a mut State,
    handle: &GenerationHandle,
) -> Result<&'a mut Generation, VectorError> {
    let generation = state
        .generations
        .get_mut(&generation_key(handle))
        .ok_or_else(|| {
            VectorError::InvalidRequest(format!(
                "unknown generation {:?}/{:?}",
                handle.artifact_id, handle.generation
            ))
        })?;
    if generation.handle != *handle {
        return Err(VectorError::InvalidRequest(
            "generation handle locator does not match".to_owned(),
        ));
    }
    Ok(generation)
}

fn score(left: &[f32], right: &[f32], metric: DistanceMetric) -> f32 {
    match metric {
        DistanceMetric::Cosine => {
            dot(left, right) / (squared_norm(left).sqrt() * squared_norm(right).sqrt())
        }
        DistanceMetric::Dot => dot(left, right),
        DistanceMetric::Euclidean => -left
            .iter()
            .zip(right)
            .map(|(left, right)| (left - right).powi(2))
            .sum::<f32>()
            .sqrt(),
        DistanceMetric::Manhattan => -left
            .iter()
            .zip(right)
            .map(|(left, right)| (left - right).abs())
            .sum::<f32>(),
        DistanceMetric::Hamming | DistanceMetric::Jaccard => {
            unreachable!("unsupported metrics are rejected when generations are created")
        }
    }
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

fn squared_norm(vector: &[f32]) -> f32 {
    dot(vector, vector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbol_db_search_sdk::DocumentId;
    use serde_json::json;

    fn spec(generation: &str, distance: DistanceMetric) -> IndexGenerationSpec {
        IndexGenerationSpec {
            artifact_id: "parts".to_owned(),
            generation: generation.to_owned(),
            dimension: 2,
            distance,
            parameters: BTreeMap::new(),
        }
    }

    fn upsert(id: &str, vector: [f32; 2], graph: &str) -> VectorChange {
        VectorChange::Upsert {
            document_id: DocumentId(id.to_owned()),
            vectors: BTreeMap::from([("content".to_owned(), VectorValue::Dense(vector.to_vec()))]),
            payload: BTreeMap::from([("graph".to_owned(), json!(graph))]),
        }
    }

    fn query(filter: Option<sbol_db_search_sdk::VectorFilter>) -> VectorQuery {
        VectorQuery {
            index: "parts".to_owned(),
            vector_name: "content".to_owned(),
            vector: VectorValue::Dense(vec![1.0, 0.0]),
            filter,
            limit: 1,
            cursor: None,
            score_threshold: None,
            parameters: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn generation_activation_is_atomic_and_rollbackable() {
        let backend = ExactFlatVectorBackend::new("local");
        let first = backend
            .create_generation(spec("g1", DistanceMetric::Cosine))
            .await
            .unwrap();
        backend
            .apply(
                &first,
                vec![
                    upsert("alpha", [1.0, 0.0], "public"),
                    upsert("beta", [0.0, 1.0], "private"),
                ],
            )
            .await
            .unwrap();
        assert!(matches!(
            backend.query(query(None)).await,
            Err(VectorError::Configuration(_))
        ));

        backend.activate(&first).await.unwrap();
        let public_only = sbol_db_search_sdk::VectorFilter::Match {
            field: "graph".to_owned(),
            value: json!("public"),
        };
        let page = backend.query(query(Some(public_only))).await.unwrap();
        assert_eq!(page.items[0].document_id, DocumentId("alpha".to_owned()));

        let second = backend
            .create_generation(spec("g2", DistanceMetric::Cosine))
            .await
            .unwrap();
        backend
            .apply(&second, vec![upsert("gamma", [1.0, 0.0], "public")])
            .await
            .unwrap();
        backend.activate(&second).await.unwrap();
        assert_eq!(
            backend.query(query(None)).await.unwrap().items[0].document_id,
            DocumentId("gamma".to_owned())
        );

        let statuses = backend.generations("parts").await.unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(!statuses[0].active);
        assert!(statuses[1].active);
        assert_eq!(statuses[0].vector_count, 2);

        assert!(backend.delete_generation(&second).await.is_err());
        backend.activate(&first).await.unwrap();
        backend.delete_generation(&second).await.unwrap();
        assert_eq!(backend.generations("parts").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn exact_scores_are_stable_and_cursor_pagination_is_deterministic() {
        let backend = ExactFlatVectorBackend::new("local");
        let generation = backend
            .create_generation(spec("g1", DistanceMetric::Dot))
            .await
            .unwrap();
        backend
            .apply(
                &generation,
                vec![
                    upsert("alpha", [1.0, 0.0], "public"),
                    upsert("beta", [0.5, 0.0], "public"),
                    upsert("gamma", [0.5, 0.0], "public"),
                ],
            )
            .await
            .unwrap();
        backend.activate(&generation).await.unwrap();

        let first = backend.query(query(None)).await.unwrap();
        assert_eq!(first.items[0].document_id, DocumentId("alpha".to_owned()));
        assert_eq!(first.items[0].score, 1.0);
        assert_eq!(first.next_cursor.as_deref(), Some("1"));

        let mut second_query = query(None);
        second_query.cursor = Some("1".to_owned());
        let second = backend.query(second_query).await.unwrap();
        assert_eq!(second.items[0].document_id, DocumentId("beta".to_owned()));
    }

    #[tokio::test]
    async fn rejected_batch_does_not_partially_mutate_generation() {
        let backend = ExactFlatVectorBackend::new("local");
        let generation = backend
            .create_generation(spec("g1", DistanceMetric::Dot))
            .await
            .unwrap();
        let result = backend
            .apply(
                &generation,
                vec![
                    upsert("valid", [1.0, 0.0], "public"),
                    VectorChange::Upsert {
                        document_id: DocumentId("invalid".to_owned()),
                        vectors: BTreeMap::from([(
                            "content".to_owned(),
                            VectorValue::Dense(vec![1.0]),
                        )]),
                        payload: BTreeMap::new(),
                    },
                ],
            )
            .await;
        assert!(matches!(result, Err(VectorError::InvalidRequest(_))));
        assert_eq!(
            backend.generations("parts").await.unwrap()[0].vector_count,
            0
        );
    }
}
