# Build a search idea on sbol-db

This guide is for someone who has a search idea and wants to test it without
forking sbol-db's compatibility behavior or committing to one model, vector
engine, or deployment topology.

The short version is:

1. implement the smallest Rust trait that represents the new idea;
2. declare exactly what the implementation can do;
3. register it under a stable, versioned identity;
4. run it through the structured search API; and
5. compare it with the current strategy on the same versioned relevance set.

The compatibility routes remain unchanged. Experimental strategies are
additive and are selected explicitly through `POST /api/v2/search`.

## The system in one picture

```text
                                  SearchStrategy
                                        |
                  +---------------------+---------------------+
                  |                     |                     |
            CandidateSource          Fusion               Reranker
          lexical / graph /     RRF / weighted /      neural / rules /
          vector / sequence       learned blend          cross-encoder
                  |
          EmbeddingProvider ---- logical vector index ---- VectorBackend
                                        |
                             exact-flat / FAISS / Qdrant

 request --> capability check --> ACL-scoped execution --> primary-store hydration
                                                        --> SearchPage + Evidence

 candidate strategy + baseline strategy --> versioned EvaluationSuite --> QualityGate
```

There are two deliberately separate planes:

- the **query plane** selects a `SearchStrategy` and executes it inside the
  caller's authorization scope;
- the **maintenance plane** embeds the canonical corpus projection and builds
  an immutable vector-index generation before atomically activating it.

sbol-db remains the database and source of authoritative SBOL metadata. FAISS,
Qdrant, or another vector engine is an index selected by deployment.

## Choose the smallest extension point

| Your idea | Implement | Use it when |
| --- | --- | --- |
| A complete search behavior | `SearchStrategy` | The idea owns request interpretation, retrieval, reasoning, or final ranking. |
| A new candidate generator | `CandidateSource` | The idea produces document IDs and scores but should compose with existing fusion or reranking. |
| A rank-combination algorithm | `Fusion` | The idea combines lexical, vector, graph, or sequence rankings deterministically. |
| A neural or algorithmic second stage | `Reranker` | The idea reranks a bounded candidate set. |
| A local or remote embedding model | `EmbeddingProvider` | The idea changes the vector representation, prefixes, normalization, or inference provider. |
| A vector engine or index format | `VectorBackend` | The idea changes vector retrieval and generation storage. |
| A new relevance benchmark | `EvaluationSuite` | The idea changes how improvement is measured rather than how results are generated. |

Most ideas should not start with `VectorBackend`. A ranking formula, graph
walk, sequence heuristic, learned reranker, or bounded agent should normally
be a strategy or composable stage and stay independent of FAISS or Qdrant.

## Contracts every strategy inherits

The SDK is intentionally small and infrastructure-free, but its contracts are
strict.

### Compatibility is preserved

`GET /api/v2/search` and the classic SynBioHub routes keep their existing
behavior. A new implementation appears under a new strategy ID on
`POST /api/v2/search`. It becomes the default only after an explicit topology
change.

### Authorization is a ceiling

`SearchContext::scope()` is computed by the application. It never comes from
the request body. `ctx.vectors()` and `ctx.documents()` are already scoped to
that ceiling, so prefer them over direct backend access.

A caller's graph filter may narrow the authorized scope. It must never widen
it. If a custom candidate source reads another index or store directly, that
adapter must accept `SearchScope` and enforce it before returning candidates.

### Vector payload is not authoritative metadata

A vector backend returns `DocumentId` plus a score. Resolve display IDs,
names, descriptions, types, and graph ownership through
`ScopedDocumentHydrator` before returning a result. This prevents stale index
payload from becoming application truth.

### Capabilities are executable promises

`StrategyDescriptor` is not marketing metadata. Before the strategy runs, the
runtime rejects input kinds, filters, cursors, and explanation requests that
the descriptor does not declare.

Declare post-filtering as `PostFilter`, not `Native`. Return `Total::Unknown`
when the implementation cannot establish a count. Set `data_egress` to
`ConfiguredRemote` whenever query or corpus content can leave the process.

### Scores need semantics

Every hit carries a `ScoreKind`. Do not compare cosine similarity, negative
distance, model logits, and lexical scores as if they share a scale. Normalize
or fuse them explicitly and identify the resulting semantics. When
`explain=true`, attach stage-level `Evidence` rather than an unsupported causal
story.

### Work is bounded

Respect `SearchBudget::max_candidates`, `timeout_ms`, and, for bounded agentic
execution, `max_tool_calls`. A timeout or cancellation should stop downstream
model and tool work. Never let a reranker silently expand an unbounded corpus
scan.

### Identity is reproducible

Strategy IDs are stable names; versions identify behavior. Embedding profiles
also name the provider, model, immutable revision, dimension, normalization,
and egress policy. An experiment report is useful only when those identities
and the active index generation can be reconstructed.

## Start a strategy crate

Inside this repository, use workspace dependencies:

```toml
[dependencies]
async-trait.workspace = true
sbol-db-search-sdk.workspace = true
```

Outside the repository, pin a released version when available. Until then,
pin an exact Git revision rather than tracking a moving branch:

```toml
[dependencies]
async-trait = "0.1"
sbol-db-search-sdk = { git = "https://github.com/marpaia/sbol-db", rev = "<tested-commit>" }
```

The SDK deliberately does not pull in SQLx, Tantivy, FAISS, Qdrant, ONNX,
HTTP, or an agent framework. Add only the dependencies your implementation
actually needs.

## Implement a complete strategy

This hybrid skeleton shows the normal shape: generate bounded candidates,
combine them, optionally rerank, hydrate authoritative documents, and return
explicit evidence. The concrete candidate sources can be lexical, graph,
sequence, embedding, or experimental algorithms.

```rust,ignore
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_search_sdk::{
    CandidateRequest, CandidateSource, DataEgress, Evidence,
    ExecutionMetadata, FilterCapability, FilterKind, Fusion,
    PaginationCapability, RerankRequest, Reranker, ScoreKind, SearchContext,
    SearchError, SearchHit, SearchInput, SearchInputKind, SearchPage,
    SearchRequest, SearchStrategy, StrategyCapabilities, StrategyDescriptor,
    StrategyRef, StrategyRequirements, Total, TotalCapability,
};

pub struct HybridStrategy {
    descriptor: StrategyDescriptor,
    sources: Vec<Arc<dyn CandidateSource>>,
    fusion: Arc<dyn Fusion>,
    reranker: Option<Arc<dyn Reranker>>,
}

impl HybridStrategy {
    pub fn new(
        sources: Vec<Arc<dyn CandidateSource>>,
        fusion: Arc<dyn Fusion>,
        reranker: Option<Arc<dyn Reranker>>,
    ) -> Self {
        Self {
            descriptor: StrategyDescriptor {
                id: "example.hybrid.v1".into(),
                version: "1".into(),
                display_name: "Example hybrid".into(),
                description: "Fused candidate retrieval with an optional reranker".into(),
                capabilities: StrategyCapabilities {
                    inputs: vec![SearchInputKind::Text],
                    filters: vec![FilterKind::Graph, FilterKind::ObjectType],
                    filter_execution: FilterCapability::PostFilter,
                    pagination: PaginationCapability::FirstPageOnly,
                    totals: TotalCapability::Unknown,
                    deterministic: false,
                    explanations: true,
                    data_egress: DataEgress::None,
                },
                requirements: StrategyRequirements {
                    embedding_profiles: vec![],
                    vector_indexes: vec![],
                    candidate_sources: sources
                        .iter()
                        .map(|source| source.id().to_owned())
                        .collect(),
                },
            },
            sources,
            fusion,
            reranker,
        }
    }
}

#[async_trait]
impl SearchStrategy for HybridStrategy {
    fn descriptor(&self) -> &StrategyDescriptor {
        &self.descriptor
    }

    async fn search(
        &self,
        ctx: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        let SearchInput::Text { text } = &request.query else {
            return Err(SearchError::Unsupported("text input required".into()));
        };
        if text.trim().is_empty() {
            return Err(SearchError::InvalidRequest("query is empty".into()));
        }

        let candidate_limit = ctx
            .budget()
            .max_candidates
            .min(request.page.limit.saturating_mul(20));
        let mut inputs = Vec::with_capacity(self.sources.len());
        for source in &self.sources {
            inputs.push(
                source
                    .candidates(
                        ctx.clone(),
                        CandidateRequest {
                            search: request.clone(),
                            limit: candidate_limit,
                        },
                    )
                    .await?,
            );
        }

        let mut ranked = self.fusion.fuse(&inputs)?;
        if let Some(reranker) = &self.reranker {
            ranked = reranker
                .rerank(RerankRequest {
                    query: text.clone(),
                    candidates: ranked,
                })
                .await?;
        }
        ranked.items.truncate(request.page.limit);
        let score_kind = if self.reranker.is_some() {
            ScoreKind::Reranker
        } else {
            ScoreKind::Custom(format!("fusion:{}", self.fusion.id()))
        };

        let ids = ranked
            .items
            .iter()
            .map(|candidate| candidate.document_id.clone())
            .collect::<Vec<_>>();
        let hydrated = ctx.documents()?.hydrate(ids).await?;
        let documents = hydrated
            .into_iter()
            .map(|document| (document.document_id.clone(), document))
            .collect::<HashMap<_, _>>();

        let mut warnings = Vec::new();
        let mut items = Vec::new();
        for (rank, candidate) in ranked.items.into_iter().enumerate() {
            let Some(document) = documents.get(&candidate.document_id) else {
                warnings.push(format!(
                    "document {:?} was absent after scoped hydration",
                    candidate.document_id.0
                ));
                continue;
            };
            if !request.filters.graphs.is_empty()
                && !document
                    .graph
                    .as_ref()
                    .is_some_and(|graph| request.filters.graphs.contains(graph))
            {
                continue;
            }
            if !request.filters.object_types.is_empty()
                && !document
                    .object_types
                    .iter()
                    .any(|kind| request.filters.object_types.contains(kind))
            {
                continue;
            }
            items.push(SearchHit {
                document_id: document.document_id.clone(),
                uri: document.uri.clone(),
                graph: document.graph.clone(),
                score: candidate.score,
                score_kind: score_kind.clone(),
                display_id: document.display_id.clone(),
                version: document.version.clone(),
                name: document.name.clone(),
                description: document.description.clone(),
                object_types: document.object_types.clone(),
                evidence: request.options.explain.then(|| Evidence {
                    source: candidate.source,
                    rank: Some(rank + 1),
                    score: Some(candidate.score),
                    details: BTreeMap::new(),
                }).into_iter().collect(),
            });
        }

        Ok(SearchPage {
            strategy: StrategyRef {
                id: self.descriptor.id.clone(),
                version: self.descriptor.version.clone(),
            },
            items,
            total: Total::Unknown,
            next_cursor: None,
            execution: ExecutionMetadata {
                warnings,
                ..ExecutionMetadata::default()
            },
        })
    }
}
```

The snippet uses first-page-only semantics honestly. Add a cursor only when the
entire composed pipeline can reproduce a stable continuation. If the reranker
calls a remote service, change `data_egress` and version the strategy whenever
the prompt, model, normalization, fusion, or reranking behavior changes.
This example declares post-filtering because it narrows hydrated results after
candidate generation. Declare native filtering only when every candidate path
executes every advertised filter before ranking. Candidate-source requirements
are visible in discovery metadata; today the strategy constructor owns those
objects while the deployment builder validates embedding-profile and logical
vector-index requirements.

## Implement an embedding provider

An embedding provider returns exactly one vector per input in the original
order. Query and document input kinds let a model apply different prefixes.

```rust,ignore
use async_trait::async_trait;
use sbol_db_search_sdk::{
    DataEgress, EmbeddingBatch, EmbeddingDescriptor, EmbeddingOutput,
    EmbeddingProvider, EmbeddingVector, Normalization, SearchError,
};

pub struct MyEmbeddingProvider {
    descriptor: EmbeddingDescriptor,
    model: MyLoadedModel,
}

impl MyEmbeddingProvider {
    pub fn new(model: MyLoadedModel) -> Self {
        Self {
            descriptor: EmbeddingDescriptor {
                id: "local.my-model.rev1".into(),
                provider: "my-runtime".into(),
                model: "org/my-model".into(),
                revision: "sha3-256:<weights-and-config-digest>".into(),
                dimension: 384,
                normalization: Normalization::L2,
                data_egress: DataEgress::None,
            },
            model,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for MyEmbeddingProvider {
    fn descriptor(&self) -> &EmbeddingDescriptor {
        &self.descriptor
    }

    async fn embed(
        &self,
        batch: EmbeddingBatch,
    ) -> Result<EmbeddingOutput, SearchError> {
        if batch.profile != self.descriptor.id {
            return Err(SearchError::Configuration("embedding profile drift".into()));
        }
        let input_count = batch.inputs.len();
        let vectors = self.model.embed(batch.inputs).await?;
        if vectors.len() != input_count {
            return Err(SearchError::Backend("embedding cardinality mismatch".into()));
        }
        if vectors.iter().any(|v| {
            v.len() != self.descriptor.dimension || v.iter().any(|x| !x.is_finite())
        }) {
            return Err(SearchError::Backend("invalid embedding output".into()));
        }
        Ok(EmbeddingOutput {
            vectors: vectors.into_iter().map(EmbeddingVector::Dense).collect(),
        })
    }
}
```

In production, load and verify model artifacts at startup. Do not download
weights in `embed()`. CPU inference should use a bounded blocking executor;
remote inference should have explicit timeouts, retry bounds, and
`DataEgress::ConfiguredRemote`.

The repository's `sbol-db-embedding-fastembed` crate is a production-oriented
local example with content-derived bundle revisions and query/document
prefixes.

## Use the reference embedding strategy

If the idea is “embed text, query a logical vector index, hydrate results,” no
new strategy code is necessary:

```rust,ignore
let semantic = EmbeddingSearchStrategy::new(
    EmbeddingStrategyConfig {
        id: "semantic.components.v1".into(),
        version: "1".into(),
        display_name: "Semantic components".into(),
        description: "Dense retrieval over canonical SBOL text".into(),
        embedding_profile: "local.my-model.rev1".into(),
        vector_index: "components".into(),
        vector_name: "content".into(),
        graph_payload_field: "graph".into(),
        distance: DistanceMetric::Cosine,
    },
    embedding_provider,
)?;
```

This implementation already handles query embedding, scoped vector retrieval,
authoritative hydration, score semantics, cursor propagation, and evidence.

## Implement a vector backend only when the engine is the idea

A backend implements both query and administration:

```rust,ignore
impl VectorSearcher for MyBackend {
    fn descriptor(&self) -> &VectorBackendDescriptor { /* ... */ }
    async fn query(&self, query: VectorQuery) -> Result<VectorSearchPage, VectorError> {
        /* execute the portable filter natively, then return IDs and scores */
    }
}

impl VectorIndexAdmin for MyBackend {
    async fn create_generation(&self, spec: IndexGenerationSpec) -> Result<GenerationHandle, VectorError> { /* ... */ }
    async fn apply(&self, generation: &GenerationHandle, changes: Vec<VectorChange>) -> Result<ApplyReceipt, VectorError> { /* ... */ }
    async fn flush(&self, generation: &GenerationHandle) -> Result<(), VectorError> { /* ... */ }
    async fn optimize(&self, generation: &GenerationHandle) -> Result<(), VectorError> { /* ... */ }
    async fn snapshot(&self, generation: &GenerationHandle) -> Result<SnapshotRef, VectorError> { /* ... */ }
    async fn activate(&self, generation: &GenerationHandle) -> Result<(), VectorError> { /* atomic swap */ }
    async fn generations(&self, artifact_id: &str) -> Result<Vec<GenerationStatus>, VectorError> { /* ... */ }
    async fn delete_generation(&self, generation: &GenerationHandle) -> Result<(), VectorError> { /* inactive only */ }
}
```

The important behavior is the state machine, not the method signatures:

```text
absent --> inactive/building --> flushed --> optimized --> active
                         |                                  |
                         +-- failure: delete                +-- rollback: reactivate prior
```

A backend must:

- reject unsupported vector kinds, distances, parameters, and filters;
- never silently turn a declared native filter into post-filtering;
- validate vector dimensions and finite values before mutation;
- keep the prior active generation queryable during a rebuild;
- make activation atomic at the logical artifact boundary;
- reject deletion of the active generation;
- report capabilities conservatively; and
- preserve enough generation metadata to reconcile after restart.

Use `sbol-db-vector-flat` as the exact-recall oracle,
`sbol-db-search-faiss` as the embedded persistent ANN example, and
`sbol-db-vector-qdrant` as the service-backed example.

## Assemble and validate a deployment

The typed builder resolves plugin IDs once at startup. It rejects missing
profiles or backends, duplicate logical indexes, profile/vector-name/distance
drift, and backends that cannot enforce graph filtering natively. Backends
validate vector dimensions when generations are created and queried.

```rust,ignore
let deployment = SearchDeploymentBuilder::new(topology)
    .register_embedding(Arc::new(my_embedding))?
    .register_vector_backend(Arc::new(faiss_or_qdrant))?
    .register_strategy(Arc::new(my_hybrid_strategy))?
    .build()?;

let app = AppServices::from_backend(&backend)
    .with_search_deployment(&deployment);
let worker = Worker::new(/* storage and job configuration */)
    .with_vector_indexes(deployment.maintainers());
```

For the shipped binary, `SBOL_DB_SEARCH_CONFIG` is the composition root. Query
servers build both planes; a standalone worker can call `build_maintenance()`
without loading query-only strategy dependencies.

Discover the resulting contract:

```sh
curl http://localhost:8080/api/v2/search/strategies
```

Call one strategy explicitly:

```sh
curl -X POST http://localhost:8080/api/v2/search \
  -H 'content-type: application/json' \
  -d '{
    "strategy": "example.hybrid.v1",
    "query": {"kind": "text", "text": "inducible promoter"},
    "filters": {
      "object_types": ["http://sbols.org/v3#Component"]
    },
    "page": {"limit": 20},
    "options": {"explain": true, "timeout_ms": 2000}
  }'
```

## Maintain vector indexes

`VectorIndexMaintainer` coordinates an embedding provider with any
`VectorBackend`. A full rebuild:

1. validates profile, dimension, distance, and unique document identity;
2. creates a new inactive generation;
3. embeds canonical document projections in stable batches;
4. applies vectors and filter payload;
5. flushes and optimizes;
6. atomically activates the completed generation; and
7. returns an `IndexBuildReport` containing model and generation provenance.

The previous generation remains active until step 6. A failed generation is
removed best-effort and never serves queries.

```rust,ignore
let report = deployment
    .maintainers()
    .get("components")
    .expect("validated logical index")
    .rebuild(
        VectorRebuildSpec {
            artifact_id: "components".into(),
            generation: "corpus-42-model-rev1".into(),
            vector_name: "content".into(),
            embedding_profile: "local.my-model.rev1".into(),
            distance: DistanceMetric::Cosine,
            batch_size: 64,
            backend_parameters: BTreeMap::new(),
        },
        canonical_documents,
    )
    .await?;
```

The shipped `rebuild_vector_index` durable job invokes this path from a
keyset-paged projection of the primary store. Use a generation name and
idempotency key derived from corpus revision, projection revision, embedding
revision, and backend parameters.

For one embedded sbol-db process, FAISS keeps the index local to SQLite or
RocksDB deployments. For independently scaled query and worker processes, use
Qdrant or another service backend. One local FAISS store has one owning
sbol-db process.

## Recipes for different kinds of ideas

### Old-school algorithm

Implement `CandidateSource` or `SearchStrategy`, inject the required read-only
index, and set `deterministic=true` when the same versioned inputs guarantee
the same ordering. Examples include BM25 variants, graph centrality, edit
distance, locality-sensitive hashing, rule-based rankers, and domain-specific
sequence algorithms.

Use the exact-flat backend only if the algorithm actually consumes vectors;
ordinary algorithms do not need a vector dependency.

### Embedding retrieval

Implement `EmbeddingProvider` and use `EmbeddingSearchStrategy`. Compare ANN
recall with exact-flat on the same vectors before tuning latency-oriented
backend parameters.

### Neural reranking

Implement `Reranker`. Retrieve a bounded pool with one or more candidate
sources, rerank only that pool, and record the model revision in evidence or
execution metadata. Evaluation should measure both relevance and tail latency.

### Hybrid retrieval

Implement candidate sources for the independent signals, a `Fusion` policy,
and optionally a reranker. Reciprocal-rank fusion is a strong first baseline
because it does not pretend heterogeneous raw scores share a scale.

### Agentic search

Implement the top-level `SearchStrategy` and treat every retrieval or reasoning
step as a bounded tool call. The SDK already reserves `max_tool_calls`, but the
repository does not yet ship a production allow-listed tool broker, durable
agent trace, or generic prompt/model registry. A production integration must
provide those pieces, enforce authorization at every tool boundary, and
declare remote egress.

An agent should return search evidence, not unverifiable prose. Its final
document IDs must still be hydrated through the scoped primary-store facade.

## Prove the idea improves search

`sbol-db-search-eval` evaluates any `SearchStrategy` with normal structured
requests and graded relevance judgments. It reports precision, recall,
reciprocal rank, and nDCG at configured cutoffs, plus per-case and mean
latency.

Keep suites in version control. A useful case records why each judgment exists
and which corpus revision it targets. Include easy, ambiguous, no-result,
authorization-sensitive, and adversarial queries rather than only examples
the candidate model already handles well.

```rust,ignore
let config = EvaluationConfig {
    cutoffs: vec![1, 5, 10, 20],
};

let baseline_report = evaluate_strategy(
    baseline.as_ref(),
    evaluation_context.clone(),
    &suite,
    &config,
).await?;

let candidate_report = evaluate_strategy(
    candidate.as_ref(),
    evaluation_context,
    &suite,
    &config,
).await?;

let comparison = compare(
    &baseline_report,
    &candidate_report,
    &QualityGate {
        cutoff: 10,
        min_mean_ndcg_delta: 0.02,
        regression_tolerance: 0.01,
        max_regressed_fraction: 0.10,
    },
)?;

if !comparison.accepted {
    return Err("candidate search strategy failed its rollout gate".into());
}
```

The gate is paired: baseline and candidate must use the same suite ID,
revision, and case set. Acceptance requires both the configured mean nDCG gain
and the regression-fraction bound. Add separate operational gates for p95/p99
latency, error rate, memory, index size, build time, model cost, and ANN recall
where those matter.

A sensible rollout sequence is:

1. deterministic unit tests for the new trait implementation;
2. contract tests for malformed inputs, capability mismatches, filters, and
   authorization narrowing;
3. offline paired evaluation against the current default;
4. exact-vs-ANN differential tests when approximate retrieval is involved;
5. shadow execution with persisted strategy and generation identities;
6. opt-in canary traffic; and
7. an explicit default-strategy change only after the quality and operational
   gates pass.

## Definition of done

A strategy is ready to share when another person can answer all of these from
its code and fixtures:

- What stable strategy ID and version identify this behavior?
- Which inputs, filters, pagination, totals, explanations, and egress does it
  honestly support?
- Where is authorization enforced for every candidate source and tool?
- What bounds candidates, inference, tool calls, and wall-clock time?
- What does the score mean, and how are heterogeneous scores fused?
- Which model, prompt, corpus projection, and index generation produced the
  result?
- Can a failed index build leave the current generation untouched?
- Does the implementation pass deterministic contract tests?
- On which versioned relevance suite does it beat the baseline?
- How many individual queries regress, and what are the operational costs?

## Working implementations to read

- [`sbol-db-search-sdk`](../crates/sbol-db-search-sdk/src/lib.rs): public
  infrastructure-free contracts.
- [`EmbeddingSearchStrategy`](../crates/sbol-db-search/src/embedding_strategy.rs):
  end-to-end semantic retrieval.
- [`LegacyExplorerStrategy`](../crates/sbol-db-app/src/search.rs): existing
  algorithm wrapped as a structured strategy.
- [`VectorIndexMaintainer`](../crates/sbol-db-search/src/maintenance.rs):
  backend-neutral immutable generation construction.
- [`sbol-db-vector-flat`](../crates/sbol-db-vector-flat/src/lib.rs): exact
  backend and recall oracle.
- [`sbol-db-search-faiss`](../crates/sbol-db-search-faiss/src/lib.rs): embedded
  persistent ANN backend.
- [`sbol-db-vector-qdrant`](../crates/sbol-db-vector-qdrant/src/lib.rs): remote
  vector-service adapter.
- [`sbol-db-search-eval`](../crates/sbol-db-search-eval/src/lib.rs): metrics and
  paired rollout gates.
- [Pluggable search reference](search-plugins.md): configuration, backend
  selection, and operational details.
