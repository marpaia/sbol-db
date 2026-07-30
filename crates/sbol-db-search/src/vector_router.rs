//! Authorization-preserving dispatch from logical vector index names to
//! backend plugins.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_search_sdk::{
    ScopedVectorSearch, SearchError, SearchScope, VectorBackend, VectorError, VectorFilter,
    VectorQuery, VectorSearchPage,
};
use serde_json::Value;

#[derive(Clone)]
struct IndexBinding {
    backend: Arc<dyn VectorBackend>,
    graph_payload_field: String,
}

/// Immutable routing table assembled by the application at startup.
///
/// A strategy addresses a logical index (for example `components`) while this
/// table chooses the deployment backend. The same strategy therefore works
/// against the exact-flat oracle, Qdrant, or a future pgvector/FAISS adapter.
#[derive(Clone, Default)]
pub struct VectorRouter {
    bindings: HashMap<String, IndexBinding>,
}

impl VectorRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        mut self,
        index: impl Into<String>,
        backend: Arc<dyn VectorBackend>,
        graph_payload_field: impl Into<String>,
    ) -> Result<Self, SearchError> {
        let index = index.into();
        let graph_payload_field = graph_payload_field.into();
        if index.trim().is_empty() || graph_payload_field.trim().is_empty() {
            return Err(SearchError::Configuration(
                "vector index and graph payload field cannot be empty".to_owned(),
            ));
        }
        if self.bindings.contains_key(&index) {
            return Err(SearchError::Configuration(format!(
                "duplicate vector index binding {index:?}"
            )));
        }
        self.bindings.insert(
            index,
            IndexBinding {
                backend,
                graph_payload_field,
            },
        );
        Ok(self)
    }

    pub fn scoped(&self, scope: SearchScope) -> Arc<dyn ScopedVectorSearch> {
        Arc::new(ScopedRouter {
            bindings: self.bindings.clone(),
            scope,
        })
    }
}

struct ScopedRouter {
    bindings: HashMap<String, IndexBinding>,
    scope: SearchScope,
}

#[async_trait]
impl ScopedVectorSearch for ScopedRouter {
    async fn query(&self, mut query: VectorQuery) -> Result<VectorSearchPage, VectorError> {
        let binding = self.bindings.get(&query.index).ok_or_else(|| {
            VectorError::Configuration(format!(
                "no vector backend is bound for logical index {:?}",
                query.index
            ))
        })?;

        let authorized = match &self.scope {
            SearchScope::Union => None,
            SearchScope::Only(graphs) if graphs.is_empty() => {
                return Ok(VectorSearchPage {
                    items: Vec::new(),
                    next_cursor: None,
                });
            }
            SearchScope::Only(graphs) => Some(VectorFilter::Any {
                field: binding.graph_payload_field.clone(),
                values: graphs.iter().cloned().map(Value::String).collect(),
            }),
        };
        query.filter = conjoin(authorized, query.filter);
        binding.backend.query(query).await
    }
}

fn conjoin(left: Option<VectorFilter>, right: Option<VectorFilter>) -> Option<VectorFilter> {
    match (left, right) {
        (None, None) => None,
        (Some(filter), None) | (None, Some(filter)) => Some(filter),
        (Some(left), Some(right)) => Some(VectorFilter::And {
            clauses: vec![left, right],
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use sbol_db_search_sdk::{
        DistanceMetric, DocumentId, IndexGenerationSpec, VectorChange, VectorIndexAdmin,
        VectorValue,
    };
    use sbol_db_vector_flat::ExactFlatVectorBackend;
    use serde_json::json;

    use super::*;

    async fn router() -> VectorRouter {
        let backend = Arc::new(ExactFlatVectorBackend::new("flat"));
        let handle = backend
            .create_generation(IndexGenerationSpec {
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
                vec![point("public", "public"), point("private", "private")],
            )
            .await
            .unwrap();
        backend.activate(&handle).await.unwrap();
        VectorRouter::new()
            .register("components", backend, "graph")
            .unwrap()
    }

    fn point(id: &str, graph: &str) -> VectorChange {
        VectorChange::Upsert {
            document_id: DocumentId(id.to_owned()),
            vectors: BTreeMap::from([("content".to_owned(), VectorValue::Dense(vec![1.0, 0.0]))]),
            payload: BTreeMap::from([("graph".to_owned(), json!(graph))]),
        }
    }

    fn query(filter: Option<VectorFilter>) -> VectorQuery {
        VectorQuery {
            index: "components".to_owned(),
            vector_name: "content".to_owned(),
            vector: VectorValue::Dense(vec![1.0, 0.0]),
            filter,
            limit: 10,
            cursor: None,
            score_threshold: None,
            parameters: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn caller_filter_cannot_widen_authorization_scope() {
        let vectors = router()
            .await
            .scoped(SearchScope::Only(vec!["public".to_owned()]));
        let result = vectors
            .query(query(Some(VectorFilter::Or {
                clauses: vec![
                    VectorFilter::Match {
                        field: "graph".to_owned(),
                        value: json!("public"),
                    },
                    VectorFilter::Match {
                        field: "graph".to_owned(),
                        value: json!("private"),
                    },
                ],
            })))
            .await
            .unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].document_id.0, "public");
    }

    #[tokio::test]
    async fn empty_authorization_scope_does_not_query_backend() {
        let result = VectorRouter::new()
            .scoped(SearchScope::Only(Vec::new()))
            .query(query(None))
            .await;

        // Index resolution still happens first, so configuration errors are
        // not hidden by an empty scope.
        assert!(matches!(result, Err(VectorError::Configuration(_))));

        let result = router()
            .await
            .scoped(SearchScope::Only(Vec::new()))
            .query(query(None))
            .await
            .unwrap();
        assert!(result.items.is_empty());
    }
}
