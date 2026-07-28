use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable backend-neutral identity of one graph-scoped search document.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(pub String);

/// An input accepted by a search strategy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchInput {
    Text {
        text: String,
    },
    Similar {
        uri: String,
    },
    Sequence {
        sequence: String,
        #[serde(default)]
        exact: bool,
    },
}

/// One predicate-equality narrowing requested by a caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredicateFilter {
    pub predicate: String,
    pub value: String,
}

/// Caller-requested narrowing. Authorization scope is carried separately and
/// can only reduce this set further.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub object_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicates: Vec<PredicateFilter>,
}

/// Paging for the structured API. A strategy that cannot provide stable
/// continuation leaves `next_cursor` absent in its response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

const fn default_limit() -> usize {
    50
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            cursor: None,
        }
    }
}

/// Optional execution controls shared by every structured strategy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchOptions {
    #[serde(default)]
    pub explain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// The new structured search request. An absent strategy asks the runtime to
/// use its configured default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    pub query: SearchInput,
    #[serde(default)]
    pub filters: SearchFilters,
    #[serde(default)]
    pub page: PageRequest,
    #[serde(default)]
    pub options: SearchOptions,
}

/// Authorization ceiling computed by the application, never accepted directly
/// from the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchScope {
    Union,
    Only(Vec<String>),
}

/// Per-request resource limits available to strategy implementations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchBudget {
    pub timeout_ms: Option<u64>,
    pub max_candidates: usize,
    pub max_tool_calls: usize,
}

impl Default for SearchBudget {
    fn default() -> Self {
        Self {
            timeout_ms: None,
            max_candidates: 10_000,
            max_tool_calls: 0,
        }
    }
}

/// The semantics of a hit score. Scores with different kinds must not be
/// compared without an explicit normalization or fusion policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreKind {
    LegacyExplorer,
    CosineSimilarity,
    DotProduct,
    NegativeDistance,
    ReciprocalRankFusion,
    Reranker,
    Custom(String),
}

/// Evidence explaining how one stage contributed to a hit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

/// Backend-neutral search hit. Metadata is hydrated from the authorized primary
/// store rather than treated as authoritative vector-backend payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub document_id: DocumentId,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<String>,
    pub score: f32,
    pub score_kind: ScoreKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub object_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

/// Authoritative metadata loaded from the primary graph store after candidate
/// generation. Vector payload is intentionally not a substitute for this
/// representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedDocument {
    pub document_id: DocumentId,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub object_types: Vec<String>,
}

/// Total-count guarantee returned by a strategy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Total {
    Exact(usize),
    LowerBound(usize),
    Unknown,
}

/// Identity of the strategy that actually served a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRef {
    pub id: String,
    pub version: String,
}

/// Reproducibility and diagnostics metadata for one execution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionMetadata {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifact_generations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

/// One page returned by a search strategy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub strategy: StrategyRef,
    pub items: Vec<SearchHit>,
    pub total: Total,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub execution: ExecutionMetadata,
}
