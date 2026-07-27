use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_trait::async_trait;
use fs2::FileExt;
use sbol_db_search_sdk::{
    ApplyReceipt, DistanceMetric, DocumentId, FilterCapability, GenerationHandle, GenerationStatus,
    IndexGenerationSpec, SnapshotRef, VectorBackendDescriptor, VectorCapabilities, VectorChange,
    VectorError, VectorIndexAdmin, VectorQuery, VectorSearchHit, VectorSearchPage, VectorSearcher,
    VectorValue,
};

use crate::config::FaissBackendConfig;
use crate::engine::{build_index, LoadedGeneration};
use crate::model::{
    ActivePointer, GenerationManifest, PersistedRecords, StoredRecord, FORMAT_VERSION,
};
use crate::persistence::{
    atomic_json, atomic_write, checksum_bytes, checksum_file, copy_file, io_error, read_json,
    sync_directory,
};

const SUPPORTED_GENERATION_PARAMETERS: &[&str] = &["nlist", "nprobe", "flat_search_cutoff"];
const SUPPORTED_QUERY_PARAMETERS: &[&str] = &["nprobe", "max_codes"];
type GenerationKey = (String, String);

struct Generation {
    handle: GenerationHandle,
    spec: IndexGenerationSpec,
    records: BTreeMap<DocumentId, StoredRecord>,
    loaded: Option<Arc<LoadedGeneration>>,
    optimizing: bool,
}

#[derive(Default)]
struct State {
    generations: HashMap<GenerationKey, Generation>,
    active: HashMap<String, String>,
}

struct Inner {
    config: FaissBackendConfig,
    root: PathBuf,
    _lock_file: File,
    state: RwLock<State>,
}

/// Persistent, embedded FAISS implementation of the sbol-db vector contract.
///
/// The backend uses immutable, checksummed generations. FAISS owns only the
/// nearest-neighbor index; sbol-db owns identity, payload filtering, durable
/// records, activation, snapshots, and maintenance state.
pub struct FaissVectorBackend {
    descriptor: VectorBackendDescriptor,
    inner: Arc<Inner>,
}

impl FaissVectorBackend {
    pub fn open(mut config: FaissBackendConfig) -> Result<Self, VectorError> {
        validate_identifier("backend id", &config.id)?;
        if config.default_nlist == 0 || config.default_nprobe == 0 || config.max_query_k == 0 {
            return Err(VectorError::Configuration(
                "FAISS default_nlist, default_nprobe, and max_query_k must be greater than zero"
                    .to_owned(),
            ));
        }
        fs::create_dir_all(&config.path).map_err(io_error)?;
        let root = config.path.canonicalize().map_err(io_error)?;
        config.path = root.clone();
        fs::create_dir_all(root.join("generations")).map_err(io_error)?;
        fs::create_dir_all(root.join("active")).map_err(io_error)?;
        fs::create_dir_all(root.join("snapshots")).map_err(io_error)?;

        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("backend.lock"))
            .map_err(io_error)?;
        lock_file.try_lock_exclusive().map_err(|error| {
            VectorError::Configuration(format!(
                "FAISS store {:?} is already open by another backend: {error}",
                root
            ))
        })?;
        let state = load_state(&root)?;
        let descriptor = VectorBackendDescriptor {
            id: config.id.clone(),
            kind: "faiss".to_owned(),
            remote: false,
            capabilities: VectorCapabilities {
                dense: true,
                sparse: false,
                multi_dense: false,
                distances: vec![
                    DistanceMetric::Cosine,
                    DistanceMetric::Dot,
                    DistanceMetric::Euclidean,
                ],
                filter_execution: FilterCapability::Native,
                persistent: true,
                incremental_updates: false,
                deletes: true,
                atomic_activation: true,
                snapshots: true,
            },
        };
        Ok(Self {
            descriptor,
            inner: Arc::new(Inner {
                config,
                root,
                _lock_file: lock_file,
                state: RwLock::new(state),
            }),
        })
    }
}

#[async_trait]
impl VectorSearcher for FaissVectorBackend {
    fn descriptor(&self) -> &VectorBackendDescriptor {
        &self.descriptor
    }

    async fn query(&self, query: VectorQuery) -> Result<VectorSearchPage, VectorError> {
        validate_query(&query, self.inner.config.max_query_k)?;
        let loaded = {
            let state = self.inner.read_state()?;
            let generation_name = state.active.get(&query.index).ok_or_else(|| {
                VectorError::Configuration(format!(
                    "vector artifact {:?} has no active generation",
                    query.index
                ))
            })?;
            let generation = state
                .generations
                .get(&(query.index.clone(), generation_name.clone()))
                .ok_or_else(|| {
                    VectorError::Backend(format!(
                        "active generation {:?}/{generation_name:?} is missing",
                        query.index
                    ))
                })?;
            generation.loaded.clone().ok_or_else(|| {
                VectorError::Backend(format!(
                    "active generation {:?}/{generation_name:?} is not ready",
                    query.index
                ))
            })?
        };
        validate_query_for_generation(&query, &loaded.manifest)?;
        let cursor = query
            .cursor
            .as_deref()
            .map(|value| parse_cursor(value, &loaded.manifest.spec.generation))
            .transpose()?
            .unwrap_or(0);
        let fetch = cursor
            .checked_add(query.limit)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                VectorError::InvalidRequest("vector query window is too large".to_owned())
            })?;
        if fetch > self.inner.config.max_query_k {
            return Err(VectorError::InvalidRequest(format!(
                "vector query window {fetch} exceeds configured maximum {}",
                self.inner.config.max_query_k
            )));
        }
        let nprobe =
            query_usize(&query.parameters, "nprobe")?.unwrap_or(loaded.manifest.default_nprobe);
        if loaded.manifest.nlist > 0 && (nprobe == 0 || nprobe > loaded.manifest.nlist) {
            return Err(VectorError::InvalidRequest(format!(
                "nprobe must be between 1 and {}",
                loaded.manifest.nlist
            )));
        }
        let max_codes = query_usize(&query.parameters, "max_codes")?.unwrap_or(0);
        let VectorValue::Dense(vector) = query.vector else {
            unreachable!("non-dense queries are rejected before execution")
        };
        let filter = query.filter;
        let threshold = query.score_threshold;
        let limit = query.limit;
        tokio::task::spawn_blocking(move || {
            let mut hits = loaded.search(vector, filter.as_ref(), fetch, nprobe, max_codes)?;
            hits.sort_by(|left, right| {
                right.1.total_cmp(&left.1).then_with(|| {
                    loaded.records[left.0]
                        .document_id
                        .cmp(&loaded.records[right.0].document_id)
                })
            });
            if let Some(threshold) = threshold {
                hits.retain(|(_, score)| *score >= threshold);
            }
            let end = cursor.saturating_add(limit).min(hits.len());
            let items = hits
                .get(cursor..end)
                .unwrap_or_default()
                .iter()
                .map(|&(id, score)| VectorSearchHit {
                    document_id: loaded.records[id].document_id.clone(),
                    score,
                })
                .collect();
            let next_cursor =
                (end < hits.len()).then(|| format!("{}:{end}", loaded.manifest.spec.generation));
            Ok(VectorSearchPage { items, next_cursor })
        })
        .await
        .map_err(join_error)?
    }
}

#[async_trait]
impl VectorIndexAdmin for FaissVectorBackend {
    async fn create_generation(
        &self,
        spec: IndexGenerationSpec,
    ) -> Result<GenerationHandle, VectorError> {
        validate_spec(&spec, &self.descriptor)?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.create_generation(spec))
            .await
            .map_err(join_error)?
    }

    async fn apply(
        &self,
        generation: &GenerationHandle,
        changes: Vec<VectorChange>,
    ) -> Result<ApplyReceipt, VectorError> {
        let inner = self.inner.clone();
        let generation = generation.clone();
        tokio::task::spawn_blocking(move || inner.apply(&generation, changes))
            .await
            .map_err(join_error)?
    }

    async fn flush(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let inner = self.inner.clone();
        let generation = generation.clone();
        tokio::task::spawn_blocking(move || inner.flush(&generation))
            .await
            .map_err(join_error)?
    }

    async fn optimize(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let inner = self.inner.clone();
        let generation = generation.clone();
        tokio::task::spawn_blocking(move || inner.optimize(&generation))
            .await
            .map_err(join_error)?
    }

    async fn snapshot(&self, generation: &GenerationHandle) -> Result<SnapshotRef, VectorError> {
        let inner = self.inner.clone();
        let generation = generation.clone();
        tokio::task::spawn_blocking(move || inner.snapshot(&generation))
            .await
            .map_err(join_error)?
    }

    async fn activate(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let inner = self.inner.clone();
        let generation = generation.clone();
        tokio::task::spawn_blocking(move || inner.activate(&generation))
            .await
            .map_err(join_error)?
    }

    async fn generations(&self, artifact_id: &str) -> Result<Vec<GenerationStatus>, VectorError> {
        validate_identifier("artifact id", artifact_id)?;
        let state = self.inner.read_state()?;
        let active = state.active.get(artifact_id);
        let mut statuses = state
            .generations
            .values()
            .filter(|generation| generation.spec.artifact_id == artifact_id)
            .map(|generation| GenerationStatus {
                handle: generation.handle.clone(),
                spec: generation.spec.clone(),
                active: active == Some(&generation.spec.generation),
                vector_count: generation.records.len(),
            })
            .collect::<Vec<_>>();
        statuses.sort_by(|left, right| left.handle.generation.cmp(&right.handle.generation));
        Ok(statuses)
    }

    async fn delete_generation(&self, generation: &GenerationHandle) -> Result<(), VectorError> {
        let inner = self.inner.clone();
        let generation = generation.clone();
        tokio::task::spawn_blocking(move || inner.delete_generation(&generation))
            .await
            .map_err(join_error)?
    }
}

impl Inner {
    fn read_state(&self) -> Result<RwLockReadGuard<'_, State>, VectorError> {
        self.state
            .read()
            .map_err(|_| VectorError::Backend("FAISS backend state lock is poisoned".to_owned()))
    }

    fn write_state(&self) -> Result<RwLockWriteGuard<'_, State>, VectorError> {
        self.state
            .write()
            .map_err(|_| VectorError::Backend("FAISS backend state lock is poisoned".to_owned()))
    }

    fn create_generation(
        &self,
        spec: IndexGenerationSpec,
    ) -> Result<GenerationHandle, VectorError> {
        let key = (spec.artifact_id.clone(), spec.generation.clone());
        let mut state = self.write_state()?;
        if state.generations.contains_key(&key) {
            return Err(VectorError::InvalidRequest(format!(
                "generation {:?}/{:?} already exists",
                key.0, key.1
            )));
        }
        let artifact_directory = self.root.join("generations").join(&key.0);
        fs::create_dir_all(&artifact_directory).map_err(io_error)?;
        let target = artifact_directory.join(&key.1);
        if target.exists() {
            return Err(VectorError::Backend(format!(
                "generation directory {:?} already exists",
                target
            )));
        }
        let temporary = tempfile::Builder::new()
            .prefix(".creating-")
            .tempdir_in(&artifact_directory)
            .map_err(io_error)?;
        atomic_json(&temporary.path().join("spec.json"), &spec)?;
        atomic_json(
            &temporary.path().join("records.json"),
            &PersistedRecords::default(),
        )?;
        sync_directory(temporary.path())?;
        let temporary_path = temporary.keep();
        fs::rename(&temporary_path, &target).map_err(io_error)?;
        sync_directory(&artifact_directory)?;
        let handle = GenerationHandle {
            artifact_id: key.0.clone(),
            generation: key.1.clone(),
            locator: target.to_string_lossy().into_owned(),
        };
        state.generations.insert(
            key,
            Generation {
                handle: handle.clone(),
                spec,
                records: BTreeMap::new(),
                loaded: None,
                optimizing: false,
            },
        );
        Ok(handle)
    }

    fn apply(
        &self,
        handle: &GenerationHandle,
        changes: Vec<VectorChange>,
    ) -> Result<ApplyReceipt, VectorError> {
        let mut state = self.write_state()?;
        reject_active(&state, handle)?;
        let generation = generation_mut(&mut state, handle)?;
        if generation.loaded.is_some() || generation.optimizing {
            return Err(VectorError::InvalidRequest(
                "cannot apply changes to a ready or optimizing generation".to_owned(),
            ));
        }
        let mut records = generation.records.clone();
        let mut applied = 0;
        for change in changes {
            match change {
                VectorChange::Upsert {
                    document_id,
                    vectors,
                    payload,
                } => {
                    validate_document_id(&document_id)?;
                    if vectors.len() != 1 || !vectors.contains_key(&generation.spec.vector_name) {
                        return Err(VectorError::InvalidRequest(format!(
                            "generation expects exactly the named vector {:?}",
                            generation.spec.vector_name
                        )));
                    }
                    let value = vectors.into_iter().next().expect("one validated vector").1;
                    let VectorValue::Dense(vector) = value else {
                        return Err(VectorError::Unsupported(
                            "FAISS backend supports only dense stored vectors".to_owned(),
                        ));
                    };
                    validate_dense(&vector, generation.spec.dimension, "stored")?;
                    if generation.spec.distance == DistanceMetric::Cosine {
                        validate_nonzero(&vector, "stored")?;
                    }
                    records.insert(
                        document_id.clone(),
                        StoredRecord {
                            document_id,
                            vector,
                            payload,
                        },
                    );
                    applied += 1;
                }
                VectorChange::Delete { document_id } => {
                    if records.remove(&document_id).is_some() {
                        applied += 1;
                    }
                }
            }
        }
        persist_records(Path::new(&generation.handle.locator), &records)?;
        generation.records = records;
        Ok(ApplyReceipt { applied })
    }

    fn flush(&self, handle: &GenerationHandle) -> Result<(), VectorError> {
        let state = self.read_state()?;
        let generation = generation_ref(&state, handle)?;
        persist_records(Path::new(&generation.handle.locator), &generation.records)
    }

    fn optimize(&self, handle: &GenerationHandle) -> Result<(), VectorError> {
        let (spec, records, directory) = {
            let mut state = self.write_state()?;
            reject_active(&state, handle)?;
            let generation = generation_mut(&mut state, handle)?;
            if generation.loaded.is_some() {
                return Ok(());
            }
            if generation.optimizing {
                return Err(VectorError::InvalidRequest(
                    "generation is already being optimized".to_owned(),
                ));
            }
            generation.optimizing = true;
            (
                generation.spec.clone(),
                generation.records.values().cloned().collect::<Vec<_>>(),
                PathBuf::from(&generation.handle.locator),
            )
        };
        let result = (|| {
            persist_records_vec(&directory, &records)?;
            let records_checksum = checksum_file(&directory.join("records.json"))?;
            let manifest =
                build_index(&directory, &spec, &records, &self.config, records_checksum)?;
            atomic_json(&directory.join("manifest.json"), &manifest)?;
            let manifest_bytes = fs::read(directory.join("manifest.json")).map_err(io_error)?;
            let loaded = LoadedGeneration::load(&directory, manifest, records)?;
            atomic_write(
                &directory.join("READY"),
                checksum_bytes(&manifest_bytes).as_bytes(),
            )?;
            Ok(Arc::new(loaded))
        })();
        let mut state = self.write_state()?;
        let generation = generation_mut(&mut state, handle)?;
        generation.optimizing = false;
        if let Ok(loaded) = &result {
            generation.loaded = Some(loaded.clone());
        }
        result.map(|_| ())
    }

    fn activate(&self, handle: &GenerationHandle) -> Result<(), VectorError> {
        let mut state = self.write_state()?;
        let generation = generation_ref(&state, handle)?;
        if generation.loaded.is_none() {
            return Err(VectorError::InvalidRequest(
                "cannot activate a generation before optimize completes".to_owned(),
            ));
        }
        let directory = Path::new(&generation.handle.locator);
        let manifest_checksum = checksum_file(&directory.join("manifest.json"))?;
        let pointer = ActivePointer {
            format_version: FORMAT_VERSION,
            artifact_id: handle.artifact_id.clone(),
            generation: handle.generation.clone(),
            manifest_sha3_256: manifest_checksum,
        };
        atomic_json(
            &self
                .root
                .join("active")
                .join(format!("{}.json", handle.artifact_id)),
            &pointer,
        )?;
        state
            .active
            .insert(handle.artifact_id.clone(), handle.generation.clone());
        Ok(())
    }

    fn snapshot(&self, handle: &GenerationHandle) -> Result<SnapshotRef, VectorError> {
        let state = self.read_state()?;
        let generation = generation_ref(&state, handle)?;
        if generation.loaded.is_none() {
            return Err(VectorError::InvalidRequest(
                "cannot snapshot a generation before optimize completes".to_owned(),
            ));
        }
        let snapshot_parent = self.root.join("snapshots").join(&handle.artifact_id);
        fs::create_dir_all(&snapshot_parent).map_err(io_error)?;
        let target = snapshot_parent.join(&handle.generation);
        if !target.exists() {
            let temporary = tempfile::Builder::new()
                .prefix(".snapshot-")
                .tempdir_in(&snapshot_parent)
                .map_err(io_error)?;
            let source = Path::new(&generation.handle.locator);
            for name in [
                "spec.json",
                "records.json",
                "index.faiss",
                "manifest.json",
                "READY",
            ] {
                copy_file(&source.join(name), &temporary.path().join(name))?;
            }
            sync_directory(temporary.path())?;
            let temporary_path = temporary.keep();
            fs::rename(&temporary_path, &target).map_err(io_error)?;
            sync_directory(&snapshot_parent)?;
        }
        Ok(SnapshotRef {
            locator: target.to_string_lossy().into_owned(),
        })
    }

    fn delete_generation(&self, handle: &GenerationHandle) -> Result<(), VectorError> {
        let mut state = self.write_state()?;
        reject_active(&state, handle)?;
        let generation = generation_ref(&state, handle)?;
        if generation.optimizing {
            return Err(VectorError::InvalidRequest(
                "cannot delete a generation while it is optimizing".to_owned(),
            ));
        }
        fs::remove_dir_all(&generation.handle.locator).map_err(io_error)?;
        sync_directory(
            Path::new(&generation.handle.locator)
                .parent()
                .expect("validated generation parent"),
        )?;
        state.generations.remove(&generation_key(handle));
        Ok(())
    }
}

fn load_state(root: &Path) -> Result<State, VectorError> {
    let mut state = State::default();
    for artifact_entry in fs::read_dir(root.join("generations")).map_err(io_error)? {
        let artifact_entry = artifact_entry.map_err(io_error)?;
        if !artifact_entry.file_type().map_err(io_error)?.is_dir() {
            continue;
        }
        let artifact_id = artifact_entry.file_name().to_string_lossy().into_owned();
        if artifact_id.starts_with('.') {
            continue;
        }
        validate_identifier("artifact id", &artifact_id)?;
        for generation_entry in fs::read_dir(artifact_entry.path()).map_err(io_error)? {
            let generation_entry = generation_entry.map_err(io_error)?;
            if !generation_entry.file_type().map_err(io_error)?.is_dir() {
                continue;
            }
            let generation_name = generation_entry.file_name().to_string_lossy().into_owned();
            if generation_name.starts_with('.') {
                continue;
            }
            validate_identifier("generation", &generation_name)?;
            let directory = generation_entry.path();
            let spec: IndexGenerationSpec = read_json(&directory.join("spec.json"))?;
            if spec.artifact_id != artifact_id || spec.generation != generation_name {
                return Err(VectorError::Backend(format!(
                    "generation directory {:?} does not match its specification",
                    directory
                )));
            }
            let persisted: PersistedRecords = read_json(&directory.join("records.json"))?;
            let records = records_map(persisted.records, &spec)?;
            let loaded = if directory.join("READY").exists() {
                Some(load_ready(
                    &directory,
                    &spec,
                    records.values().cloned().collect(),
                )?)
            } else {
                None
            };
            let handle = GenerationHandle {
                artifact_id: artifact_id.clone(),
                generation: generation_name.clone(),
                locator: directory.to_string_lossy().into_owned(),
            };
            state.generations.insert(
                (artifact_id.clone(), generation_name),
                Generation {
                    handle,
                    spec,
                    records,
                    loaded,
                    optimizing: false,
                },
            );
        }
    }
    for entry in fs::read_dir(root.join("active")).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_file() {
            continue;
        }
        let pointer: ActivePointer = read_json(&entry.path())?;
        if pointer.format_version != FORMAT_VERSION {
            return Err(VectorError::Unsupported(format!(
                "unsupported FAISS active pointer format {}",
                pointer.format_version
            )));
        }
        validate_identifier("artifact id", &pointer.artifact_id)?;
        validate_identifier("generation", &pointer.generation)?;
        let generation = state
            .generations
            .get(&(pointer.artifact_id.clone(), pointer.generation.clone()))
            .ok_or_else(|| {
                VectorError::Backend(format!(
                    "active pointer references missing generation {:?}/{:?}",
                    pointer.artifact_id, pointer.generation
                ))
            })?;
        if generation.loaded.is_none() {
            return Err(VectorError::Backend(format!(
                "active pointer references unready generation {:?}/{:?}",
                pointer.artifact_id, pointer.generation
            )));
        }
        let checksum = checksum_file(&Path::new(&generation.handle.locator).join("manifest.json"))?;
        if checksum != pointer.manifest_sha3_256 {
            return Err(VectorError::Backend(format!(
                "active pointer manifest checksum mismatch for {:?}/{:?}",
                pointer.artifact_id, pointer.generation
            )));
        }
        state.active.insert(pointer.artifact_id, pointer.generation);
    }
    Ok(state)
}

fn load_ready(
    directory: &Path,
    spec: &IndexGenerationSpec,
    records: Vec<StoredRecord>,
) -> Result<Arc<LoadedGeneration>, VectorError> {
    let manifest_bytes = fs::read(directory.join("manifest.json")).map_err(io_error)?;
    let ready = fs::read_to_string(directory.join("READY")).map_err(io_error)?;
    if ready.trim() != checksum_bytes(&manifest_bytes) {
        return Err(VectorError::Backend(format!(
            "generation {:?} has an invalid READY marker",
            directory
        )));
    }
    let manifest: GenerationManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            VectorError::Backend(format!(
                "cannot parse generation manifest {:?}: {error}",
                directory
            ))
        })?;
    if &manifest.spec != spec {
        return Err(VectorError::Backend(format!(
            "generation manifest {:?} does not match spec.json",
            directory
        )));
    }
    if checksum_file(&directory.join("records.json"))? != manifest.records_sha3_256 {
        return Err(VectorError::Backend(format!(
            "generation {:?} records checksum mismatch",
            directory
        )));
    }
    LoadedGeneration::load(directory, manifest, records).map(Arc::new)
}

fn records_map(
    records: Vec<StoredRecord>,
    spec: &IndexGenerationSpec,
) -> Result<BTreeMap<DocumentId, StoredRecord>, VectorError> {
    let mut map = BTreeMap::new();
    for record in records {
        validate_document_id(&record.document_id)?;
        validate_dense(&record.vector, spec.dimension, "stored")?;
        if spec.distance == DistanceMetric::Cosine {
            validate_nonzero(&record.vector, "stored")?;
        }
        if map.insert(record.document_id.clone(), record).is_some() {
            return Err(VectorError::Backend(
                "generation records contain a duplicate document id".to_owned(),
            ));
        }
    }
    Ok(map)
}

fn persist_records(
    directory: &Path,
    records: &BTreeMap<DocumentId, StoredRecord>,
) -> Result<(), VectorError> {
    persist_records_vec(directory, &records.values().cloned().collect::<Vec<_>>())
}

fn persist_records_vec(directory: &Path, records: &[StoredRecord]) -> Result<(), VectorError> {
    atomic_json(
        &directory.join("records.json"),
        &PersistedRecords {
            records: records.to_vec(),
        },
    )
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
            "distance {:?} is not supported by the FAISS backend",
            spec.distance
        )));
    }
    reject_unknown_parameters(
        &spec.parameters,
        SUPPORTED_GENERATION_PARAMETERS,
        "generation",
    )
}

fn validate_query(query: &VectorQuery, max_query_k: usize) -> Result<(), VectorError> {
    validate_identifier("index", &query.index)?;
    validate_identifier("vector name", &query.vector_name)?;
    if query.limit == 0 || query.limit > max_query_k {
        return Err(VectorError::InvalidRequest(format!(
            "vector query limit must be between 1 and {max_query_k}"
        )));
    }
    if query
        .score_threshold
        .is_some_and(|value| !value.is_finite())
    {
        return Err(VectorError::InvalidRequest(
            "score_threshold must be finite".to_owned(),
        ));
    }
    if !matches!(query.vector, VectorValue::Dense(_)) {
        return Err(VectorError::Unsupported(
            "FAISS backend supports only dense query vectors".to_owned(),
        ));
    }
    reject_unknown_parameters(&query.parameters, SUPPORTED_QUERY_PARAMETERS, "query")
}

fn validate_query_for_generation(
    query: &VectorQuery,
    manifest: &GenerationManifest,
) -> Result<(), VectorError> {
    if query.vector_name != manifest.spec.vector_name {
        return Err(VectorError::InvalidRequest(format!(
            "query vector {:?} does not match generation vector {:?}",
            query.vector_name, manifest.spec.vector_name
        )));
    }
    let VectorValue::Dense(vector) = &query.vector else {
        unreachable!("validated dense query")
    };
    validate_dense(vector, manifest.spec.dimension, "query")?;
    if manifest.spec.distance == DistanceMetric::Cosine {
        validate_nonzero(vector, "query")?;
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), VectorError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(VectorError::InvalidRequest(format!(
            "{label} must contain 1-128 ASCII letters, digits, '_' or '-'"
        )));
    }
    Ok(())
}

fn validate_document_id(id: &DocumentId) -> Result<(), VectorError> {
    if id.0.is_empty() || id.0.len() > 16 * 1024 {
        return Err(VectorError::InvalidRequest(
            "document id must contain 1-16384 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn validate_dense(vector: &[f32], dimension: usize, label: &str) -> Result<(), VectorError> {
    if vector.len() != dimension {
        return Err(VectorError::InvalidRequest(format!(
            "{label} vector dimension {} does not match index dimension {dimension}",
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

fn validate_nonzero(vector: &[f32], label: &str) -> Result<(), VectorError> {
    if vector.iter().map(|value| value * value).sum::<f32>() == 0.0 {
        return Err(VectorError::InvalidRequest(format!(
            "{label} cosine vector cannot have zero magnitude"
        )));
    }
    Ok(())
}

fn reject_unknown_parameters(
    parameters: &BTreeMap<String, serde_json::Value>,
    supported: &[&str],
    label: &str,
) -> Result<(), VectorError> {
    let unknown = parameters
        .keys()
        .filter(|key| !supported.contains(&key.as_str()))
        .collect::<Vec<_>>();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(VectorError::Unsupported(format!(
            "unsupported FAISS {label} parameters: {unknown:?}"
        )))
    }
}

fn query_usize(
    parameters: &BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Result<Option<usize>, VectorError> {
    let Some(value) = parameters.get(key) else {
        return Ok(None);
    };
    let value = value.as_u64().ok_or_else(|| {
        VectorError::InvalidRequest(format!(
            "FAISS parameter {key:?} must be an unsigned integer"
        ))
    })?;
    usize::try_from(value)
        .map(Some)
        .map_err(|_| VectorError::InvalidRequest(format!("FAISS parameter {key:?} is too large")))
}

fn parse_cursor(value: &str, active_generation: &str) -> Result<usize, VectorError> {
    let (generation, offset) = value.split_once(':').ok_or_else(|| {
        VectorError::InvalidRequest(
            "FAISS cursor must contain a generation and non-negative offset".to_owned(),
        )
    })?;
    if generation != active_generation {
        return Err(VectorError::InvalidRequest(format!(
            "FAISS cursor belongs to stale generation {generation:?}"
        )));
    }
    offset.parse().map_err(|_| {
        VectorError::InvalidRequest("FAISS cursor offset must be a non-negative integer".to_owned())
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

fn reject_active(state: &State, handle: &GenerationHandle) -> Result<(), VectorError> {
    if state.active.get(&handle.artifact_id) == Some(&handle.generation) {
        return Err(VectorError::InvalidRequest(format!(
            "cannot mutate active generation {:?}/{:?}",
            handle.artifact_id, handle.generation
        )));
    }
    Ok(())
}

fn join_error(error: tokio::task::JoinError) -> VectorError {
    VectorError::Backend(format!("FAISS blocking task failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbol_db_search_sdk::VectorFilter;
    use sbol_db_vector_flat::ExactFlatVectorBackend;
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::sync::Barrier;

    fn open_backend(directory: &TempDir) -> FaissVectorBackend {
        FaissVectorBackend::open(FaissBackendConfig::new("local", directory.path())).unwrap()
    }

    fn spec(generation: &str) -> IndexGenerationSpec {
        IndexGenerationSpec {
            artifact_id: "parts".to_owned(),
            generation: generation.to_owned(),
            vector_name: "content".to_owned(),
            dimension: 2,
            distance: DistanceMetric::Cosine,
            parameters: BTreeMap::new(),
        }
    }

    fn upsert(id: &str, vector: [f32; 2], graph: &str, year: u64) -> VectorChange {
        VectorChange::Upsert {
            document_id: DocumentId(id.to_owned()),
            vectors: BTreeMap::from([("content".to_owned(), VectorValue::Dense(vector.to_vec()))]),
            payload: BTreeMap::from([
                ("graph".to_owned(), json!(graph)),
                ("metadata".to_owned(), json!({"year": year})),
            ]),
        }
    }

    fn query(filter: Option<VectorFilter>, limit: usize) -> VectorQuery {
        VectorQuery {
            index: "parts".to_owned(),
            vector_name: "content".to_owned(),
            vector: VectorValue::Dense(vec![1.0, 0.0]),
            filter,
            limit,
            cursor: None,
            score_threshold: None,
            parameters: BTreeMap::new(),
        }
    }

    async fn build_generation(
        backend: &FaissVectorBackend,
        name: &str,
        records: Vec<VectorChange>,
    ) -> GenerationHandle {
        let handle = backend.create_generation(spec(name)).await.unwrap();
        backend.apply(&handle, records).await.unwrap();
        backend.flush(&handle).await.unwrap();
        backend.optimize(&handle).await.unwrap();
        handle
    }

    #[tokio::test]
    async fn lifecycle_is_durable_filtered_and_rollbackable() {
        let directory = TempDir::new().unwrap();
        let backend = open_backend(&directory);
        assert!(
            FaissVectorBackend::open(FaissBackendConfig::new("other", directory.path())).is_err()
        );

        let first = build_generation(
            &backend,
            "g1",
            vec![
                upsert("alpha", [1.0, 0.0], "public", 2024),
                upsert("beta", [0.9, 0.1], "public", 2026),
                upsert("secret", [1.0, 0.0], "private", 2026),
            ],
        )
        .await;
        let snapshot = backend.snapshot(&first).await.unwrap();
        assert!(Path::new(&snapshot.locator).join("index.faiss").exists());
        let manifest: GenerationManifest =
            read_json(&Path::new(&first.locator).join("manifest.json")).unwrap();
        assert!(manifest.faiss_version.starts_with("1.14."));
        backend.activate(&first).await.unwrap();

        let authorized = VectorFilter::And {
            clauses: vec![
                VectorFilter::Match {
                    field: "graph".to_owned(),
                    value: json!("public"),
                },
                VectorFilter::Range {
                    field: "metadata.year".to_owned(),
                    gte: Some(2025.0),
                    lte: None,
                },
            ],
        };
        let page = backend.query(query(Some(authorized), 10)).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].document_id, DocumentId("beta".to_owned()));

        let first_page = backend.query(query(None, 1)).await.unwrap();
        let cursor = first_page.next_cursor.clone().unwrap();
        assert!(cursor.starts_with("g1:"));
        drop(backend);

        let backend = open_backend(&directory);
        assert_eq!(
            backend.query(query(None, 1)).await.unwrap().items[0].document_id,
            DocumentId("alpha".to_owned())
        );
        let second = build_generation(
            &backend,
            "g2",
            vec![upsert("gamma", [1.0, 0.0], "public", 2026)],
        )
        .await;
        backend.activate(&second).await.unwrap();
        let mut stale = query(None, 1);
        stale.cursor = Some(cursor);
        assert!(matches!(
            backend.query(stale).await,
            Err(VectorError::InvalidRequest(_))
        ));
        assert!(backend.delete_generation(&second).await.is_err());
        backend.activate(&first).await.unwrap();
        backend.delete_generation(&second).await.unwrap();
        assert_eq!(backend.generations("parts").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejected_batch_is_not_persisted() {
        let directory = TempDir::new().unwrap();
        let backend = open_backend(&directory);
        let handle = backend.create_generation(spec("g1")).await.unwrap();
        let result = backend
            .apply(
                &handle,
                vec![
                    upsert("valid", [1.0, 0.0], "public", 2026),
                    upsert("invalid", [1.0, f32::NAN], "public", 2026),
                ],
            )
            .await;
        assert!(matches!(result, Err(VectorError::InvalidRequest(_))));
        drop(backend);

        let backend = open_backend(&directory);
        assert_eq!(
            backend.generations("parts").await.unwrap()[0].vector_count,
            0
        );
    }

    #[tokio::test]
    async fn corrupted_index_is_rejected_before_faiss_load() {
        let directory = TempDir::new().unwrap();
        let backend = open_backend(&directory);
        let handle = build_generation(
            &backend,
            "g1",
            vec![upsert("alpha", [1.0, 0.0], "public", 2026)],
        )
        .await;
        backend.activate(&handle).await.unwrap();
        drop(backend);

        let index_path = directory.path().join("generations/parts/g1/index.faiss");
        let mut bytes = fs::read(&index_path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(index_path, bytes).unwrap();
        assert!(matches!(
            FaissVectorBackend::open(FaissBackendConfig::new("local", directory.path())),
            Err(VectorError::Backend(_))
        ));
    }

    #[tokio::test]
    async fn concurrent_queries_observe_complete_generations_during_activation() {
        const READERS: usize = 8;
        let directory = TempDir::new().unwrap();
        let backend = Arc::new(open_backend(&directory));
        let first = build_generation(
            backend.as_ref(),
            "g1",
            vec![upsert("old", [1.0, 0.0], "public", 2025)],
        )
        .await;
        let second = build_generation(
            backend.as_ref(),
            "g2",
            vec![upsert("new", [1.0, 0.0], "public", 2026)],
        )
        .await;
        backend.activate(&first).await.unwrap();

        let barrier = Arc::new(Barrier::new(READERS + 1));
        let mut readers = Vec::new();
        for _ in 0..READERS {
            let backend = Arc::clone(&backend);
            let barrier = Arc::clone(&barrier);
            readers.push(tokio::spawn(async move {
                barrier.wait().await;
                for _ in 0..40 {
                    let page = backend.query(query(None, 1)).await.unwrap();
                    assert_eq!(page.items.len(), 1);
                    assert!(matches!(
                        page.items[0].document_id.0.as_str(),
                        "old" | "new"
                    ));
                    tokio::task::yield_now().await;
                }
            }));
        }

        barrier.wait().await;
        for iteration in 0..12 {
            let generation = if iteration % 2 == 0 { &second } else { &first };
            backend.activate(generation).await.unwrap();
            tokio::task::yield_now().await;
        }
        backend.activate(&second).await.unwrap();
        for reader in readers {
            reader.await.unwrap();
        }
        assert_eq!(
            backend.query(query(None, 1)).await.unwrap().items[0].document_id,
            DocumentId("new".to_owned())
        );
    }

    #[tokio::test]
    async fn flat_profile_matches_exact_backend_scores_and_filters() {
        let directory = TempDir::new().unwrap();
        let faiss = open_backend(&directory);
        let exact = ExactFlatVectorBackend::new("exact");
        let mut euclidean = spec("g1");
        euclidean.distance = DistanceMetric::Euclidean;
        let changes = vec![
            upsert("near", [0.5, 0.0], "public", 2026),
            upsert("far", [3.0, 0.0], "public", 2026),
            upsert("secret", [0.1, 0.0], "private", 2026),
        ];

        let faiss_generation = faiss.create_generation(euclidean.clone()).await.unwrap();
        faiss
            .apply(&faiss_generation, changes.clone())
            .await
            .unwrap();
        faiss.optimize(&faiss_generation).await.unwrap();
        faiss.activate(&faiss_generation).await.unwrap();
        let exact_generation = exact.create_generation(euclidean).await.unwrap();
        exact.apply(&exact_generation, changes).await.unwrap();
        exact.activate(&exact_generation).await.unwrap();

        let public = Some(VectorFilter::Match {
            field: "graph".to_owned(),
            value: json!("public"),
        });
        let mut request = query(public, 10);
        request.vector = VectorValue::Dense(vec![0.0, 0.0]);
        let faiss_page = faiss.query(request.clone()).await.unwrap();
        let exact_page = exact.query(request).await.unwrap();
        assert_eq!(
            faiss_page
                .items
                .iter()
                .map(|hit| &hit.document_id)
                .collect::<Vec<_>>(),
            exact_page
                .items
                .iter()
                .map(|hit| &hit.document_id)
                .collect::<Vec<_>>()
        );
        for (faiss_hit, exact_hit) in faiss_page.items.iter().zip(exact_page.items) {
            assert!((faiss_hit.score - exact_hit.score).abs() < 1.0e-6);
        }
    }

    #[tokio::test]
    async fn ivf_profile_filters_candidates_inside_faiss() {
        let directory = TempDir::new().unwrap();
        let backend = open_backend(&directory);
        let mut ann_spec = spec("g1");
        ann_spec.distance = DistanceMetric::Euclidean;
        ann_spec.parameters = BTreeMap::from([
            ("flat_search_cutoff".to_owned(), json!(0)),
            ("nlist".to_owned(), json!(4)),
            ("nprobe".to_owned(), json!(4)),
        ]);
        let handle = backend.create_generation(ann_spec).await.unwrap();
        let changes = (0..160)
            .map(|id| {
                upsert(
                    &format!("doc-{id:03}"),
                    [id as f32, 0.0],
                    if id >= 80 { "allowed" } else { "denied" },
                    2026,
                )
            })
            .collect();
        backend.apply(&handle, changes).await.unwrap();
        backend.optimize(&handle).await.unwrap();
        backend.activate(&handle).await.unwrap();

        let mut request = query(
            Some(VectorFilter::Match {
                field: "graph".to_owned(),
                value: json!("allowed"),
            }),
            1,
        );
        request.vector = VectorValue::Dense(vec![0.0, 0.0]);
        request.parameters.insert("nprobe".to_owned(), json!(4));
        let page = backend.query(request).await.unwrap();
        assert_eq!(page.items[0].document_id, DocumentId("doc-080".to_owned()));
    }
}
