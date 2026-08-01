use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::History;

#[derive(Clone, Debug, Serialize)]
pub struct RunManifest {
    pub schema_version: u16,
    pub run_id: uuid::Uuid,
    pub scenario: String,
    pub seed: u64,
    pub driver: String,
    pub cluster_id: uuid::Uuid,
    pub node_count: usize,
    pub corpus_commit: Option<String>,
    pub corpus_fingerprint: Option<String>,
}

pub struct ArtifactBundle {
    root: PathBuf,
}

impl ArtifactBundle {
    pub fn create(root: impl AsRef<Path>, manifest: &RunManifest) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)
            .with_context(|| format!("creating artifact directory {}", root.display()))?;
        write_json(root.join("manifest.json"), manifest)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_history(&self, history: &History) -> Result<()> {
        let mut encoded = String::new();
        for event in history {
            encoded.push_str(&serde_json::to_string(event)?);
            encoded.push('\n');
        }
        fs::write(self.root.join("history.jsonl"), encoded).context("writing operation history")
    }

    pub fn write_json(&self, name: &str, value: &impl Serialize) -> Result<()> {
        write_json(self.root.join(name), value)
    }
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    let encoded = serde_json::to_vec_pretty(value).context("encoding artifact JSON")?;
    fs::write(&path, encoded).with_context(|| format!("writing {}", path.display()))
}
