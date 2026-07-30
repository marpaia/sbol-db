//! Reference dense-embedding strategy built entirely from public SDK
//! extension points.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_search_sdk::{
    DistanceMetric, EmbeddingBatch, EmbeddingInput, EmbeddingInputKind, EmbeddingProvider,
    EmbeddingVector, Evidence, FilterCapability, FilterKind, HydratedDocument,
    PaginationCapability, ScoreKind, SearchContext, SearchError, SearchHit, SearchInput,
    SearchInputKind, SearchPage, SearchRequest, SearchScope, SearchStrategy, StrategyCapabilities,
    StrategyDescriptor, StrategyRef, StrategyRequirements, Total, TotalCapability, VectorFilter,
    VectorQuery, VectorValue,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Wiring for one dense semantic-search strategy instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingStrategyConfig {
    pub id: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub embedding_profile: String,
    pub vector_index: String,
    pub vector_name: String,
    #[serde(default = "default_graph_payload_field")]
    pub graph_payload_field: String,
    pub distance: DistanceMetric,
}

fn default_graph_payload_field() -> String {
    "graph".to_owned()
}

/// Text-query embedding, vector retrieval, and primary-store hydration.
///
/// This is both a usable baseline and a small example of the SDK surface a
/// third-party strategy implements. It owns the embedding plugin, but receives
/// vector search and document hydration as request-scoped services so the
/// application remains the authorization boundary.
pub struct EmbeddingSearchStrategy {
    descriptor: StrategyDescriptor,
    config: EmbeddingStrategyConfig,
    embedding: Arc<dyn EmbeddingProvider>,
}

impl EmbeddingSearchStrategy {
    pub fn new(
        config: EmbeddingStrategyConfig,
        embedding: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, SearchError> {
        validate_config(&config, embedding.as_ref())?;
        let descriptor = StrategyDescriptor {
            id: config.id.clone(),
            version: config.version.clone(),
            display_name: config.display_name.clone(),
            description: config.description.clone(),
            capabilities: StrategyCapabilities {
                inputs: vec![SearchInputKind::Text],
                filters: vec![FilterKind::Graph],
                filter_execution: FilterCapability::Native,
                pagination: PaginationCapability::Cursor,
                totals: TotalCapability::Unknown,
                // Some providers may be deterministic, but the embedding
                // contract does not currently promise that property.
                deterministic: false,
                explanations: true,
                data_egress: embedding.descriptor().data_egress,
            },
            requirements: StrategyRequirements {
                embedding_profiles: vec![config.embedding_profile.clone()],
                vector_indexes: vec![config.vector_index.clone()],
                candidate_sources: Vec::new(),
            },
        };
        Ok(Self {
            descriptor,
            config,
            embedding,
        })
    }
}

#[async_trait]
impl SearchStrategy for EmbeddingSearchStrategy {
    fn descriptor(&self) -> &StrategyDescriptor {
        &self.descriptor
    }

    async fn search(
        &self,
        ctx: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        let SearchInput::Text { text } = &request.query else {
            return Err(SearchError::Unsupported(
                "embedding strategy accepts text queries only".to_owned(),
            ));
        };
        if text.trim().is_empty() {
            return Err(SearchError::InvalidRequest(
                "embedding search text cannot be empty".to_owned(),
            ));
        }

        let output = self
            .embedding
            .embed(EmbeddingBatch {
                profile: self.config.embedding_profile.clone(),
                inputs: vec![EmbeddingInput {
                    kind: EmbeddingInputKind::Query,
                    text: text.clone(),
                }],
            })
            .await?;
        let vector = one_dense_vector(output.vectors, self.embedding.as_ref())?;
        let scope = ctx.scope().clone();
        let vectors = Arc::clone(ctx.vectors()?);
        let documents = Arc::clone(ctx.documents()?);
        let page = vectors
            .query(VectorQuery {
                index: self.config.vector_index.clone(),
                vector_name: self.config.vector_name.clone(),
                vector: VectorValue::Dense(vector),
                filter: graph_filter(&request, &self.config.graph_payload_field),
                limit: request.page.limit,
                cursor: request.page.cursor.clone(),
                score_threshold: None,
                parameters: BTreeMap::new(),
            })
            .await?;

        let requested_ids = page
            .items
            .iter()
            .map(|item| item.document_id.clone())
            .collect::<Vec<_>>();
        let hydrated = documents.hydrate(requested_ids.clone()).await?;
        let hydrated = index_hydrated(hydrated, &requested_ids, &scope, &request)?;
        let mut warnings = Vec::new();
        let mut items = Vec::with_capacity(page.items.len());
        for candidate in page.items {
            let Some(document) = hydrated.get(&candidate.document_id) else {
                warnings.push(format!(
                    "authorized primary store did not hydrate document {:?}",
                    candidate.document_id.0
                ));
                continue;
            };
            let evidence = request.options.explain.then(|| {
                vec![Evidence {
                    source: self.descriptor.id.clone(),
                    rank: Some(items.len() + 1),
                    score: Some(candidate.score),
                    details: BTreeMap::from([
                        (
                            "embedding_profile".to_owned(),
                            json!(self.config.embedding_profile),
                        ),
                        ("vector_index".to_owned(), json!(self.config.vector_index)),
                        ("vector_name".to_owned(), json!(self.config.vector_name)),
                    ]),
                }]
            });
            items.push(search_hit(
                document,
                candidate.score,
                score_kind(self.config.distance),
                evidence.unwrap_or_default(),
            ));
        }

        Ok(SearchPage {
            strategy: StrategyRef {
                id: self.descriptor.id.clone(),
                version: self.descriptor.version.clone(),
            },
            items,
            total: Total::Unknown,
            next_cursor: page.next_cursor,
            execution: sbol_db_search_sdk::ExecutionMetadata {
                warnings,
                ..Default::default()
            },
        })
    }
}

fn validate_config(
    config: &EmbeddingStrategyConfig,
    embedding: &dyn EmbeddingProvider,
) -> Result<(), SearchError> {
    for (name, value) in [
        ("id", &config.id),
        ("version", &config.version),
        ("embedding_profile", &config.embedding_profile),
        ("vector_index", &config.vector_index),
        ("vector_name", &config.vector_name),
        ("graph_payload_field", &config.graph_payload_field),
    ] {
        if value.trim().is_empty() {
            return Err(SearchError::Configuration(format!(
                "embedding strategy {name} cannot be empty"
            )));
        }
    }
    let descriptor = embedding.descriptor();
    if descriptor.id != config.embedding_profile {
        return Err(SearchError::Configuration(format!(
            "strategy requests embedding profile {:?}, but provider exposes {:?}",
            config.embedding_profile, descriptor.id
        )));
    }
    if descriptor.dimension == 0 {
        return Err(SearchError::Configuration(format!(
            "embedding profile {:?} declares a zero dimension",
            descriptor.id
        )));
    }
    if matches!(
        config.distance,
        DistanceMetric::Hamming | DistanceMetric::Jaccard
    ) {
        return Err(SearchError::Configuration(format!(
            "dense embedding strategy does not support {:?} distance",
            config.distance
        )));
    }
    Ok(())
}

fn one_dense_vector(
    vectors: Vec<EmbeddingVector>,
    embedding: &dyn EmbeddingProvider,
) -> Result<Vec<f32>, SearchError> {
    if vectors.len() != 1 {
        return Err(SearchError::Backend(format!(
            "embedding profile {:?} returned {} vectors for one query",
            embedding.descriptor().id,
            vectors.len()
        )));
    }
    let EmbeddingVector::Dense(vector) = vectors.into_iter().next().expect("length checked") else {
        return Err(SearchError::Configuration(format!(
            "embedding profile {:?} did not return a dense query vector",
            embedding.descriptor().id
        )));
    };
    if vector.len() != embedding.descriptor().dimension {
        return Err(SearchError::Backend(format!(
            "embedding profile {:?} returned dimension {}, expected {}",
            embedding.descriptor().id,
            vector.len(),
            embedding.descriptor().dimension
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(SearchError::Backend(format!(
            "embedding profile {:?} returned a non-finite value",
            embedding.descriptor().id
        )));
    }
    Ok(vector)
}

fn graph_filter(request: &SearchRequest, field: &str) -> Option<VectorFilter> {
    (!request.filters.graphs.is_empty()).then(|| VectorFilter::Any {
        field: field.to_owned(),
        values: request
            .filters
            .graphs
            .iter()
            .cloned()
            .map(Value::String)
            .collect(),
    })
}

fn index_hydrated(
    documents: Vec<HydratedDocument>,
    requested: &[sbol_db_search_sdk::DocumentId],
    scope: &SearchScope,
    request: &SearchRequest,
) -> Result<HashMap<sbol_db_search_sdk::DocumentId, HydratedDocument>, SearchError> {
    let requested = requested.iter().cloned().collect::<HashSet<_>>();
    let mut indexed = HashMap::with_capacity(documents.len());
    for document in documents {
        if !requested.contains(&document.document_id) {
            return Err(SearchError::Backend(format!(
                "document hydrator returned unrequested document {:?}",
                document.document_id.0
            )));
        }
        validate_hydrated_scope(&document, scope, request)?;
        if indexed
            .insert(document.document_id.clone(), document)
            .is_some()
        {
            return Err(SearchError::Backend(
                "document hydrator returned a duplicate document".to_owned(),
            ));
        }
    }
    Ok(indexed)
}

fn validate_hydrated_scope(
    document: &HydratedDocument,
    scope: &SearchScope,
    request: &SearchRequest,
) -> Result<(), SearchError> {
    if let SearchScope::Only(graphs) = scope {
        let authorized = document
            .graph
            .as_ref()
            .is_some_and(|graph| graphs.contains(graph));
        if !authorized {
            return Err(SearchError::Backend(format!(
                "document hydrator violated authorization scope for {:?}",
                document.document_id.0
            )));
        }
    }
    if !request.filters.graphs.is_empty()
        && !document
            .graph
            .as_ref()
            .is_some_and(|graph| request.filters.graphs.contains(graph))
    {
        return Err(SearchError::Backend(format!(
            "vector backend violated requested graph filter for {:?}",
            document.document_id.0
        )));
    }
    Ok(())
}

fn score_kind(distance: DistanceMetric) -> ScoreKind {
    match distance {
        DistanceMetric::Cosine => ScoreKind::CosineSimilarity,
        DistanceMetric::Dot => ScoreKind::DotProduct,
        DistanceMetric::Euclidean | DistanceMetric::Manhattan => ScoreKind::NegativeDistance,
        DistanceMetric::Hamming | DistanceMetric::Jaccard => {
            unreachable!("unsupported dense distances are rejected at construction")
        }
    }
}

fn search_hit(
    document: &HydratedDocument,
    score: f32,
    score_kind: ScoreKind,
    evidence: Vec<Evidence>,
) -> SearchHit {
    SearchHit {
        document_id: document.document_id.clone(),
        uri: document.uri.clone(),
        graph: document.graph.clone(),
        score,
        score_kind,
        display_id: document.display_id.clone(),
        version: document.version.clone(),
        name: document.name.clone(),
        description: document.description.clone(),
        object_types: document.object_types.clone(),
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use sbol_db_search_sdk::{
        DataEgress, DocumentId, EmbeddingDescriptor, EmbeddingOutput, Normalization, PageRequest,
        ScopedDocumentHydrator, SearchBudget, SearchFilters, SearchOptions, VectorChange,
        VectorIndexAdmin,
    };
    use sbol_db_vector_flat::ExactFlatVectorBackend;

    use crate::VectorRouter;

    use super::*;

    struct StubEmbedding {
        descriptor: EmbeddingDescriptor,
    }

    #[async_trait]
    impl EmbeddingProvider for StubEmbedding {
        fn descriptor(&self) -> &EmbeddingDescriptor {
            &self.descriptor
        }

        async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError> {
            assert_eq!(batch.profile, self.descriptor.id);
            Ok(EmbeddingOutput {
                vectors: batch
                    .inputs
                    .into_iter()
                    .map(|_| EmbeddingVector::Dense(vec![1.0, 0.0]))
                    .collect(),
            })
        }
    }

    struct StubHydrator {
        documents: Mutex<HashMap<DocumentId, HydratedDocument>>,
    }

    #[async_trait]
    impl ScopedDocumentHydrator for StubHydrator {
        async fn hydrate(
            &self,
            document_ids: Vec<DocumentId>,
        ) -> Result<Vec<HydratedDocument>, SearchError> {
            let documents = self.documents.lock().unwrap();
            Ok(document_ids
                .iter()
                .filter_map(|id| documents.get(id).cloned())
                .collect())
        }
    }

    fn embedding() -> Arc<dyn EmbeddingProvider> {
        Arc::new(StubEmbedding {
            descriptor: EmbeddingDescriptor {
                id: "test.embedding".to_owned(),
                provider: "test".to_owned(),
                model: "two-axis".to_owned(),
                revision: "fixed".to_owned(),
                dimension: 2,
                normalization: Normalization::L2,
                data_egress: DataEgress::None,
            },
        })
    }

    fn config() -> EmbeddingStrategyConfig {
        EmbeddingStrategyConfig {
            id: "semantic.v1".to_owned(),
            version: "1".to_owned(),
            display_name: "Semantic".to_owned(),
            description: "Dense semantic search".to_owned(),
            embedding_profile: "test.embedding".to_owned(),
            vector_index: "components".to_owned(),
            vector_name: "content".to_owned(),
            graph_payload_field: "graph".to_owned(),
            distance: DistanceMetric::Cosine,
        }
    }

    fn vector_point(id: &str, graph: &str, vector: Vec<f32>) -> VectorChange {
        VectorChange::Upsert {
            document_id: DocumentId(id.to_owned()),
            vectors: BTreeMap::from([("content".to_owned(), VectorValue::Dense(vector))]),
            payload: BTreeMap::from([("graph".to_owned(), json!(graph))]),
        }
    }

    fn document(id: &str, graph: &str) -> HydratedDocument {
        HydratedDocument {
            document_id: DocumentId(id.to_owned()),
            uri: format!("https://example.test/{id}"),
            graph: Some(graph.to_owned()),
            display_id: Some(id.to_owned()),
            version: None,
            name: None,
            description: None,
            object_types: vec!["Component".to_owned()],
        }
    }

    #[tokio::test]
    async fn searches_vectors_then_hydrates_authoritative_documents() {
        let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
        let handle = backend
            .create_generation(sbol_db_search_sdk::IndexGenerationSpec {
                artifact_id: "components".to_owned(),
                generation: "one".to_owned(),
                vector_name: "content".to_owned(),
                dimension: 2,
                distance: DistanceMetric::Cosine,
                embedding: None,
                parameters: BTreeMap::new(),
            })
            .await
            .unwrap();
        backend
            .apply(
                &handle,
                vec![
                    vector_point("public", "public", vec![1.0, 0.0]),
                    vector_point("private", "private", vec![1.0, 0.0]),
                    vector_point("other", "public", vec![0.0, 1.0]),
                ],
            )
            .await
            .unwrap();
        backend.activate(&handle).await.unwrap();
        let router = VectorRouter::new()
            .register("components", backend, "graph")
            .unwrap();
        let scope = SearchScope::Only(vec!["public".to_owned()]);
        let hydrator = Arc::new(StubHydrator {
            documents: Mutex::new(HashMap::from([
                (
                    DocumentId("public".to_owned()),
                    document("public", "public"),
                ),
                (DocumentId("other".to_owned()), document("other", "public")),
            ])),
        });
        let strategy = EmbeddingSearchStrategy::new(config(), embedding()).unwrap();
        let result = strategy
            .search(
                SearchContext::new(scope.clone(), SearchBudget::default())
                    .with_vectors(router.scoped(scope))
                    .with_documents(hydrator),
                SearchRequest {
                    strategy: Some("semantic.v1".to_owned()),
                    query: SearchInput::Text {
                        text: "promoter".to_owned(),
                    },
                    filters: SearchFilters::default(),
                    page: PageRequest {
                        limit: 10,
                        cursor: None,
                    },
                    options: SearchOptions {
                        explain: true,
                        timeout_ms: None,
                    },
                },
            )
            .await
            .unwrap();

        assert_eq!(
            result
                .items
                .iter()
                .map(|item| item.document_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["public", "other"]
        );
        assert_eq!(result.items[0].score_kind, ScoreKind::CosineSimilarity);
        assert_eq!(result.items[0].evidence[0].source, "semantic.v1");
        assert!(matches!(result.total, Total::Unknown));
    }

    #[test]
    fn validates_embedding_profile_wiring() {
        let mut wrong = config();
        wrong.embedding_profile = "wrong".to_owned();
        assert!(matches!(
            EmbeddingSearchStrategy::new(wrong, embedding()),
            Err(SearchError::Configuration(_))
        ));
    }
}
