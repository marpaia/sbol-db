use std::fs;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_search_sdk::{
    DocumentId, EmbeddingBatch, EmbeddingInput, EmbeddingInputKind, EmbeddingVector,
    HydratedDocument, ScopedDocumentHydrator, ScopedVectorSearch, ScoreKind, SearchBudget,
    SearchContext, SearchError, SearchRequest, SearchScope, VectorError, VectorFilter, VectorQuery,
    VectorSearchHit, VectorSearchPage, VectorValue,
};
use serde_json::json;

use crate::{load_plugin, PythonSearchPluginConfig};

#[tokio::test(flavor = "multi_thread")]
async fn loads_provider_and_native_strategy_from_python() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("fixture_plugin.py"),
        r#"
class Embedding:
    def embed(self, texts, *, kind):
        scale = 1.0 if kind == "query" else 2.0
        return [[scale, 0.0] for _ in texts]

def register(search):
    search.add_embedding(
        Embedding(),
        id="python.fixture.v1",
        provider="fixture",
        model="fixture/model",
        revision="abc123",
        dimension=2,
        normalization="l2",
    )
    search.add_strategy(
        id="python.fixture-search.v1",
        embedding_profile="python.fixture.v1",
        vector_index="fixture-index",
    )
"#,
    )
    .unwrap();

    let plugin = load_plugin(&PythonSearchPluginConfig {
        module: "fixture_plugin".to_owned(),
        register: "register".to_owned(),
        path: Some(directory.path().to_owned()),
    })
    .unwrap();
    assert_eq!(plugin.embeddings.len(), 1);
    assert_eq!(plugin.embedding_strategies.len(), 1);
    assert_eq!(
        plugin.embedding_strategies[0].embedding_profile,
        "python.fixture.v1"
    );

    let output = plugin.embeddings[0]
        .embed(EmbeddingBatch {
            profile: "python.fixture.v1".to_owned(),
            inputs: vec![EmbeddingInput {
                kind: EmbeddingInputKind::Query,
                text: "promoter".to_owned(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(output.vectors, vec![EmbeddingVector::Dense(vec![1.0, 0.0])]);
}

struct StubVectors;

#[async_trait]
impl ScopedVectorSearch for StubVectors {
    async fn query(&self, query: VectorQuery) -> Result<VectorSearchPage, VectorError> {
        assert_eq!(query.index, "fixture-index");
        assert_eq!(query.vector_name, "content");
        assert_eq!(query.vector, VectorValue::Dense(vec![1.0, 0.0]));
        assert_eq!(query.limit, 3);
        assert_eq!(
            query.filter,
            Some(VectorFilter::Any {
                field: "graph".to_owned(),
                values: vec![json!("public")],
            })
        );
        Ok(VectorSearchPage {
            items: vec![VectorSearchHit {
                document_id: DocumentId("part-1".to_owned()),
                score: 0.95,
            }],
            next_cursor: Some("next".to_owned()),
        })
    }
}

struct StubDocuments;

#[async_trait]
impl ScopedDocumentHydrator for StubDocuments {
    async fn hydrate(
        &self,
        document_ids: Vec<DocumentId>,
    ) -> Result<Vec<HydratedDocument>, SearchError> {
        assert_eq!(document_ids, vec![DocumentId("part-1".to_owned())]);
        Ok(vec![HydratedDocument {
            document_id: DocumentId("part-1".to_owned()),
            uri: "https://example.org/part-1".to_owned(),
            graph: Some("public".to_owned()),
            display_id: Some("part_1".to_owned()),
            version: None,
            name: Some("Inducible promoter".to_owned()),
            description: None,
            object_types: vec!["Component".to_owned()],
        }])
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn python_strategy_receives_the_scoped_search_context() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("context_plugin.py"),
        r#"
class Embedding:
    def embed(self, texts, *, kind):
        return [[1.0, 0.0] for _ in texts]

class Strategy:
    async def search(self, ctx, request):
        assert ctx.scope == {"kind": "only", "graphs": ["public"]}
        assert ctx.budget["max_candidates"] == 4
        vector = ctx.embed(request["query"]["text"])[0]
        candidates = ctx.vectors.query(
            vector,
            filter={"op": "any", "field": "graph", "values": ["public"]},
            limit=request["page"]["limit"],
        )
        documents = ctx.documents.hydrate(
            [candidate["document_id"] for candidate in candidates["items"]]
        )
        by_id = {document["document_id"]: document for document in documents}
        items = []
        for candidate in candidates["items"]:
            hit = dict(by_id[candidate["document_id"]])
            hit.update(
                score=candidate["score"],
                score_kind="cosine_similarity",
                evidence=[],
            )
            items.append(hit)
        return {"items": items, "next_cursor": candidates["next_cursor"]}

def register(search):
    search.add_embedding(
        Embedding(), id="python.context.v1", model="fixture", revision="abc", dimension=2
    )
    search.add_strategy(
        Strategy(),
        id="python.context-search.v1",
        embedding_profile="python.context.v1",
        vector_index="fixture-index",
    )
"#,
    )
    .unwrap();

    let mut plugin = load_plugin(&PythonSearchPluginConfig {
        module: "context_plugin".to_owned(),
        register: "register".to_owned(),
        path: Some(directory.path().to_owned()),
    })
    .unwrap();
    assert_eq!(plugin.embedding_strategies.len(), 0);
    assert_eq!(plugin.strategies.len(), 1);
    let embedding = plugin.embeddings.pop().unwrap();
    let strategy = plugin.strategies.pop().unwrap().bind(embedding).unwrap();
    let request: SearchRequest = serde_json::from_value(json!({
        "query": {"kind": "text", "text": "promoter"},
        "filters": {"graphs": ["public"]},
        "page": {"limit": 3},
        "options": {}
    }))
    .unwrap();
    let page = strategy
        .search(
            SearchContext::new(
                SearchScope::Only(vec!["public".to_owned()]),
                SearchBudget {
                    timeout_ms: Some(1_000),
                    max_candidates: 4,
                    max_tool_calls: 0,
                },
            )
            .with_vectors(Arc::new(StubVectors))
            .with_documents(Arc::new(StubDocuments)),
            request,
        )
        .await
        .unwrap();

    assert_eq!(page.strategy.id, "python.context-search.v1");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].score_kind, ScoreKind::CosineSimilarity);
    assert_eq!(page.next_cursor.as_deref(), Some("next"));
}

#[test]
fn rejects_unknown_registration_kwargs() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("bad_plugin.py"),
        r#"
class Embedding:
    def embed(self, texts, *, kind):
        return [[1.0] for _ in texts]

def register(search):
    search.add_embedding(
        Embedding(), id="x", model="x", revision="x", dimension=1, typo=True
    )
"#,
    )
    .unwrap();
    let error = load_plugin(&PythonSearchPluginConfig {
        module: "bad_plugin".to_owned(),
        register: "register".to_owned(),
        path: Some(directory.path().to_owned()),
    })
    .err()
    .unwrap();
    assert!(error.to_string().contains("unexpected keyword argument"));
}
