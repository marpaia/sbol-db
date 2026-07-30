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
| `sbol-db-embedding-fastembed` | Local FastEmbed/ONNX provider with immutable profile identity, query/document prefixes, bounded blocking execution, and normalization validation. |
| `sbol-db-vector-flat` | Deterministic exact in-memory scan. Development backend and recall oracle for approximate indexes. |
| `sbol-db-vector-qdrant` | Persistent adapter for self-hosted Qdrant and Qdrant Cloud. |
| `sbol-db-search-faiss` | Persistent embedded FAISS backend with sbol-db-owned payload indexes, checksummed generations, snapshots, and atomic activation. |
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
                            exact-flat   FAISS   Qdrant / future pgvector
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
trace contract. The runtime enforces `SearchBudget::timeout_ms` around the
complete strategy future; `max_tool_calls` is reserved for the strategy's hard
tool limit.

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

For a complete deployment, one typed topology drives query routing and index
maintenance. Concrete crates construct the plugin objects; the shared builder
resolves their stable IDs and rejects missing providers, duplicate logical
indexes, vector-name/profile drift, unsupported distances, non-dense backends,
or backends that cannot enforce graph filters natively:

```rust,ignore
let topology: SearchTopologyConfig = serde_json::from_slice(config_bytes)?;
let deployment = SearchDeploymentBuilder::new(topology)
    .register_embedding(fastembed_provider)?
    .register_vector_backend(qdrant_backend)?
    .register_strategy(Arc::new(LegacyExplorerStrategy::new(
        text_index,
        clusters,
    )))?
    .build()?;

let app = AppServices::from_backend(&backend)
    .with_search_deployment(&deployment);
let worker = Worker::new(/* existing storage/job arguments */)
    .with_vector_indexes(deployment.maintainers());
```

`VectorIndexBindingConfig` names the logical index, backend instance,
embedding profile, named vector, and graph payload field. This keeps the Rust
SDK usable in an embedded application without forcing that application to use
sbol-db's CLI or a particular configuration-file format.

Without `--search-config`, `sbol-db server` installs a built-in vector index:
`builtin.sbol-text-bge-small.v1`. It embeds the canonical, labeled SBOL object
projection (URI, SBOL class, display ID, name, description, types, and roles)
with a checksum-pinned 384-dimensional BGE-small ONNX bundle and cosine
similarity. The model is local and has no request-time egress; the production
image carries its verified weights. Source builds set
`SBOL_DB_BGE_SMALL_MODEL_DIR` after `make model/bge-small`. There is no silent
lexical fallback, because the same strategy name must always denote one vector
space. It is enabled with the normal embedded worker, queues a full rebuild at
every server startup, and becomes the default structured-search strategy as
`builtin.sbol-text-vector.v2`. See [the model release contract](builtin-bge-small-model.md)
for the upstream pin, per-file checksums, and relevance gate.

The built-in backend is exact-flat and intentionally lives in the API/worker
process, so `--no-worker` disables this default index. Use a configured shared
backend (such as Qdrant) when API and worker processes are separate or when
multiple replicas must serve the same vector state. The compatibility search
routes retain their existing ranked-text behavior.

`--search-config` / `SBOL_DB_SEARCH_CONFIG` replaces this baseline with an
explicit JSON composition root. `server` installs both query and maintenance
planes; `worker` installs only maintenance. A Qdrant deployment with a verified
local FastEmbed model looks like:

```json
{
  "topology": {
    "default_strategy": "legacy.explorer.v1",
    "indexes": [{
      "index": "components",
      "backend": "qdrant-primary",
      "embedding_profile": "local.bge-small-en-v1.5.rev1",
      "vector_name": "content",
      "graph_payload_field": "graph",
      "maintenance": {
        "generation_prefix": "components-auto",
        "distance": "cosine",
        "batch_size": 64,
        "backend_parameters": { "on_disk": true }
      }
    }],
    "embedding_strategies": [{
      "id": "semantic.components.v1",
      "version": "1",
      "display_name": "Semantic components",
      "description": "Dense retrieval over canonical SBOL metadata",
      "embedding_profile": "local.bge-small-en-v1.5.rev1",
      "vector_index": "components",
      "vector_name": "content",
      "graph_payload_field": "graph",
      "distance": "cosine"
    }]
  },
  "embeddings": [{
    "kind": "fastembed_local",
    "profile": {
      "id": "local.bge-small-en-v1.5.rev1",
      "model": "Qdrant/bge-small-en-v1.5-onnx-Q@52398278842ec682c6f32300af41344b1c0b0bb2",
      "revision": "sha3-256:bf577972c34b37578aa42965fa8401d5538f4b4c007c810332e936548658c7b3",
      "dimension": 384,
      "normalization": "l2",
      "batch_size": 32
    },
    "bundle": {
      "directory": "/opt/sbol-db/models/bge-small-en-v1.5",
      "onnx_file": "model_optimized.onnx",
      "pooling": "cls",
      "max_length": 512,
      "intra_threads": 2
    }
  }],
  "vector_backends": [{
    "kind": "qdrant",
    "config": {
      "id": "qdrant-primary",
      "grpc_url": "https://cluster.example:6334",
      "rest_url": "https://cluster.example:6333",
      "collection_prefix": "sbol",
      "timeout_seconds": 30
    },
    "api_key_env": "QDRANT_API_KEY"
  }]
}
```

For a small in-process deployment, replace the vector backend entry with
`{"kind":"exact_flat","id":"flat"}` and reference `flat` from the index.
Exact-flat state belongs to that process and is not suitable for a separate
maintenance-worker/API topology.

For an embedded production deployment, source builds enable the backend with
`--features faiss`. The published `ghcr.io/marpaia/sbol-db` container already
includes that feature and the native FAISS runtime. Configure a persistent
local store:

```json
{
  "kind": "faiss",
  "config": {
    "id": "faiss-local",
    "path": "/var/lib/sbol-db/search/faiss",
    "default_nlist": 256,
    "default_nprobe": 16,
    "flat_search_cutoff": 256,
    "max_query_k": 10000
  }
}
```

In the container, place `path` under the pre-owned `/var/lib/sbol-db`
directory and mount that directory as a named volume or persistent volume.
See [Running embedded FAISS](deployment.md#running-embedded-faiss) for a
complete invocation. One running process exclusively owns a local FAISS store;
use the embedded worker for maintenance and a service backend such as Qdrant
when query or worker processes must scale independently.

`sbol-db-search-faiss` generates target-native Rust bindings from the FAISS
1.14 C headers at build time. On macOS, `brew install faiss` provides the
development and runtime libraries. Linux source builds need Clang/libclang and
should install or build FAISS 1.14 with `FAISS_ENABLE_C_API=ON` and
`BUILD_SHARED_LIBS=ON`, then expose it through `FAISS_DIR` or the platform
library search path. The feature is opt-in so a deployment that selects
Qdrant, pgvector, or exact-flat does not acquire a native FAISS dependency
accidentally.

FAISS generation parameters support `nlist`, `nprobe`, and
`flat_search_cutoff`. Query parameters support `nprobe` and `max_codes`.
Unknown parameters are rejected. Small generations use exact `IDMap2,Flat`;
larger generations use `IndexIVFFlat`, with training-centroid limits validated
before FAISS is called.

## Writing an embedding provider

Embedding identity includes provider, model, immutable revision, dimension,
normalization, and data-egress behavior. A rebuild refuses a profile mismatch,
and generated indexes record the vector-space identity in both the generation
spec and build report. Incremental maintenance refuses generations without
that provenance, so an older generation must be rebuilt once before accepting
newly embedded documents.

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

### Python search plugins

Build the `sbol-db` binary with the opt-in `python` feature to load ordinary
Python modules into the same composition root. A Python strategy receives the
same authorization-scoped services as a Rust strategy:

```python
from sbol_db.search import SearchContext, SearchPlugin

class MyEmbedding:
    def embed(self, texts, *, kind):
        return model.encode(texts).tolist()

class MyStrategy:
    def search(self, ctx: SearchContext, request):
        vector = ctx.embed(request["query"]["text"])[0]
        candidates = ctx.vectors.query(
            vector,
            limit=request["page"]["limit"],
            cursor=request["page"].get("cursor"),
        )
        documents = ctx.documents.hydrate(
            [item["document_id"] for item in candidates["items"]]
        )
        # Join the authorized documents to the candidates and return SearchHit
        # mappings. See the BGE example for the complete response construction.
        return make_page(candidates, documents)

def register(search: SearchPlugin):
    search.add_embedding(
        MyEmbedding(),
        id="python.my-model.v1",
        model="organization/model-name",
        revision="immutable-model-revision",
        dimension=384,
        normalization="l2",
    )
    search.add_strategy(
        MyStrategy(),
        id="python.my-model-search.v1",
        embedding_profile="python.my-model.v1",
        vector_index="my-model-index",
    )
```

`ctx.scope` and `ctx.budget` expose the effective request constraints;
`ctx.embed(...)`, `ctx.vectors.query(...)`, and `ctx.documents.hydrate(...)`
expose the bound embedding profile, logical vector index, and authoritative
SBOL store. The vector and document handles already enforce the request's ACL
scope. Strategies may implement either synchronous or asynchronous `search`.
Omitting `MyStrategy()` from `add_strategy` selects sbol-db's built-in dense
strategy instead.

The process calls `embed(texts, kind="document")` while the native maintenance
worker builds or updates the configured vector backend. Python does not own a
parallel corpus or index: the logical index binding selects exact-flat, FAISS,
or Qdrant, and reindex checkpoints, publication, and recovery remain in the
durable native maintenance plane. Request strategies intentionally receive
read/query access, not mutable index administration.

Load the module from `--search-config`:

```json
{
  "python_plugins": [{
    "module": "my_search_plugin",
    "path": "/opt/sbol-db/plugins"
  }]
}
```

Installed modules omit `path`; relative paths resolve from the search config's
directory. The binary embeds the Python interpreter chosen when PyO3 builds,
so model packages must be installed for that interpreter.
Imports and model construction happen at process startup; failures prevent the
search deployment from activating. Registration metadata is keyword-only and
unknown fields are rejected. The same Python provider is shared between query
and maintenance paths, and one provider or strategy implementation may be
configured under multiple stable profile/strategy IDs when those IDs describe
distinct model spaces.

See [`examples/python-search-bge`](../examples/python-search-bge) for a complete
Hugging Face Transformers implementation backed by sbol-db's integrated FAISS
index.

The included `sbol-db-embedding-fastembed` adapter takes an already initialized
`fastembed::TextEmbedding`, keeping weight acquisition out of request
execution. Its default `dynamic-ort` feature avoids a build-time runtime
download; deployments may instead disable defaults and enable `download-ort`.
`online-models` enables Hugging Face retrieval through rustls. In every case,
the profile's `revision` must name the immutable weight commit or digest that
was loaded; floating `main`/`latest` revisions are rejected.

For local production bundles, `FastEmbedProvider::from_local_bundle` reads the
ONNX model plus the four required tokenizer/config files and verifies their
combined SHA3-256 content revision before creating the ONNX session.
`local_bundle_revision` calculates the exact `sha3-256:...` value to place in
`FastEmbedProviderConfig`. This makes a model-file change a startup error until
the profile and its derived vector generation are deliberately revised.

## Vector backend selection

The logical index router lets deployment topology choose the engine:

| sbol-db role | Initial recommendation | Alternatives to evaluate |
| --- | --- | --- |
| Library, tests, small corpus | `sbol-db-vector-flat` | Qdrant Edge once stable-toolchain-compatible |
| Embedded SQLite or RocksDB application | `sbol-db-search-faiss` for ANN scale; exact-flat for small data | Qdrant Edge, LanceDB, USearch |
| Postgres service | Qdrant self-hosted or Cloud | pgvector to reduce operational components |
| Search service in a larger stack | Qdrant self-hosted or Cloud | deployment's existing vector service |

Exact-flat is deliberately not presented as an ANN performance backend. It is
the correctness oracle: evaluate an approximate backend's recall against its
exact ranking on the same vectors and filters.

Qdrant Edge remains a desired embedded adapter. The current `qdrant-edge`
0.7.2 crate was compile-tested against this workspace's stable Rust 1.93 toolchain
and currently reaches unstable Rust APIs. The project will not set
`RUSTC_BOOTSTRAP` or silently require nightly to ship it. Keep the adapter
boundary and add Edge when it builds on supported stable Rust. FAISS is the
shipping local ANN adapter; retain LanceDB and USearch as embedded alternatives
to benchmark. See the
[Qdrant Edge quickstart](https://qdrant.tech/documentation/edge/edge-quickstart/)
and [LanceDB embedded quickstart](https://docs.lancedb.com/quickstart).

## Generation lifecycle

Indexes are derived artifacts, not application truth. Every backend implements
the same lifecycle:

1. `create_generation` creates an inactive generation with dimension,
   distance, named-vector, and backend parameters;
2. `apply` writes validated batches while the previous generation serves
   queries, and may also update the active generation when the backend
   advertises `incremental_updates`;
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

The FAISS adapter persists the canonical generation spec and sorted sbol-db
document records separately from `index.faiss`. It verifies SHA3-256 checksums
before calling FAISS deserialization, compiles graph and caller filters into
native ID selectors before ranking, and records the exact index factory,
effective `nlist`/`nprobe`, vector count, and FAISS version in the immutable
manifest. The active pointer includes the manifest checksum, so a crash or
partial build cannot make an unready generation queryable.

The built-in `rebuild_vector_index` durable job keyset-pages the primary
store's complete derived SBOL object view, resolves graph IRIs, creates
deterministic labeled embedding text, and returns the maintainer's
`IndexBuildReport` as the durable job result. Deployment code injects the
validated provider/backend registry with
`Worker::with_vector_indexes(Arc<VectorIndexMaintainerRegistry>)`; a worker without that
configuration fails this job kind clearly while continuing to serve other job
kinds.

```json
{
  "artifact_id": "components",
  "generation": "2026-07-26-model-rev-1",
  "vector_name": "content",
  "embedding_profile": "fastembed.bge-small-en-v1.5.rev1",
  "distance": "cosine",
  "batch_size": 64,
  "backend_parameters": { "on_disk": true }
}
```

Callers may enqueue that payload under kind `rebuild_vector_index` with an
idempotency key derived from artifact, generation, and corpus revision. The
optional `maintenance` object in a logical index binding instead opts that
index into automatic maintenance. It supplies the generation-name prefix,
distance, batch size, and backend parameters needed to create a replacement
generation. With it installed, the application invokes SDK-registered
maintenance plugins after successful writes and enqueues their returned durable
tasks. A plugin can choose narrow document synchronization or full
reconciliation without the application knowing a particular vector backend's
lifecycle.

Enabling `maintenance` changes future committed writes only. For an existing
corpus under an explicit deployment, enqueue one initial `rebuild_vector_index`
(or wait for the first mutation, which plans the same full rebuild when no
generation is active) before relying on the index. The shipped built-in index
does this startup rebuild automatically. After activation, document-precise
writes use the incremental path when the selected backend advertises it.

For a document-precise event, the built-in vector plugin enqueues
`maintain_vector_index` when an incremental-capable generation is active. That
job resolves the active generation at execution time and synchronizes selected
identities into it. It therefore follows a concurrent rebuild rather than
becoming permanently stale. The explicit `update_vector_index` operation
remains available to callers who want a job pinned to one named active
generation:

```json
{
  "artifact_id": "components",
  "generation": "2026-07-26-model-rev-1",
  "document_ids": [
    "https://example.org/components/pTet",
    "https://example.org/components/deleted-part"
  ],
  "batch_size": 64
}
```

The handler reads each identity from the authoritative derived object view at
execution time: present objects become canonical embedding upserts and absent
objects become deletes. Repeated delivery therefore converges to the same
state. The explicit named-generation operation requires that generation to be
active at both the start and completion of the update, and its stored embedding
profile, provider, model revision, dimension, and normalization must match the
configured maintainer.

The backend applies changes in bounded batches and flushes before the job
succeeds. A transient failure can leave a prefix applied, but retrying the full
desired-state payload is safe. Exact-flat and Qdrant support incremental
maintenance. FAISS continues to require a full rebuild because it advertises
`incremental_updates = false`. Full rebuild also remains the reconciliation and
model-migration path.

The shipped application emits a document-precise event after SynBioHub and V2
submission, typed object edits, and attachment writes. It emits a
corpus-reconciliation event after destructive object mutations, raw SPARQL
Update, Graph Store writes/deletes, and raw REST imports or graph deletion;
those routes can alter an arbitrary closure and therefore cannot safely name a
complete document set. Task planning happens only after the authoritative write
has succeeded. If durable scheduling then fails, the request reports that
failure rather than pretending the index will converge; replay or a full
rebuild repairs the already committed data write.

## Evaluation and rollout

`sbol-db-search-eval` runs any strategy against a versioned
`EvaluationSuite`. Each case carries a normal `SearchRequest` and graded
document judgments. Reports include precision, recall, reciprocal rank, and
nDCG at configured cutoffs plus per-case latency. Each suite records corpus and
judgment provenance, and each report is bound to the full suite with a SHA-256
fingerprint. Persisted reports can be verified by reconstructing their case and
aggregate metrics from the returned document IDs.

Promotion is a paired comparison against the same suite revision. A
`QualityGate` requires a minimum case count, an absolute minimum candidate
mean nDCG, a minimum mean nDCG delta, and a maximum fraction of queries that
regress beyond tolerance. Comparison uses the suite judgments rather than
trusting cached report aggregates and includes every per-query delta. This
prevents a large gain on a few queries—or a tiny improvement over a poor
baseline—from concealing weak overall relevance.

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
- local FastEmbed/ONNX provider adapter;
- generation rebuild/activation/rollback coordinator;
- durable full-corpus vector rebuild job and canonical SBOL projection;
- durable desired-state incremental vector update job with embedding-space
  validation;
- SDK maintenance plugins, durable task scheduling, and automatic write-path
  events for SynBioHub, V2, SPARQL, Graph Store, and REST ingestion; and
- offline relevance metrics and paired quality gate.

Next:

1. persist artifact/snapshot provenance with each scheduled maintenance intent
   and add an outbox for atomic write-and-schedule delivery;
2. expose backend/profile/strategy assembly through typed server
   configuration;
3. run live Qdrant lifecycle integration tests in CI;
4. benchmark Qdrant, pgvector, Qdrant Edge (when stable), and LanceDB against
   exact-flat; and
5. add hybrid fusion/reranking, then the separately bounded agentic tool
   broker and trace model.
