//! Exercise the exact contextual model shipped by the default image against
//! the versioned SBOL semantic release suite.
//!
//! This fixture is intentionally small and synthetic. It is a deterministic
//! release guard for model bytes, the canonical text projection shape, and
//! relevance judgments; it is not evidence about live SynBioHub traffic.

use std::cmp::Ordering;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_embedding_fastembed::{
    FastEmbedPooling, FastEmbedProvider, FastEmbedProviderConfig, LocalFastEmbedBundleConfig,
};
use sbol_db_search::HashingTextEmbeddingProvider;
use sbol_db_search_eval::{
    compare, evaluate_strategy, EvaluationConfig, EvaluationSuite, QualityGate,
};
use sbol_db_search_sdk::{
    DataEgress, DocumentId, EmbeddingBatch, EmbeddingInput, EmbeddingInputKind, EmbeddingProvider,
    EmbeddingVector, ExecutionMetadata, FilterCapability, PaginationCapability, ScoreKind,
    SearchContext, SearchError, SearchHit, SearchInput, SearchInputKind, SearchPage, SearchRequest,
    SearchStrategy, StrategyCapabilities, StrategyDescriptor, StrategyRef, StrategyRequirements,
    Total, TotalCapability,
};
use serde::Deserialize;

const CORPUS_JSON: &str = include_str!("../fixtures/sbol-semantic-release-v1-corpus.json");
const SUITE_JSON: &str = include_str!("../fixtures/sbol-semantic-release-v1.json");
const MODEL_DIR_ENV: &str = "SBOL_DB_BGE_SMALL_MODEL_DIR";
const PROFILE_ID: &str = "builtin.sbol-text-bge-small.v1";
const PROFILE_REVISION: &str =
    "sha3-256:bf577972c34b37578aa42965fa8401d5538f4b4c007c810332e936548658c7b3";
const MODEL_SOURCE: &str =
    "Qdrant/bge-small-en-v1.5-onnx-Q@52398278842ec682c6f32300af41344b1c0b0bb2";

#[derive(Clone, Deserialize)]
struct Corpus {
    schema_version: u32,
    id: String,
    revision: String,
    license: String,
    documents: Vec<Document>,
}

#[derive(Clone, Deserialize)]
struct Document {
    document_id: String,
    uri: String,
    name: String,
    description: String,
    object_type: String,
    text: String,
}

/// A small, direct strategy so the release test measures precisely the
/// provider and canonical-text embeddings, independent of a storage backend.
struct CorpusEmbeddingStrategy {
    descriptor: StrategyDescriptor,
    provider: Arc<dyn EmbeddingProvider>,
    documents: Vec<Document>,
    document_vectors: Vec<Vec<f32>>,
}

impl CorpusEmbeddingStrategy {
    async fn new(
        id: &str,
        version: &str,
        provider: Arc<dyn EmbeddingProvider>,
        documents: Vec<Document>,
    ) -> Result<Self, SearchError> {
        let document_vectors = dense_vectors(
            provider
                .embed(EmbeddingBatch {
                    profile: provider.descriptor().id.clone(),
                    inputs: documents
                        .iter()
                        .map(|document| EmbeddingInput {
                            kind: EmbeddingInputKind::Document,
                            text: document.text.clone(),
                        })
                        .collect(),
                })
                .await?,
            provider.descriptor().dimension,
        )?;
        if document_vectors.len() != documents.len() {
            return Err(SearchError::Backend(
                "embedding provider returned a different number of document vectors".to_owned(),
            ));
        }
        Ok(Self {
            descriptor: StrategyDescriptor {
                id: id.to_owned(),
                version: version.to_owned(),
                display_name: id.to_owned(),
                description: "Release-fixture dense cosine retrieval".to_owned(),
                capabilities: StrategyCapabilities {
                    inputs: vec![SearchInputKind::Text],
                    filters: Vec::new(),
                    filter_execution: FilterCapability::None,
                    pagination: PaginationCapability::FirstPageOnly,
                    totals: TotalCapability::Exact,
                    deterministic: true,
                    explanations: false,
                    data_egress: DataEgress::None,
                },
                requirements: StrategyRequirements {
                    embedding_profiles: vec![provider.descriptor().id.clone()],
                    vector_indexes: Vec::new(),
                    candidate_sources: Vec::new(),
                },
            },
            provider,
            documents,
            document_vectors,
        })
    }
}

#[async_trait]
impl SearchStrategy for CorpusEmbeddingStrategy {
    fn descriptor(&self) -> &StrategyDescriptor {
        &self.descriptor
    }

    async fn search(
        &self,
        _context: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        let SearchInput::Text { text } = request.query else {
            return Err(SearchError::Unsupported(
                "the release fixture accepts text queries only".to_owned(),
            ));
        };
        let query = dense_vectors(
            self.provider
                .embed(EmbeddingBatch {
                    profile: self.provider.descriptor().id.clone(),
                    inputs: vec![EmbeddingInput {
                        kind: EmbeddingInputKind::Query,
                        text,
                    }],
                })
                .await?,
            self.provider.descriptor().dimension,
        )?
        .into_iter()
        .next()
        .ok_or_else(|| {
            SearchError::Backend("embedding provider did not return a query vector".to_owned())
        })?;

        let mut ranked: Vec<_> = self
            .documents
            .iter()
            .zip(&self.document_vectors)
            .map(|(document, vector)| (cosine_similarity(&query, vector), document))
            .collect();
        ranked.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.document_id.cmp(&right.document_id))
        });

        let limit = request.page.limit.min(ranked.len());
        let items = ranked
            .into_iter()
            .take(limit)
            .map(|(score, document)| SearchHit {
                document_id: DocumentId(document.document_id.clone()),
                uri: document.uri.clone(),
                graph: None,
                score,
                score_kind: ScoreKind::CosineSimilarity,
                display_id: None,
                version: None,
                name: Some(document.name.clone()),
                description: Some(document.description.clone()),
                object_types: vec![document.object_type.clone()],
                evidence: Vec::new(),
            })
            .collect();

        Ok(SearchPage {
            strategy: StrategyRef {
                id: self.descriptor.id.clone(),
                version: self.descriptor.version.clone(),
            },
            items,
            total: Total::Exact(self.documents.len()),
            next_cursor: None,
            execution: ExecutionMetadata::default(),
        })
    }
}

fn dense_vectors(
    output: sbol_db_search_sdk::EmbeddingOutput,
    dimension: usize,
) -> Result<Vec<Vec<f32>>, SearchError> {
    output
        .vectors
        .into_iter()
        .map(|vector| match vector {
            EmbeddingVector::Dense(vector) if vector.len() == dimension => Ok(vector),
            EmbeddingVector::Dense(vector) => Err(SearchError::Backend(format!(
                "embedding provider returned {} dimensions, expected {dimension}",
                vector.len()
            ))),
            _ => Err(SearchError::Backend(
                "release fixture requires dense embeddings".to_owned(),
            )),
        })
        .collect()
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    let dot = left
        .iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm * right_norm)
    }
}

fn builtin_provider(directory: PathBuf) -> Result<FastEmbedProvider, SearchError> {
    FastEmbedProvider::from_local_bundle(
        FastEmbedProviderConfig {
            id: PROFILE_ID.to_owned(),
            model: MODEL_SOURCE.to_owned(),
            revision: PROFILE_REVISION.to_owned(),
            dimension: 384,
            normalization: sbol_db_search_sdk::Normalization::L2,
            query_prefix: None,
            document_prefix: None,
            batch_size: 32,
        },
        &LocalFastEmbedBundleConfig {
            directory,
            onnx_file: "model_optimized.onnx".to_owned(),
            pooling: FastEmbedPooling::Cls,
            max_length: 512,
            intra_threads: Some(2),
        },
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let corpus: Corpus = serde_json::from_str(CORPUS_JSON)?;
    if corpus.schema_version != 1
        || corpus.id != "sbol-db-semantic-release"
        || corpus.revision != "1"
        || corpus.license != "CC0-1.0"
    {
        return Err("unexpected semantic release corpus identity".into());
    }
    let suite: EvaluationSuite = serde_json::from_str(SUITE_JSON)?;
    let model_directory = std::env::var_os(MODEL_DIR_ENV)
        .map(PathBuf::from)
        .ok_or("SBOL_DB_BGE_SMALL_MODEL_DIR must identify the verified BGE-small bundle")?;
    let documents = corpus.documents;
    let baseline = CorpusEmbeddingStrategy::new(
        "release.sbol-text-hash.v1",
        "1",
        Arc::new(HashingTextEmbeddingProvider::new()),
        documents.clone(),
    )
    .await?;
    let candidate = CorpusEmbeddingStrategy::new(
        "builtin.sbol-text-vector.v2",
        "2",
        Arc::new(builtin_provider(model_directory)?),
        documents,
    )
    .await?;
    let context = SearchContext::new(sbol_db_search_sdk::SearchScope::Union, Default::default());
    let config = EvaluationConfig {
        cutoffs: vec![1, 3],
    };
    let baseline_report = evaluate_strategy(&baseline, context.clone(), &suite, &config).await?;
    let candidate_report = evaluate_strategy(&candidate, context, &suite, &config).await?;
    let comparison = compare(
        &suite,
        &baseline_report,
        &candidate_report,
        &QualityGate {
            cutoff: 3,
            min_cases: 8,
            // First Linux release measurement: candidate 0.97504 nDCG@3,
            // baseline 0.96347. Keep 0.005 headroom for platform-level
            // floating-point variation, but require the candidate to remain
            // useful in absolute terms and not trail its lexical baseline.
            min_candidate_mean_ndcg: 0.97,
            min_mean_ndcg_delta: 0.0,
            regression_tolerance: 0.02,
            max_regressed_fraction: 0.0,
        },
    )?;

    println!(
        "baseline: {}",
        serde_json::to_string_pretty(&baseline_report)?
    );
    println!(
        "candidate: {}",
        serde_json::to_string_pretty(&candidate_report)?
    );
    println!("comparison: {}", serde_json::to_string_pretty(&comparison)?);
    Ok(())
}
