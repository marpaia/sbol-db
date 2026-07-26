use std::collections::BTreeMap;

use sbol_db_search_sdk::{DocumentId, IndexGenerationSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredRecord {
    pub(crate) document_id: DocumentId,
    pub(crate) vector: Vec<f32>,
    pub(crate) payload: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct PersistedRecords {
    pub(crate) records: Vec<StoredRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IndexProfile {
    Flat,
    IvfFlat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GenerationManifest {
    pub(crate) format_version: u32,
    pub(crate) spec: IndexGenerationSpec,
    pub(crate) profile: IndexProfile,
    pub(crate) index_factory: String,
    pub(crate) vector_count: usize,
    pub(crate) nlist: usize,
    pub(crate) default_nprobe: usize,
    pub(crate) faiss_version: String,
    pub(crate) records_sha3_256: String,
    pub(crate) index_sha3_256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ActivePointer {
    pub(crate) format_version: u32,
    pub(crate) artifact_id: String,
    pub(crate) generation: String,
    pub(crate) manifest_sha3_256: String,
}
