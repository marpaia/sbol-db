//! Process-level construction of built-in search plugins from JSON config.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use sbol_db_embedding_fastembed::{
    FastEmbedProvider, FastEmbedProviderConfig, LocalFastEmbedBundleConfig,
};
use sbol_db_search::{SearchDeploymentBuilder, SearchTopologyConfig};
use sbol_db_vector_flat::ExactFlatVectorBackend;
use sbol_db_vector_qdrant::{QdrantRemoteBackend, QdrantRemoteConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchProcessConfig {
    topology: SearchTopologyConfig,
    #[serde(default)]
    embeddings: Vec<EmbeddingPluginConfig>,
    #[serde(default)]
    vector_backends: Vec<VectorBackendPluginConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum EmbeddingPluginConfig {
    FastembedLocal {
        profile: FastEmbedProviderConfig,
        bundle: LocalFastEmbedBundleConfig,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum VectorBackendPluginConfig {
    ExactFlat {
        id: String,
    },
    Qdrant {
        config: QdrantRemoteConfig,
        /// Read the API key from this environment variable. Keeping the key out
        /// of the JSON file is the recommended deployment shape.
        #[serde(default)]
        api_key_env: Option<String>,
    },
}

pub async fn load_builder(path: &Path) -> Result<SearchDeploymentBuilder> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading search config {}", path.display()))?;
    let config: SearchProcessConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing search config {}", path.display()))?;
    let mut builder = SearchDeploymentBuilder::new(config.topology);

    for embedding in config.embeddings {
        match embedding {
            EmbeddingPluginConfig::FastembedLocal { profile, bundle } => {
                let provider = tokio::task::spawn_blocking(move || {
                    FastEmbedProvider::from_local_bundle(profile, &bundle)
                })
                .await
                .context("joining FastEmbed model initialization task")??;
                builder = builder.register_embedding(Arc::new(provider))?;
            }
        }
    }

    for backend in config.vector_backends {
        match backend {
            VectorBackendPluginConfig::ExactFlat { id } => {
                builder =
                    builder.register_vector_backend(Arc::new(ExactFlatVectorBackend::new(id)))?;
            }
            VectorBackendPluginConfig::Qdrant {
                mut config,
                api_key_env,
            } => {
                if config.api_key.is_some() && api_key_env.is_some() {
                    bail!("Qdrant config cannot set both api_key and api_key_env");
                }
                if let Some(variable) = api_key_env {
                    if variable.trim().is_empty() {
                        bail!("Qdrant api_key_env cannot be empty");
                    }
                    config.api_key = Some(std::env::var(&variable).with_context(|| {
                        format!("reading Qdrant API key environment variable {variable:?}")
                    })?);
                }
                builder =
                    builder.register_vector_backend(Arc::new(QdrantRemoteBackend::new(config)?))?;
            }
        }
    }

    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tagged_plugin_configuration() {
        let config: SearchProcessConfig = serde_json::from_value(serde_json::json!({
            "topology": {
                "default_strategy": "semantic.v1",
                "indexes": [{
                    "index": "components",
                    "backend": "flat",
                    "embedding_profile": "local.test.v1",
                    "vector_name": "content"
                }],
                "embedding_strategies": []
            },
            "embeddings": [{
                "kind": "fastembed_local",
                "profile": {
                    "id": "local.test.v1",
                    "model": "test-model",
                    "revision": "sha3-256:abc",
                    "dimension": 384,
                    "normalization": "l2"
                },
                "bundle": {"directory": "/models/test"}
            }],
            "vector_backends": [{"kind": "exact_flat", "id": "flat"}]
        }))
        .unwrap();

        assert_eq!(config.topology.indexes[0].graph_payload_field, "graph");
        assert_eq!(config.embeddings.len(), 1);
        assert_eq!(config.vector_backends.len(), 1);
    }
}
