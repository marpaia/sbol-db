//! Strategy dispatch for the structured search surface.
//!
//! The runtime is intentionally small: it selects a registered strategy,
//! validates the request against that strategy's declared capabilities, and
//! delegates execution. Concrete strategies and scoped services are assembled
//! by the application crate.

use std::time::Duration;

use sbol_db_search_sdk::{
    FilterKind, PaginationCapability, SearchContext, SearchError, SearchInput, SearchInputKind,
    SearchPage, SearchRequest, StrategyDescriptor, StrategyRegistry,
};

/// Maximum page size accepted by the structured runtime independent of any
/// backend's larger internal candidate pool.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// Immutable strategy dispatcher shared by request handlers.
pub struct SearchRuntime {
    strategies: StrategyRegistry,
    default_strategy: String,
}

impl SearchRuntime {
    pub fn new(
        strategies: StrategyRegistry,
        default_strategy: impl Into<String>,
    ) -> Result<Self, SearchError> {
        let default_strategy = default_strategy.into();
        if strategies.get(&default_strategy).is_none() {
            return Err(SearchError::Configuration(format!(
                "default strategy `{default_strategy}` is not registered"
            )));
        }
        Ok(Self {
            strategies,
            default_strategy,
        })
    }

    pub fn default_strategy(&self) -> &str {
        &self.default_strategy
    }

    pub fn descriptors(&self) -> Vec<StrategyDescriptor> {
        self.strategies.descriptors()
    }

    pub async fn search(
        &self,
        ctx: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        let strategy_id = request
            .strategy
            .as_deref()
            .unwrap_or(self.default_strategy.as_str());
        let strategy = self.strategies.get(strategy_id).ok_or_else(|| {
            SearchError::InvalidRequest(format!("unknown search strategy `{strategy_id}`"))
        })?;
        validate_request(strategy.descriptor(), &request)?;
        let Some(timeout_ms) = ctx.budget().timeout_ms else {
            return strategy.search(ctx, request).await;
        };
        tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            strategy.search(ctx, request),
        )
        .await
        .map_err(|_| SearchError::Timeout { timeout_ms })?
    }
}

fn validate_request(
    descriptor: &StrategyDescriptor,
    request: &SearchRequest,
) -> Result<(), SearchError> {
    if request.page.limit == 0 || request.page.limit > MAX_PAGE_SIZE {
        return Err(SearchError::InvalidRequest(format!(
            "page limit must be between 1 and {MAX_PAGE_SIZE}"
        )));
    }
    if request.options.timeout_ms == Some(0) {
        return Err(SearchError::InvalidRequest(
            "search timeout_ms must be greater than zero".to_owned(),
        ));
    }

    let input = match &request.query {
        SearchInput::Text { .. } => SearchInputKind::Text,
        SearchInput::Similar { .. } => SearchInputKind::Similar,
        SearchInput::Sequence { .. } => SearchInputKind::Sequence,
    };
    if !descriptor.capabilities.inputs.contains(&input) {
        return Err(SearchError::Unsupported(format!(
            "strategy `{}` does not accept {input:?} input",
            descriptor.id
        )));
    }

    let mut required_filters = Vec::new();
    if !request.filters.graphs.is_empty() {
        required_filters.push(FilterKind::Graph);
    }
    if !request.filters.object_types.is_empty() {
        required_filters.push(FilterKind::ObjectType);
    }
    if !request.filters.predicates.is_empty() {
        required_filters.push(FilterKind::Predicate);
    }
    if let Some(filter) = required_filters
        .into_iter()
        .find(|filter| !descriptor.capabilities.filters.contains(filter))
    {
        return Err(SearchError::Unsupported(format!(
            "strategy `{}` does not support {filter:?} filters",
            descriptor.id
        )));
    }

    if request.page.cursor.is_some()
        && descriptor.capabilities.pagination != PaginationCapability::Cursor
    {
        return Err(SearchError::Unsupported(format!(
            "strategy `{}` does not support cursor pagination",
            descriptor.id
        )));
    }
    if request.options.explain && !descriptor.capabilities.explanations {
        return Err(SearchError::Unsupported(format!(
            "strategy `{}` does not provide explanations",
            descriptor.id
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sbol_db_search_sdk::{
        DataEgress, FilterCapability, PageRequest, SearchBudget, SearchFilters, SearchOptions,
        SearchScope, StrategyCapabilities, StrategyRef, StrategyRequirements, Total,
        TotalCapability,
    };

    use super::*;

    struct Stub {
        descriptor: StrategyDescriptor,
        pending: bool,
    }

    #[async_trait]
    impl sbol_db_search_sdk::SearchStrategy for Stub {
        fn descriptor(&self) -> &StrategyDescriptor {
            &self.descriptor
        }

        async fn search(
            &self,
            _ctx: SearchContext,
            _request: SearchRequest,
        ) -> Result<SearchPage, SearchError> {
            if self.pending {
                std::future::pending::<()>().await;
            }
            Ok(SearchPage {
                strategy: StrategyRef {
                    id: self.descriptor.id.clone(),
                    version: self.descriptor.version.clone(),
                },
                items: Vec::new(),
                total: Total::Exact(0),
                next_cursor: None,
                execution: Default::default(),
            })
        }
    }

    fn runtime() -> SearchRuntime {
        let strategy = Stub {
            descriptor: StrategyDescriptor {
                id: "stub.v1".to_owned(),
                version: "1".to_owned(),
                display_name: "Stub".to_owned(),
                description: String::new(),
                capabilities: StrategyCapabilities {
                    inputs: vec![SearchInputKind::Text],
                    filters: vec![FilterKind::Graph],
                    filter_execution: FilterCapability::Native,
                    pagination: PaginationCapability::FirstPageOnly,
                    totals: TotalCapability::Exact,
                    deterministic: true,
                    explanations: false,
                    data_egress: DataEgress::None,
                },
                requirements: StrategyRequirements::default(),
            },
            pending: false,
        };
        let registry = StrategyRegistry::builder()
            .register(strategy)
            .expect("register")
            .build();
        SearchRuntime::new(registry, "stub.v1").expect("runtime")
    }

    fn request() -> SearchRequest {
        SearchRequest {
            strategy: None,
            query: SearchInput::Text {
                text: "promoter".to_owned(),
            },
            filters: SearchFilters::default(),
            page: PageRequest::default(),
            options: SearchOptions::default(),
        }
    }

    #[tokio::test]
    async fn dispatches_the_default_strategy() {
        let result = runtime()
            .search(
                SearchContext::new(SearchScope::Union, SearchBudget::default()),
                request(),
            )
            .await
            .expect("search");
        assert_eq!(result.strategy.id, "stub.v1");
    }

    #[tokio::test]
    async fn rejects_undeclared_inputs_and_paging() {
        let mut unsupported = request();
        unsupported.query = SearchInput::Similar {
            uri: "https://example.org/part".to_owned(),
        };
        assert!(matches!(
            runtime()
                .search(
                    SearchContext::new(SearchScope::Union, SearchBudget::default()),
                    unsupported,
                )
                .await,
            Err(SearchError::Unsupported(_))
        ));

        let mut cursor = request();
        cursor.page.cursor = Some("50".to_owned());
        assert!(matches!(
            runtime()
                .search(
                    SearchContext::new(SearchScope::Union, SearchBudget::default()),
                    cursor,
                )
                .await,
            Err(SearchError::Unsupported(_))
        ));
    }

    #[tokio::test]
    async fn enforces_the_context_execution_budget() {
        let mut runtime = runtime();
        runtime.strategies = StrategyRegistry::builder()
            .register(Stub {
                descriptor: runtime.descriptors().remove(0),
                pending: true,
            })
            .expect("register")
            .build();
        let mut request = request();
        request.options.timeout_ms = Some(5);
        let result = runtime
            .search(
                SearchContext::new(
                    SearchScope::Union,
                    SearchBudget {
                        timeout_ms: Some(5),
                        ..SearchBudget::default()
                    },
                ),
                request,
            )
            .await;
        assert_eq!(result, Err(SearchError::Timeout { timeout_ms: 5 }));
    }

    #[tokio::test]
    async fn rejects_a_zero_timeout() {
        let mut request = request();
        request.options.timeout_ms = Some(0);
        let result = runtime()
            .search(
                SearchContext::new(SearchScope::Union, SearchBudget::default()),
                request,
            )
            .await;
        assert!(matches!(result, Err(SearchError::InvalidRequest(_))));
    }
}
