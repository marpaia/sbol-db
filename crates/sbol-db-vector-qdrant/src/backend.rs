use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use qdrant_client::qdrant::{
    CreateCollectionBuilder, DeletePointsBuilder, Distance, NamedVectors, PointStruct,
    QueryPointsBuilder, VectorParamsBuilder, VectorsConfigBuilder,
};
use qdrant_client::{Payload, Qdrant};
use sbol_db_search_sdk::{
    ApplyReceipt, DistanceMetric, DocumentId, FilterCapability, GenerationHandle, GenerationStatus,
    IndexGenerationSpec, SnapshotRef, VectorBackendDescriptor, VectorCapabilities, VectorChange,
    VectorError, VectorIndexAdmin, VectorQuery, VectorSearchHit, VectorSearchPage, VectorSearcher,
    VectorValue,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha3::{Digest, Sha3_256};
use uuid::Uuid;

use crate::filter;

const GENERATION_SEPARATOR: &str = "__gen__";
const SPEC_METADATA_KEY: &str = "sbol_db_generation_spec";
const DOCUMENT_ID_PAYLOAD: &str = "__sbol_db_document_id";
const SUPPORTED_PARAMETERS: &[&str] = &[
    "on_disk",
    "on_disk_payload",
    "shard_number",
    "replication_factor",
    "write_consistency_factor",
];

/// Connection and namespace settings for a Qdrant server or Qdrant Cloud.
///
/// Qdrant exposes gRPC and REST on separate endpoints. Data-plane operations
/// use the official Rust gRPC client; REST is used only for a multi-action,
/// atomic collection-alias update because the high-level Rust client exposes
/// only one alias action at a time.
#[derive(Clone, Serialize, Deserialize)]
pub struct QdrantRemoteConfig {
    pub id: String,
    pub grpc_url: String,
    pub rest_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default = "default_collection_prefix")]
    pub collection_prefix: String,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl fmt::Debug for QdrantRemoteConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QdrantRemoteConfig")
            .field("id", &self.id)
            .field("grpc_url", &self.grpc_url)
            .field("rest_url", &self.rest_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field("collection_prefix", &self.collection_prefix)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

fn default_collection_prefix() -> String {
    "sbol".to_owned()
}

const fn default_timeout_seconds() -> u64 {
    30
}

/// Persistent remote vector backend for Qdrant self-hosted and cloud.
pub struct QdrantRemoteBackend {
    descriptor: VectorBackendDescriptor,
    client: Qdrant,
    http: reqwest::Client,
    config: QdrantRemoteConfig,
}

impl QdrantRemoteBackend {
    pub fn new(mut config: QdrantRemoteConfig) -> Result<Self, VectorError> {
        validate_identifier("backend id", &config.id)?;
        validate_identifier("collection prefix", &config.collection_prefix)?;
        if config.grpc_url.trim().is_empty() || config.rest_url.trim().is_empty() {
            return Err(VectorError::Configuration(
                "Qdrant grpc_url and rest_url cannot be empty".to_owned(),
            ));
        }
        if config.timeout_seconds == 0 {
            return Err(VectorError::Configuration(
                "Qdrant timeout_seconds must be greater than zero".to_owned(),
            ));
        }
        config.rest_url = config.rest_url.trim_end_matches('/').to_owned();

        let timeout = Duration::from_secs(config.timeout_seconds);
        let client = Qdrant::from_url(&config.grpc_url)
            .api_key(config.api_key.clone())
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .map_err(configuration_error)?;
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(configuration_error)?;
        let descriptor = VectorBackendDescriptor {
            id: config.id.clone(),
            kind: "qdrant".to_owned(),
            remote: true,
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
                persistent: true,
                incremental_updates: true,
                deletes: true,
                atomic_activation: true,
                snapshots: true,
            },
        };

        Ok(Self {
            descriptor,
            client,
            http,
            config,
        })
    }

    fn alias_name(&self, artifact_id: &str) -> Result<String, VectorError> {
        validate_identifier("artifact id", artifact_id)?;
        checked_collection_name(format!("{}__{artifact_id}", self.config.collection_prefix))
    }

    fn collection_name(&self, artifact_id: &str, generation: &str) -> Result<String, VectorError> {
        validate_identifier("generation", generation)?;
        checked_collection_name(format!(
            "{}{GENERATION_SEPARATOR}{generation}",
            self.alias_name(artifact_id)?
        ))
    }

    fn validate_handle(&self, handle: &GenerationHandle) -> Result<String, VectorError> {
        let expected = self.collection_name(&handle.artifact_id, &handle.generation)?;
        if handle.locator != expected {
            return Err(VectorError::InvalidRequest(format!(
                "generation locator {:?} does not match expected Qdrant collection {expected:?}",
                handle.locator
            )));
        }
        Ok(expected)
    }

    async fn load_spec(
        &self,
        collection: &str,
    ) -> Result<(IndexGenerationSpec, usize), VectorError> {
        let info = self
            .client
            .collection_info(collection)
            .await
            .map_err(backend_error)?;
        let result = info.result.ok_or_else(|| {
            VectorError::Backend(format!(
                "Qdrant returned no collection information for {collection:?}"
            ))
        })?;
        let count = usize::try_from(result.points_count.unwrap_or(0)).unwrap_or(usize::MAX);
        let metadata = result
            .config
            .and_then(|config| config.metadata.get(SPEC_METADATA_KEY).cloned())
            .ok_or_else(|| {
                VectorError::Backend(format!(
                    "Qdrant collection {collection:?} lacks sbol-db generation metadata"
                ))
            })?;
        let spec = serde_json::from_value(metadata.into_json()).map_err(|error| {
            VectorError::Backend(format!(
                "invalid sbol-db generation metadata on Qdrant collection {collection:?}: {error}"
            ))
        })?;
        Ok((spec, count))
    }

    async fn active_collection(&self, artifact_id: &str) -> Result<Option<String>, VectorError> {
        let alias = self.alias_name(artifact_id)?;
        Ok(self
            .client
            .list_aliases()
            .await
            .map_err(backend_error)?
            .aliases
            .into_iter()
            .find(|candidate| candidate.alias_name == alias)
            .map(|candidate| candidate.collection_name))
    }

    async fn activate_alias(&self, alias: &str, collection: &str) -> Result<(), VectorError> {
        let current = self
            .client
            .list_aliases()
            .await
            .map_err(backend_error)?
            .aliases
            .into_iter()
            .find(|candidate| candidate.alias_name == alias)
            .map(|candidate| candidate.collection_name);
        if current.as_deref() == Some(collection) {
            return Ok(());
        }

        let mut actions = Vec::with_capacity(2);
        if current.is_some() {
            actions.push(json!({"delete_alias": {"alias_name": alias}}));
        }
        actions.push(json!({
            "create_alias": {"collection_name": collection, "alias_name": alias}
        }));

        let mut request = self
            .http
            .post(format!("{}/collections/aliases", self.config.rest_url))
            .json(&json!({"actions": actions}));
        if let Some(api_key) = &self.config.api_key {
            request = request.header("api-key", api_key);
        }
        request
            .send()
            .await
            .map_err(backend_error)?
            .error_for_status()
            .map_err(backend_error)?;
        Ok(())
    }
}

#[async_trait]
impl VectorSearcher for QdrantRemoteBackend {
    fn descriptor(&self) -> &VectorBackendDescriptor {
        &self.descriptor
    }

    async fn query(&self, query: VectorQuery) -> Result<VectorSearchPage, VectorError> {
        validate_query(&query)?;
        let VectorValue::Dense(vector) = query.vector else {
            return Err(VectorError::Unsupported(
                "Qdrant adapter currently supports dense query vectors".to_owned(),
            ));
        };
        validate_dense(&vector, "query")?;
        let collection = self.alias_name(&query.index)?;
        let (spec, _) = self.load_spec(&collection).await?;
        if query.vector_name != spec.vector_name {
            return Err(VectorError::InvalidRequest(format!(
                "query vector {:?} does not match generation vector {:?}",
                query.vector_name, spec.vector_name
            )));
        }
        if vector.len() != spec.dimension {
            return Err(VectorError::InvalidRequest(format!(
                "query vector dimension {} does not match index dimension {}",
                vector.len(),
                spec.dimension
            )));
        }
        if spec.distance == DistanceMetric::Cosine
            && vector.iter().map(|value| value * value).sum::<f32>() == 0.0
        {
            return Err(VectorError::InvalidRequest(
                "query cosine vector cannot have zero magnitude".to_owned(),
            ));
        }
        let offset = query
            .cursor
            .as_deref()
            .map(parse_cursor)
            .transpose()?
            .unwrap_or(0);
        let fetch_limit = query.limit.checked_add(1).ok_or_else(|| {
            VectorError::InvalidRequest("vector query limit is too large".to_owned())
        })?;
        let mut request = QueryPointsBuilder::new(collection)
            .query(vector)
            .using(query.vector_name)
            .offset(u64::try_from(offset).map_err(|_| {
                VectorError::InvalidRequest("vector query cursor is too large".to_owned())
            })?)
            .limit(u64::try_from(fetch_limit).map_err(|_| {
                VectorError::InvalidRequest("vector query limit is too large".to_owned())
            })?)
            .with_payload(true);
        if let Some(filter) = query.filter.as_ref() {
            request = request.filter(filter::translate(filter)?);
        }
        if let Some(threshold) = query.score_threshold {
            request = request.score_threshold(qdrant_score_threshold(threshold, spec.distance));
        }

        let response = self.client.query(request).await.map_err(backend_error)?;
        let mut items = response
            .result
            .into_iter()
            .map(|point| {
                let document_id = point
                    .payload
                    .get(DOCUMENT_ID_PAYLOAD)
                    .and_then(|value| value.as_str())
                    .cloned()
                    .ok_or_else(|| {
                        VectorError::Backend(format!(
                            "Qdrant result is missing string payload {DOCUMENT_ID_PAYLOAD:?}"
                        ))
                    })?;
                Ok(VectorSearchHit {
                    document_id: DocumentId(document_id),
                    score: portable_score(point.score, spec.distance),
                })
            })
            .collect::<Result<Vec<_>, VectorError>>()?;
        let has_more = items.len() > query.limit;
        items.truncate(query.limit);
        let next_cursor = has_more.then(|| offset.saturating_add(items.len()).to_string());
        Ok(VectorSearchPage { items, next_cursor })
    }
}

#[async_trait]
impl VectorIndexAdmin for QdrantRemoteBackend {
    async fn create_generation(
        &self,
        spec: IndexGenerationSpec,
    ) -> Result<GenerationHandle, VectorError> {
        validate_spec(&spec, &self.descriptor)?;
        let collection = self.collection_name(&spec.artifact_id, &spec.generation)?;
        if self
            .client
            .collection_exists(&collection)
            .await
            .map_err(backend_error)?
        {
            return Err(VectorError::InvalidRequest(format!(
                "generation {:?}/{:?} already exists",
                spec.artifact_id, spec.generation
            )));
        }

        let mut vector_params = VectorParamsBuilder::new(
            u64::try_from(spec.dimension).map_err(|_| {
                VectorError::InvalidRequest("vector dimension is too large".to_owned())
            })?,
            qdrant_distance(spec.distance),
        );
        if let Some(value) = bool_parameter(&spec.parameters, "on_disk")? {
            vector_params = vector_params.on_disk(value);
        }
        let mut vectors = VectorsConfigBuilder::default();
        vectors.add_named_vector_params(&spec.vector_name, vector_params);
        let metadata = HashMap::from([(
            SPEC_METADATA_KEY.to_owned(),
            serde_json::to_value(&spec).map_err(configuration_error)?,
        )]);
        let mut request = CreateCollectionBuilder::new(&collection)
            .vectors_config(vectors)
            .metadata(metadata);
        if let Some(value) = bool_parameter(&spec.parameters, "on_disk_payload")? {
            request = request.on_disk_payload(value);
        }
        if let Some(value) = u32_parameter(&spec.parameters, "shard_number")? {
            request = request.shard_number(value);
        }
        if let Some(value) = u32_parameter(&spec.parameters, "replication_factor")? {
            request = request.replication_factor(value);
        }
        if let Some(value) = u32_parameter(&spec.parameters, "write_consistency_factor")? {
            request = request.write_consistency_factor(value);
        }
        self.client
            .create_collection(request)
            .await
            .map_err(backend_error)?;
        Ok(GenerationHandle {
            artifact_id: spec.artifact_id,
            generation: spec.generation,
            locator: collection,
        })
    }

    async fn apply(
        &self,
        generation: &GenerationHandle,
        changes: Vec<VectorChange>,
    ) -> Result<ApplyReceipt, VectorError> {
        let collection = self.validate_handle(generation)?;
        let (spec, _) = self.load_spec(&collection).await?;
        let mut prepared = Vec::with_capacity(changes.len());
        for change in changes {
            prepared.push(prepare_change(change, &spec)?);
        }

        let applied = prepared.len();
        let mut position = 0;
        while position < prepared.len() {
            match &prepared[position] {
                PreparedChange::Upsert(_) => {
                    let end = prepared[position..]
                        .iter()
                        .position(|change| matches!(change, PreparedChange::Delete(_)))
                        .map_or(prepared.len(), |relative| position + relative);
                    let points = prepared[position..end]
                        .iter()
                        .filter_map(|change| match change {
                            PreparedChange::Upsert(point) => Some((**point).clone()),
                            PreparedChange::Delete(_) => None,
                        })
                        .collect::<Vec<_>>();
                    self.client
                        .upsert_points(
                            qdrant_client::qdrant::UpsertPointsBuilder::new(&collection, points)
                                .wait(true),
                        )
                        .await
                        .map_err(backend_error)?;
                    position = end;
                }
                PreparedChange::Delete(_) => {
                    let end = prepared[position..]
                        .iter()
                        .position(|change| matches!(change, PreparedChange::Upsert(_)))
                        .map_or(prepared.len(), |relative| position + relative);
                    let ids = prepared[position..end]
                        .iter()
                        .filter_map(|change| match change {
                            PreparedChange::Delete(id) => Some(*id),
                            PreparedChange::Upsert(_) => None,
                        })
                        .collect::<Vec<_>>();
                    self.client
                        .delete_points(DeletePointsBuilder::new(&collection).points(ids).wait(true))
                        .await
                        .map_err(backend_error)?;
                    position = end;
                }
            }
        }
        Ok(ApplyReceipt { applied })
    }

    async fn flush(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let collection = self.validate_handle(generation)?;
        self.client
            .collection_info(collection)
            .await
            .map_err(backend_error)?;
        Ok(())
    }

    async fn optimize(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        // Qdrant continuously schedules segment optimization. All writes use
        // wait=true; verifying the collection here is the portable lifecycle
        // barrier without overriding deployment-specific optimizer settings.
        self.flush(generation).await
    }

    async fn snapshot(&self, generation: &GenerationHandle) -> Result<SnapshotRef, VectorError> {
        let collection = self.validate_handle(generation)?;
        let result = self
            .client
            .create_snapshot(&collection)
            .await
            .map_err(backend_error)?;
        let snapshot = result.snapshot_description.ok_or_else(|| {
            VectorError::Backend(format!(
                "Qdrant did not return a snapshot name for collection {collection:?}"
            ))
        })?;
        Ok(SnapshotRef {
            locator: format!("qdrant://{collection}/snapshots/{}", snapshot.name),
        })
    }

    async fn activate(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let collection = self.validate_handle(generation)?;
        if !self
            .client
            .collection_exists(&collection)
            .await
            .map_err(backend_error)?
        {
            return Err(VectorError::InvalidRequest(format!(
                "unknown Qdrant generation collection {collection:?}"
            )));
        }
        let alias = self.alias_name(&generation.artifact_id)?;
        self.activate_alias(&alias, &collection).await
    }

    async fn generations(&self, artifact_id: &str) -> Result<Vec<GenerationStatus>, VectorError> {
        let prefix = format!("{}{GENERATION_SEPARATOR}", self.alias_name(artifact_id)?);
        let active = self.active_collection(artifact_id).await?;
        let collections = self
            .client
            .list_collections()
            .await
            .map_err(backend_error)?;
        let mut statuses = Vec::new();
        for collection in collections
            .collections
            .into_iter()
            .map(|description| description.name)
            .filter(|name| name.starts_with(&prefix))
        {
            let (spec, vector_count) = self.load_spec(&collection).await?;
            statuses.push(GenerationStatus {
                handle: GenerationHandle {
                    artifact_id: spec.artifact_id.clone(),
                    generation: spec.generation.clone(),
                    locator: collection.clone(),
                },
                spec,
                active: active.as_deref() == Some(&collection),
                vector_count,
            });
        }
        statuses.sort_by(|left, right| left.handle.generation.cmp(&right.handle.generation));
        Ok(statuses)
    }

    async fn delete_generation(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let collection = self.validate_handle(generation)?;
        if self
            .active_collection(&generation.artifact_id)
            .await?
            .as_deref()
            == Some(&collection)
        {
            return Err(VectorError::InvalidRequest(format!(
                "cannot delete active generation {:?}/{:?}",
                generation.artifact_id, generation.generation
            )));
        }
        self.client
            .delete_collection(collection)
            .await
            .map_err(backend_error)?;
        Ok(())
    }
}

#[derive(Clone)]
enum PreparedChange {
    Upsert(Box<PointStruct>),
    Delete(Uuid),
}

fn prepare_change(
    change: VectorChange,
    spec: &IndexGenerationSpec,
) -> Result<PreparedChange, VectorError> {
    match change {
        VectorChange::Upsert {
            document_id,
            vectors,
            payload,
        } => {
            if vectors.len() != 1 || !vectors.contains_key(&spec.vector_name) {
                return Err(VectorError::InvalidRequest(format!(
                    "generation expects exactly the named vector {:?}",
                    spec.vector_name
                )));
            }
            let VectorValue::Dense(vector) = vectors.into_values().next().expect("length checked")
            else {
                return Err(VectorError::Unsupported(
                    "Qdrant adapter currently supports dense stored vectors".to_owned(),
                ));
            };
            validate_dense(&vector, "stored")?;
            if vector.len() != spec.dimension {
                return Err(VectorError::InvalidRequest(format!(
                    "stored vector dimension {} does not match index dimension {}",
                    vector.len(),
                    spec.dimension
                )));
            }
            if spec.distance == DistanceMetric::Cosine
                && vector.iter().map(|value| value * value).sum::<f32>() == 0.0
            {
                return Err(VectorError::InvalidRequest(
                    "stored cosine vector cannot have zero magnitude".to_owned(),
                ));
            }
            if payload.contains_key(DOCUMENT_ID_PAYLOAD) {
                return Err(VectorError::InvalidRequest(format!(
                    "payload field {DOCUMENT_ID_PAYLOAD:?} is reserved"
                )));
            }
            let mut payload = payload.into_iter().collect::<serde_json::Map<_, _>>();
            payload.insert(DOCUMENT_ID_PAYLOAD.to_owned(), json!(document_id.0));
            let payload = Payload::try_from(Value::Object(payload)).map_err(backend_error)?;
            let vectors = NamedVectors::default().add_vector(&spec.vector_name, vector);
            let id = point_id(&document_id);
            Ok(PreparedChange::Upsert(Box::new(PointStruct::new(
                id, vectors, payload,
            ))))
        }
        VectorChange::Delete { document_id } => Ok(PreparedChange::Delete(point_id(&document_id))),
    }
}

fn point_id(document_id: &DocumentId) -> Uuid {
    let digest = Sha3_256::digest(document_id.0.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 variant and a stable, name-derived UUID version marker.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn validate_spec(
    spec: &IndexGenerationSpec,
    descriptor: &VectorBackendDescriptor,
) -> Result<(), VectorError> {
    validate_identifier("artifact id", &spec.artifact_id)?;
    validate_identifier("generation", &spec.generation)?;
    validate_identifier("vector name", &spec.vector_name)?;
    if spec.dimension == 0 {
        return Err(VectorError::InvalidRequest(
            "vector dimension must be greater than zero".to_owned(),
        ));
    }
    if !descriptor.capabilities.distances.contains(&spec.distance) {
        return Err(VectorError::Unsupported(format!(
            "distance {:?} is not supported by Qdrant",
            spec.distance
        )));
    }
    let unsupported = spec
        .parameters
        .keys()
        .filter(|key| !SUPPORTED_PARAMETERS.contains(&key.as_str()))
        .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        return Err(VectorError::Unsupported(format!(
            "unsupported Qdrant generation parameters: {unsupported:?}"
        )));
    }
    // Validate parameter types before making a remote request.
    bool_parameter(&spec.parameters, "on_disk")?;
    bool_parameter(&spec.parameters, "on_disk_payload")?;
    u32_parameter(&spec.parameters, "shard_number")?;
    u32_parameter(&spec.parameters, "replication_factor")?;
    u32_parameter(&spec.parameters, "write_consistency_factor")?;
    Ok(())
}

fn validate_query(query: &VectorQuery) -> Result<(), VectorError> {
    validate_identifier("index", &query.index)?;
    validate_identifier("vector name", &query.vector_name)?;
    if query.limit == 0 {
        return Err(VectorError::InvalidRequest(
            "vector query limit must be greater than zero".to_owned(),
        ));
    }
    if query
        .score_threshold
        .is_some_and(|threshold| !threshold.is_finite())
    {
        return Err(VectorError::InvalidRequest(
            "score_threshold must be finite".to_owned(),
        ));
    }
    if !query.parameters.is_empty() {
        return Err(VectorError::Unsupported(format!(
            "unsupported Qdrant query parameters: {:?}",
            query.parameters.keys().collect::<Vec<_>>()
        )));
    }
    Ok(())
}

fn validate_dense(vector: &[f32], label: &str) -> Result<(), VectorError> {
    if vector.is_empty() {
        return Err(VectorError::InvalidRequest(format!(
            "{label} vector cannot be empty"
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(VectorError::InvalidRequest(format!(
            "{label} vector contains a non-finite value"
        )));
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), VectorError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(VectorError::InvalidRequest(format!(
            "{label} must contain only ASCII letters, digits, '.', '_', or '-'"
        )));
    }
    Ok(())
}

fn checked_collection_name(value: String) -> Result<String, VectorError> {
    if value.len() > 255 {
        return Err(VectorError::InvalidRequest(
            "derived Qdrant collection name exceeds 255 bytes".to_owned(),
        ));
    }
    Ok(value)
}

fn parse_cursor(value: &str) -> Result<usize, VectorError> {
    value.parse().map_err(|_| {
        VectorError::InvalidRequest("Qdrant cursor must be a non-negative integer".to_owned())
    })
}

fn bool_parameter(
    parameters: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<bool>, VectorError> {
    parameters
        .get(name)
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                VectorError::InvalidRequest(format!(
                    "Qdrant generation parameter {name:?} must be a boolean"
                ))
            })
        })
        .transpose()
}

fn u32_parameter(
    parameters: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<u32>, VectorError> {
    parameters
        .get(name)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    VectorError::InvalidRequest(format!(
                        "Qdrant generation parameter {name:?} must be a positive u32"
                    ))
                })
        })
        .transpose()
}

fn qdrant_distance(distance: DistanceMetric) -> Distance {
    match distance {
        DistanceMetric::Cosine => Distance::Cosine,
        DistanceMetric::Dot => Distance::Dot,
        DistanceMetric::Euclidean => Distance::Euclid,
        DistanceMetric::Manhattan => Distance::Manhattan,
        DistanceMetric::Hamming | DistanceMetric::Jaccard => {
            unreachable!("unsupported distances are rejected before conversion")
        }
    }
}

fn portable_score(score: f32, distance: DistanceMetric) -> f32 {
    match distance {
        DistanceMetric::Cosine | DistanceMetric::Dot => score,
        DistanceMetric::Euclidean | DistanceMetric::Manhattan => -score,
        DistanceMetric::Hamming | DistanceMetric::Jaccard => {
            unreachable!("unsupported distances are rejected before querying")
        }
    }
}

fn qdrant_score_threshold(threshold: f32, distance: DistanceMetric) -> f32 {
    match distance {
        DistanceMetric::Cosine | DistanceMetric::Dot => threshold,
        DistanceMetric::Euclidean | DistanceMetric::Manhattan => -threshold,
        DistanceMetric::Hamming | DistanceMetric::Jaccard => {
            unreachable!("unsupported distances are rejected before querying")
        }
    }
}

fn configuration_error(error: impl fmt::Display) -> VectorError {
    VectorError::Configuration(error.to_string())
}

fn backend_error(error: impl fmt::Display) -> VectorError {
    VectorError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> QdrantRemoteConfig {
        QdrantRemoteConfig {
            id: "primary".to_owned(),
            grpc_url: "http://localhost:6334".to_owned(),
            rest_url: "http://localhost:6333/".to_owned(),
            api_key: Some("secret".to_owned()),
            collection_prefix: "sbol".to_owned(),
            timeout_seconds: 5,
        }
    }

    #[test]
    fn config_debug_redacts_api_key() {
        let rendered = format!("{:?}", config());
        assert!(rendered.contains("[redacted]"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn collection_names_encode_artifact_and_generation() {
        let backend = QdrantRemoteBackend::new(config()).unwrap();
        assert_eq!(
            backend.alias_name("components").unwrap(),
            "sbol__components"
        );
        assert_eq!(
            backend.collection_name("components", "2026-07-26").unwrap(),
            "sbol__components__gen__2026-07-26"
        );
    }

    #[test]
    fn document_ids_map_to_stable_distinct_point_ids() {
        let first = point_id(&DocumentId("https://example.test/a".to_owned()));
        assert_eq!(
            first,
            point_id(&DocumentId("https://example.test/a".to_owned()))
        );
        assert_ne!(
            first,
            point_id(&DocumentId("https://example.test/b".to_owned()))
        );
    }

    #[test]
    fn rejects_unknown_generation_parameters() {
        let descriptor = QdrantRemoteBackend::new(config()).unwrap().descriptor;
        let spec = IndexGenerationSpec {
            artifact_id: "components".to_owned(),
            generation: "one".to_owned(),
            vector_name: "content".to_owned(),
            dimension: 3,
            distance: DistanceMetric::Cosine,
            parameters: BTreeMap::from([("mystery".to_owned(), json!(true))]),
        };
        assert!(matches!(
            validate_spec(&spec, &descriptor),
            Err(VectorError::Unsupported(_))
        ));
    }

    #[test]
    fn normalizes_distance_scores_to_higher_is_better() {
        assert_eq!(portable_score(2.5, DistanceMetric::Euclidean), -2.5);
        assert_eq!(portable_score(3.0, DistanceMetric::Manhattan), -3.0);
        assert_eq!(portable_score(0.75, DistanceMetric::Cosine), 0.75);
        assert_eq!(qdrant_score_threshold(-2.5, DistanceMetric::Euclidean), 2.5);
    }
}
