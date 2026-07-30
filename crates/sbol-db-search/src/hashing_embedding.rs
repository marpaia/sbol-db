//! Deterministic, zero-dependency text vectors for the shipped SBOL baseline.
//!
//! This is deliberately a lexical feature-hashing embedding rather than a
//! pretrained semantic model. It has no download, model file, or network
//! dependency, and its explicit descriptor makes the vector space stable for
//! generated-index provenance. The canonical SBOL projection supplies labels,
//! IDs, names, descriptions, types, and roles as input text.

use async_trait::async_trait;
use sbol_db_search_sdk::{
    DataEgress, EmbeddingBatch, EmbeddingDescriptor, EmbeddingOutput, EmbeddingProvider,
    EmbeddingVector, Normalization, SearchError,
};

/// Stable ID of the built-in lexical SBOL text embedding profile.
pub const BUILTIN_SBOL_TEXT_PROFILE: &str = "builtin.sbol-text-hash.v1";
/// Fixed vector width for the built-in profile.
pub const BUILTIN_SBOL_TEXT_DIMENSION: usize = 256;

/// A signed feature-hashing embedding over ASCII tokens and token trigrams.
///
/// It is an always-available baseline for canonical SBOL metadata. It is not
/// presented as a biological or natural-language semantic model.
#[derive(Clone, Debug)]
pub struct HashingTextEmbeddingProvider {
    descriptor: EmbeddingDescriptor,
}

impl Default for HashingTextEmbeddingProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl HashingTextEmbeddingProvider {
    pub fn new() -> Self {
        Self {
            descriptor: EmbeddingDescriptor {
                id: BUILTIN_SBOL_TEXT_PROFILE.to_owned(),
                provider: "sbol-db".to_owned(),
                model: "signed-feature-hash".to_owned(),
                revision: "fnv1a-ascii-token-trigram-256-v1".to_owned(),
                dimension: BUILTIN_SBOL_TEXT_DIMENSION,
                normalization: Normalization::L2,
                data_egress: DataEgress::None,
            },
        }
    }
}

#[async_trait]
impl EmbeddingProvider for HashingTextEmbeddingProvider {
    fn descriptor(&self) -> &EmbeddingDescriptor {
        &self.descriptor
    }

    async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError> {
        if batch.profile != self.descriptor.id {
            return Err(SearchError::Configuration(format!(
                "embedding batch requests profile {:?}, but provider exposes {:?}",
                batch.profile, self.descriptor.id
            )));
        }
        Ok(EmbeddingOutput {
            vectors: batch
                .inputs
                .iter()
                .map(|input| EmbeddingVector::Dense(embed(&input.text, self.descriptor.dimension)))
                .collect(),
        })
    }
}

fn embed(text: &str, dimension: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dimension];
    for token in ascii_tokens(text) {
        add_feature(&mut vector, token.as_bytes(), 1.0);
        let bytes = token.as_bytes();
        if bytes.len() >= 3 {
            for window in bytes.windows(3) {
                add_feature(&mut vector, window, 0.35);
            }
        }
    }
    l2_normalize(&mut vector);
    vector
}

fn ascii_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = Vec::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() {
            token.push(byte.to_ascii_lowercase());
        } else if !token.is_empty() {
            tokens.push(String::from_utf8(std::mem::take(&mut token)).expect("ASCII token"));
        }
    }
    if !token.is_empty() {
        tokens.push(String::from_utf8(token).expect("ASCII token"));
    }
    tokens
}

fn add_feature(vector: &mut [f32], feature: &[u8], weight: f32) {
    let hash = fnv1a(feature);
    let index = (hash as usize) % vector.len();
    let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
    vector[index] += sign * weight;
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn l2_normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use sbol_db_search_sdk::{EmbeddingInput, EmbeddingInputKind};

    use super::*;

    #[tokio::test]
    async fn embeds_canonical_text_deterministically_without_egress() {
        let provider = HashingTextEmbeddingProvider::new();
        let batch = EmbeddingBatch {
            profile: BUILTIN_SBOL_TEXT_PROFILE.to_owned(),
            inputs: vec![EmbeddingInput {
                kind: EmbeddingInputKind::Document,
                text: "Name: pTet; Role: promoter".to_owned(),
            }],
        };
        let first = provider.embed(batch.clone()).await.unwrap();
        let second = provider.embed(batch).await.unwrap();
        assert_eq!(first, second);
        let EmbeddingVector::Dense(vector) = &first.vectors[0] else {
            panic!("built-in provider must return dense vectors");
        };
        assert_eq!(vector.len(), BUILTIN_SBOL_TEXT_DIMENSION);
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert_eq!(provider.descriptor().data_egress, DataEgress::None);
    }

    #[tokio::test]
    async fn rejects_a_different_profile() {
        let provider = HashingTextEmbeddingProvider::new();
        let result = provider
            .embed(EmbeddingBatch {
                profile: "wrong".to_owned(),
                inputs: Vec::new(),
            })
            .await;
        assert!(matches!(result, Err(SearchError::Configuration(_))));
    }
}
