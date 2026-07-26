use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Local storage and tuning defaults for one embedded FAISS backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaissBackendConfig {
    pub id: String,
    pub path: PathBuf,
    #[serde(default = "default_nlist")]
    pub default_nlist: usize,
    #[serde(default = "default_nprobe")]
    pub default_nprobe: usize,
    #[serde(default = "default_flat_search_cutoff")]
    pub flat_search_cutoff: usize,
    #[serde(default = "default_max_query_k")]
    pub max_query_k: usize,
}

impl FaissBackendConfig {
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            default_nlist: default_nlist(),
            default_nprobe: default_nprobe(),
            flat_search_cutoff: default_flat_search_cutoff(),
            max_query_k: default_max_query_k(),
        }
    }
}

const fn default_nlist() -> usize {
    256
}

const fn default_nprobe() -> usize {
    16
}

const fn default_flat_search_cutoff() -> usize {
    256
}

const fn default_max_query_k() -> usize {
    10_000
}
