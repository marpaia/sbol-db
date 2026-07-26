# Pluggable search

The structured search layer adds experimental and deployment-specific search
without changing the classic SynBioHub endpoints or `GET /api/v2/search`.
Those compatibility paths continue to use the existing explorer and ranked
text implementations. New strategies use `POST /api/v2/search`; clients can
discover them with `GET /api/v2/search/strategies`.

The design separates four decisions that are often accidentally coupled:

1. a **strategy** decides how to retrieve, combine, rerank, or reason over
   candidates;
2. an **embedding profile** turns versioned text projections into vectors;
3. a **vector backend** stores and searches named index generations; and
4. an **evaluation suite** determines whether a candidate strategy is fit to
   replace a baseline.

No strategy depends on Qdrant, Postgres, RocksDB, SQLite, FAISS, or an
inference runtime. The application resolves logical names to configured
plugins at startup.

## Crates and trust boundary

| Crate | Responsibility |
| --- | --- |
| `sbol-db-search-sdk` | Stable object-safe contracts and wire-neutral types for strategies, embeddings, candidate stages, scoped services, and vector lifecycle. No storage, model, HTTP, or vector-engine dependencies. |
| `sbol-db-search` | Runtime validation, exact algorithms, scope-preserving vector router, reference embedding strategy, and index-maintenance coordinator. |
| `sbol-db-vector-flat` | Deterministic exact in-memory scan. Development backend and recall oracle for approximate indexes. |
| `sbol-db-vector-qdrant` | Persistent adapter for self-hosted Qdrant and Qdrant Cloud. |
| `sbol-db-search-eval` | Versioned relevance fixtures, ranking metrics, paired comparisons, and rollout gates. |
| `sbol-db-app` | Computes the caller's graph scope, binds scoped vector and primary-store hydration services, and owns compatibility behavior. |

The vector backend returns only document identity and a score. Display
metadata is loaded from the primary `SbolStore` after retrieval. Both the
vector router and hydrator enforce the caller's graph scope; a caller filter
can narrow that scope but cannot widen it.

```text
POST /api/v2/search
        |
        v
 SearchRuntime -- selects and capability-checks --> SearchStrategy
        |                                             |       |
        |                                  query embed|       |hydrate
        |                                             v       v
        +-- ACL-bound SearchContext --> VectorRouter   Primary SbolStore
                                           |
                                  logical index binding
                                  /        |          \
                            exact-flat   Qdrant   future pgvector/FAISS
```

## Writing a strategy

A strategy is one object-safe trait. Its descriptor is an executable contract:
the runtime rejects undeclared input shapes, filters, cursors, and explanation
requests before calling the implementation.

```rust,ignore
use async_trait::async_trait;
use sbol_db_search_sdk::{
    SearchContext, SearchError, SearchPage, SearchRequest, SearchStrategy,
    StrategyDescriptor,
};

pub struct MyStrategy {
    descriptor: StrategyDescriptor,
    // Long-lived, strategy-specific dependencies belong here:
    // lexical candidate source, embedding provider, reranker, tool broker, ...
}

#[async_trait]
impl SearchStrategy for MyStrategy {
    fn descriptor(&self) -> &StrategyDescriptor {
        &self.descriptor
    }

    async fn search(
        &self,
        ctx: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        let budget = ctx.budget();
        let vectors = ctx.vectors()?;       // already ACL scoped
        let documents = ctx.documents()?;  // authoritative and ACL scoped

        // Generate candidates, cap work at budget.max_candidates, hydrate the
        // selected DocumentIds, and return scores with an explicit ScoreKind.
        todo!()
    }
}
```

Classic algorithms can implement `SearchStrategy` directly. Hybrid and neural
strategies can compose the smaller `CandidateSource`, `Fusion`, and `Reranker`
traits. An agentic strategy also implements `SearchStrategy`, but production
agentic execution still needs the planned allow-listed tool broker and durable
trace contract; `SearchBudget::max_tool_calls` is already reserved for its
hard request limit.

Register strategies in an immutable registry and choose an explicit default:

```rust,ignore
let strategies = StrategyRegistry::builder()
    .register(LegacyExplorerStrategy::new(text_index, clusters))?
    .register(my_strategy)?
    .build();
let runtime = Arc::new(SearchRuntime::new(strategies, "legacy.explorer.v1")?);
let app = app.with_search_runtime(runtime);
```

`EmbeddingSearchStrategy` is the reference implementation for text embedding,
vector retrieval, primary-store hydration, score semantics, cursor propagation,
and optional evidence:

```rust,ignore
let semantic = EmbeddingSearchStrategy::new(
    EmbeddingStrategyConfig {
        id: "semantic.components.v1".into(),
        version: "1".into(),
        display_name: "Semantic components".into(),
        description: "Dense retrieval over canonical component text".into(),
        embedding_profile: "local.minilm.v1".into(),
        vector_index: "components".into(),
        vector_name: "content".into(),
        graph_payload_field: "graph".into(),
        distance: DistanceMetric::Cosine,
    },
    embedding_provider,
)?;
```

## Writing an embedding provider

Embedding identity includes provider, model, immutable revision, dimension,
normalization, and data-egress behavior. A rebuild refuses a profile mismatch,
and generated indexes record this provenance in the build report.

```rust,ignore
#[async_trait]
impl EmbeddingProvider for LocalMiniLm {
    fn descriptor(&self) -> &EmbeddingDescriptor {
        &self.descriptor
    }

    async fn embed(
        &self,
        batch: EmbeddingBatch,
    ) -> Result<EmbeddingOutput, SearchError> {
        // Preserve input order and return exactly one vector per input.
        // CPU inference should normally run on a bounded blocking executor.
        self.embed_batch(batch).await
    }
}
```

Recommended provider adapters, rather than dependencies of the SDK itself:

- `fastembed` for a simple local ONNX-backed baseline with curated embedding
  and reranking models;
- Hugging Face Candle when pure-Rust inference, Metal, CUDA, or custom model
  control matters;
- an HTTP provider implementing the same trait for OpenAI-compatible or other
  managed embedding APIs, with `data_egress = configured_remote`;
- a deterministic fixture provider for tests and evaluation.

Model download/cache policy, license metadata, text projection revision, and
query/document prefixes are part of profile configuration and provenance, not
implicit behavior inside a search strategy.

## Vector backend selection

The logical index router lets deployment topology choose the engine:

| sbol-db role | Initial recommendation | Alternatives to evaluate |
| --- | --- | --- |
| Library, tests, small corpus | `sbol-db-vector-flat` | Qdrant Edge once stable-toolchain-compatible |
| Embedded SQLite or RocksDB application | exact-flat for small data; Qdrant server when ANN scale is needed | Qdrant Edge, LanceDB, USearch/FAISS adapter |
| Postgres service | Qdrant self-hosted or Cloud | pgvector to reduce operational components |
| Search service in a larger stack | Qdrant self-hosted or Cloud | deployment's existing vector service |

Exact-flat is deliberately not presented as an ANN performance backend. It is
the correctness oracle: evaluate an approximate backend's recall against its
exact ranking on the same vectors and filters.

Qdrant Edge remains a desired embedded adapter. The current `qdrant-edge`
0.7.2 crate was compile-tested against this workspace's stable Rust 1.93 toolchain
and currently reaches unstable Rust APIs. The project will not set
`RUSTC_BOOTSTRAP` or silently require nightly to ship it. Keep the adapter
boundary, add Edge when it builds on supported stable Rust, and retain LanceDB
as a mature embedded alternative to benchmark. See the
[Qdrant Edge quickstart](https://qdrant.tech/documentation/edge/edge-quickstart/)
and [LanceDB embedded quickstart](https://docs.lancedb.com/quickstart).

## Generation lifecycle

Indexes are build artifacts, not mutable application truth. Every backend
implements the same lifecycle:

1. `create_generation` creates an inactive generation with dimension,
   distance, named-vector, and backend parameters;
2. `apply` writes validated batches while the previous generation serves
   queries;
3. `flush`, `optimize`, and optional `snapshot` establish the durability
   boundary;
4. `activate` atomically swaps the logical artifact to the new generation;
5. `generations` reconciles state after restarts or partial failures; and
6. `delete_generation` removes only inactive generations.

`VectorIndexMaintainer::rebuild` coordinates embedding batches and this state
machine. A failed build is cleaned up best-effort and never activates. Qdrant
stores each generation in a physical collection, persists the full generation
spec in collection metadata, and changes the logical artifact using one atomic
multi-action alias request. Rollback is another alias activation, not a
re-embedding job.

The next maintenance slice should connect import/update/delete projection
events to durable jobs, add idempotency keys and retry policy, and persist
`IndexBuildReport` plus snapshot references in an artifact catalog.

## Evaluation and rollout

`sbol-db-search-eval` runs any strategy against a versioned
`EvaluationSuite`. Each case carries a normal `SearchRequest` and graded
document judgments. Reports include precision, recall, reciprocal rank, and
nDCG at configured cutoffs plus per-case latency.

Promotion is a paired comparison against the same suite revision. A
`QualityGate` requires both a minimum mean nDCG delta and a maximum fraction of
queries that regress beyond tolerance. This prevents a large gain on a few
queries from concealing broad regressions.

A production rollout should add, in order:

1. checked-in synthetic and curated SBOL relevance suites with provenance;
2. exact-flat versus ANN recall/latency benchmarks on identical vectors;
3. shadow execution that records candidate reports without affecting users;
4. deterministic traffic splitting by strategy version;
5. online success metrics and operational budgets; and
6. explicit promotion or rollback through configuration, never automatic
   replacement from a single aggregate score.

## Implementation status and next slices

Implemented:

- SDK contracts and immutable registries;
- legacy explorer adapter and additive structured HTTP API;
- exact-flat backend and Qdrant self-hosted/cloud adapter;
- ACL-bound logical vector router and authoritative hydration;
- dense reference embedding strategy;
- generation rebuild/activation/rollback coordinator; and
- offline relevance metrics and paired quality gate.

Next:

1. implement a local `fastembed` provider plus deterministic model-cache and
   revision policy;
2. project canonical SBOL search documents and enqueue durable incremental or
   full rebuild jobs;
3. expose backend/profile/strategy assembly through typed server
   configuration;
4. run live Qdrant lifecycle integration tests in CI;
5. benchmark Qdrant, pgvector, Qdrant Edge (when stable), and LanceDB against
   exact-flat; and
6. add hybrid fusion/reranking, then the separately bounded agentic tool
   broker and trace model.
