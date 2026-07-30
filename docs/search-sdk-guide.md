# Contribute a search idea to sbol-db

This is the shortest path from a search idea to evidence that it helps.

Your first contribution should usually be one opt-in `SearchStrategy`: give it
a stable ID, register it beside the current strategy, and compare both on the
same relevance cases. The compatibility search routes do not change, and users
can select your experiment explicitly through `POST /api/v2/search`.

## Start with a testable claim

Write down one claim before writing infrastructure. For example:

- “Reciprocal-rank fusion improves nDCG@10 for descriptive text queries.”
- “A sequence-aware reranker improves recall@20 for part discovery.”
- “A bounded agent resolves ambiguous intent without increasing regressions.”

Then collect a small, representative set of queries and relevant document IDs.
That set becomes your first `EvaluationSuite`; it keeps implementation choices
connected to user-visible search quality.

## Pick the smallest extension point

Implement `SearchStrategy` when your idea owns the end-to-end behavior of a
request. This is the recommended first path and works for embedding, neural,
agentic, graph, sequence, and classic algorithmic search.

Use a smaller trait only when your idea is naturally one reusable stage:

- `CandidateSource` generates candidates;
- `Fusion` combines ranked candidate lists; or
- `Reranker` reorders a bounded candidate set.

Compose that stage into a `SearchStrategy` so it can still be selected and
evaluated through the common API. Do not start by implementing an embedding
provider or vector backend unless the infrastructure itself is the experiment.

## Implement one strategy

Create a small crate or add a focused module under `sbol-db-search`. The core
contract is deliberately narrow:

```rust
#[async_trait]
pub trait SearchStrategy: Send + Sync + 'static {
    fn descriptor(&self) -> &StrategyDescriptor;

    async fn search(
        &self,
        ctx: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError>;
}
```

Copy the runnable
[`contributor_strategy.rs`](../crates/sbol-db-search-eval/examples/contributor_strategy.rs)
example. It is a complete public-API implementation, baseline comparison, and
quality gate—not pseudocode:

```console
cargo run -p sbol-db-search-eval --example contributor_strategy
```

Replace its `score` method with your idea and give the strategy a stable ID.
Keep three rules visible while implementing it:

- use `ctx.scope()` as the authorization ceiling;
- respect `ctx.budget()` when retrieving candidates or calling tools (the
  runtime enforces `timeout_ms`; the strategy must enforce candidate and tool
  limits); and
- if retrieval starts from IDs, use `ctx.documents()` for authoritative result
  metadata rather than trusting an index payload.

The descriptor is enforced at runtime. Declare only capabilities you implement;
the API will reject unsupported filters, cursors, inputs, or explanations before
your strategy runs.

## Define index maintenance

Search plugins can also define how their artifacts remain current. Implement
`IndexMaintenancePlugin` when a committed write should turn into a durable
maintenance task. The SDK sees only the mutation source and either a complete,
deduplicated set of affected document IDs or a corpus-level invalidation; it
does not depend on the database or job queue.

```rust,ignore
use async_trait::async_trait;
use sbol_db_search_sdk::{
    IndexMaintenanceDescriptor, IndexMaintenanceEvent, IndexMaintenancePlugin,
    IndexMaintenanceTask, IndexMutationScope, SearchError,
};

struct MyIndexMaintenance {
    descriptor: IndexMaintenanceDescriptor,
}

#[async_trait]
impl IndexMaintenancePlugin for MyIndexMaintenance {
    fn descriptor(&self) -> &IndexMaintenanceDescriptor {
        &self.descriptor
    }

    async fn plan(
        &self,
        event: &IndexMaintenanceEvent,
    ) -> Result<Vec<IndexMaintenanceTask>, SearchError> {
        match &event.scope {
            IndexMutationScope::Documents { document_ids } => Ok(vec![
                IndexMaintenanceTask::new(
                    "my_index_sync",
                    serde_json::json!({ "document_ids": document_ids }),
                ),
            ]),
            IndexMutationScope::Corpus => Ok(vec![
                IndexMaintenanceTask::new("my_index_rebuild", serde_json::json!({})),
            ]),
        }
    }
}
```

Register it through `SearchDeploymentBuilder::register_maintenance_plugin`.
After an authoritative application write commits, `AppServices` asks every
registered plugin to plan and persists the returned tasks in its durable job
queue. A plugin must make repeated task delivery converge. Typed submission,
object-edit, and attachment paths provide exact document events; raw SPARQL,
Graph Store, and import routes provide corpus events. The application reports a
post-commit scheduling failure rather than claiming an index is current.

## Register and try it

Register the strategy where the server assembles its search deployment:

```rust,ignore
let deployment = load_builder(&path)
    .await?
    .register_strategy(Arc::new(MyStrategy::new()))?
    .build()?;
```

Confirm that the strategy appears in discovery:

```console
curl http://localhost:8000/api/v2/search/strategies
```

Then select it explicitly:

```console
curl -X POST http://localhost:8000/api/v2/search \
  -H 'content-type: application/json' \
  -d '{
    "strategy": "contrib.my-idea.v1",
    "query": {"kind": "text", "text": "inducible promoter"},
    "page": {"limit": 10}
  }'
```

Do not make an experimental strategy the deployment default in the same first
change. Keeping selection explicit makes the contribution safe to review and
easy to compare.

## Show that it helps

Add versioned cases to an `EvaluationSuite`, then call `evaluate_strategy` for
both the current baseline and your candidate using the same suite, scope, and
cutoffs. Pass the suite and both reports to `compare` with an explicit
`QualityGate`. Start from
[`contributor-smoke-v1.json`](../crates/sbol-db-search-eval/fixtures/contributor-smoke-v1.json)
to exercise the plumbing, then replace its synthetic cases with evidence that
represents your intended users.

Your pull request should report:

- the hypothesis and suite revision;
- nDCG and recall at the cutoff that matters;
- wins, ties, and regressions by query;
- mean latency for both strategies; and
- any known corpus, model, or deployment limitations.

The goal is not merely a positive average. Reports fingerprint the complete
suite and comparison reconstructs metrics from returned IDs and judgments.
The paired report shows which queries improved, while `min_cases` and the
regression limit keep a tiny or uneven result from passing accidentally.

## A useful first pull request

Before opening the PR, make sure it includes:

- one narrow strategy or composable stage;
- a stable strategy ID and honest capability declaration;
- unit tests that distinguish the new behavior from the baseline;
- authorization, budget, and metadata-hydration handling;
- a versioned evaluation fixture and paired comparison; and
- no change to compatibility routes or the default strategy.

Run the focused crate tests while iterating, then the workspace checks required
by CI.

## Read only what you need next

- [`strategy.rs`](../crates/sbol-db-search-sdk/src/strategy.rs) defines the
  strategy boundary and request-scoped services.
- [`composition.rs`](../crates/sbol-db-search-sdk/src/composition.rs) defines
  candidate, fusion, and reranking stages.
- [`embedding_strategy.rs`](../crates/sbol-db-search/src/embedding_strategy.rs)
  is the reference for an embedding-backed strategy.
- [`sbol-db-search-eval`](../crates/sbol-db-search-eval/src/lib.rs) contains the
  evaluation suite, metrics, and paired quality gate.
- [Pluggable search](search-plugins.md) covers deployment configuration,
  embedding providers, vector backends, and index maintenance when your idea
  actually needs that infrastructure.

Existing vector implementations include exact-flat for a correctness oracle,
FAISS for embedded native ANN, and Qdrant for service or edge deployments.
Treat the vector engine as a replaceable index: sbol-db remains the database
and source of authoritative SBOL metadata. Backends that advertise
`incremental_updates` must accept idempotent upserts and deletes against the
active generation; a missing delete still counts as an accepted operation.
The high-level maintainer verifies stored embedding-space provenance before it
uses that capability.
