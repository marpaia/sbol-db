use serde::{Deserialize, Serialize};

/// Query input shapes a strategy accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchInputKind {
    Text,
    Similar,
    Sequence,
}

/// Structured filters understood by a strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    Graph,
    ObjectType,
    Predicate,
}

/// Where a strategy applies its declared filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterCapability {
    None,
    PostFilter,
    Native,
}

/// Paging guarantees made by a strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaginationCapability {
    Offset,
    Cursor,
    FirstPageOnly,
}

/// Total-count guarantees made by a strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TotalCapability {
    Exact,
    LowerBound,
    Unknown,
}

/// Whether execution can send query or corpus data out of the sbol-db process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataEgress {
    None,
    ConfiguredRemote,
}

/// Capabilities exposed by a registered strategy and returned by discovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyCapabilities {
    pub inputs: Vec<SearchInputKind>,
    pub filters: Vec<FilterKind>,
    pub filter_execution: FilterCapability,
    pub pagination: PaginationCapability,
    pub totals: TotalCapability,
    pub deterministic: bool,
    pub explanations: bool,
    pub data_egress: DataEgress,
}

/// Named dependencies the runtime must resolve before activating a strategy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRequirements {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embedding_profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vector_indexes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_sources: Vec<String>,
}

/// Stable identity and capability declaration for a strategy implementation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyDescriptor {
    pub id: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub capabilities: StrategyCapabilities,
    #[serde(default)]
    pub requirements: StrategyRequirements,
}
