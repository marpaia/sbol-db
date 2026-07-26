//! Faceted ranked search over the shared tantivy index.
//!
//! [`FacetedSearch`] is the facade's typed search value object: the classic
//! SynBioHub `/search/<key>=<value>&.../<freetext>` path grammar is parsed in
//! the HTTP adapter and never reaches here, so the wire quirk stays quarantined
//! and the facade sees only a typed query.
//!
//! [`AppServices::ranked_search`] answers the free-text relevance path against
//! the [`RankedTextIndex`](sbol_db_search::ranked_text::RankedTextIndex) the
//! facade owns, enforcing the caller's [`GraphScope`] inside the index and
//! narrowing the ranked hits by the `objectType` facet. Purely faceted queries
//! (no free text) are answered by the SPARQL engine in the adapter, so this
//! verb is only the relevance surface.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_search::ranked_text::{cluster_map, GraphFilter, Hit, RankedTextIndex};
use sbol_db_search::SearchRuntime;
use sbol_db_search_sdk::{
    DataEgress, DocumentId, FilterCapability, FilterKind, HydratedDocument, PaginationCapability,
    ScopedDocumentHydrator, ScoreKind, SearchContext, SearchError, SearchHit, SearchInput,
    SearchInputKind, SearchPage, SearchRequest, SearchScope, SearchStrategy, StrategyCapabilities,
    StrategyDescriptor, StrategyRef, StrategyRegistry, StrategyRequirements, Total,
    TotalCapability,
};
use sbol_db_sparql::GraphScope;
use sbol_db_storage::{ClusterStore, SbolStore};

use crate::AppServices;

/// The classic SynBioHub default page size when a request names no `limit`.
const DEFAULT_LIMIT: usize = 50;

/// The ranked candidate pool pulled from the index before the offset/limit
/// window is taken. Matches the index's own fetch cap so a facet filter never
/// silently drops in-scope hits below the window.
const RANKED_FETCH: usize = 10_000;

struct StoreDocumentHydrator {
    store: Arc<dyn SbolStore>,
    scope: SearchScope,
}

#[async_trait]
impl ScopedDocumentHydrator for StoreDocumentHydrator {
    async fn hydrate(
        &self,
        document_ids: Vec<DocumentId>,
    ) -> Result<Vec<HydratedDocument>, SearchError> {
        let iris = document_ids
            .iter()
            .map(|document_id| document_id.0.as_str())
            .collect::<Vec<_>>();
        let records = self
            .store
            .get_objects_by_iris(&iris)
            .await
            .map_err(|error| SearchError::Backend(error.to_string()))?;
        let mut documents = Vec::with_capacity(records.len());
        for record in records {
            let graph = match record.graph_id {
                Some(graph_id) => self
                    .store
                    .get_graph(graph_id)
                    .await
                    .map_err(|error| SearchError::Backend(error.to_string()))?
                    .and_then(|graph| graph.document_iri.map(|iri| iri.into_inner())),
                None => None,
            };
            if !scope_contains(&self.scope, graph.as_deref()) {
                continue;
            }
            let mut object_types = Vec::with_capacity(record.types.len() + 1);
            object_types.push(record.sbol_class);
            for object_type in record.types {
                if !object_types.contains(&object_type) {
                    object_types.push(object_type);
                }
            }
            let uri = record.iri.into_inner();
            documents.push(HydratedDocument {
                document_id: DocumentId(uri.clone()),
                uri,
                graph,
                display_id: record.display_id,
                version: None,
                name: record.name,
                description: record.description,
                object_types,
            });
        }
        Ok(documents)
    }
}

fn scope_contains(scope: &SearchScope, graph: Option<&str>) -> bool {
    match scope {
        SearchScope::Union => true,
        SearchScope::Only(graphs) => graph.is_some_and(|graph| graphs.iter().any(|g| g == graph)),
    }
}

/// Which timestamp a date-range facet constrains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateField {
    Created,
    Modified,
}

/// A typed faceted search: the `objectType` class, an optional collection the
/// results must be a member of, arbitrary predicate-equality facets, an
/// optional date range, and the free-text term. Paging is carried alongside so
/// one value drives both the ranked and the SPARQL paths.
#[derive(Clone, Debug, Default)]
pub struct FacetedSearch {
    /// The `objectType` facet as a full rdf:type IRI.
    pub class: Option<String>,
    /// A collection whose `sbol2:member` the results must be.
    pub collection_member: Option<String>,
    /// Predicate-equality facets: each `(predicate, object)` where the
    /// predicate is a prefixed name or a full IRI and the object is a literal
    /// or a full IRI, exactly as the wire carried them.
    pub predicate_eq: Vec<(String, String)>,
    /// Which timestamp the date range constrains, when one is present.
    pub date_field: Option<DateField>,
    /// Inclusive upper bound of the date range (a bare `YYYY-MM-DD`).
    pub date_before: Option<String>,
    /// Inclusive lower bound of the date range (a bare `YYYY-MM-DD`).
    pub date_after: Option<String>,
    /// The free-text term, absent for a purely faceted query.
    pub free_text: Option<String>,
    /// The paging offset.
    pub offset: usize,
    /// The page size, defaulting to the classic 50 when absent.
    pub limit: Option<usize>,
}

impl FacetedSearch {
    /// The effective page size, applying the classic default.
    pub fn effective_limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT)
    }
}

/// Map the caller's authorized [`GraphScope`] onto the index's graph filter so
/// the scope is enforced in the query rather than by post-filtering.
fn graph_filter(scope: GraphScope) -> GraphFilter {
    match scope {
        GraphScope::Union => GraphFilter::Any,
        GraphScope::Only(graphs) => GraphFilter::Only(graphs),
    }
}

/// Built-in structured strategy that preserves the current
/// Tantivy/PageRank/cluster-penalty ranking. Compatibility HTTP adapters remain
/// wired directly to [`AppServices::ranked_search`]; registering this strategy
/// makes the same semantics available to the new pluggable runtime without
/// changing those existing call paths.
pub struct LegacyExplorerStrategy {
    descriptor: StrategyDescriptor,
    text_search: Arc<RankedTextIndex>,
    cluster: Arc<dyn ClusterStore>,
}

impl LegacyExplorerStrategy {
    pub const ID: &'static str = "legacy.explorer.v1";

    pub fn new(text_search: Arc<RankedTextIndex>, cluster: Arc<dyn ClusterStore>) -> Self {
        Self {
            descriptor: StrategyDescriptor {
                id: Self::ID.to_owned(),
                version: "1".to_owned(),
                display_name: "Legacy SBOLExplorer ranking".to_owned(),
                description: "Tantivy fuzzy text ranking combined with PageRank and sequence/cluster penalties".to_owned(),
                capabilities: StrategyCapabilities {
                    inputs: vec![SearchInputKind::Text],
                    filters: vec![FilterKind::Graph, FilterKind::ObjectType],
                    // Graph authorization is native to Tantivy. The legacy
                    // object-type facet is applied to its bounded candidate
                    // pool, matching the existing application behavior.
                    filter_execution: FilterCapability::PostFilter,
                    pagination: PaginationCapability::Cursor,
                    totals: TotalCapability::Exact,
                    deterministic: true,
                    explanations: false,
                    data_egress: DataEgress::None,
                },
                requirements: StrategyRequirements::default(),
            },
            text_search,
            cluster,
        }
    }
}

#[async_trait]
impl SearchStrategy for LegacyExplorerStrategy {
    fn descriptor(&self) -> &StrategyDescriptor {
        &self.descriptor
    }

    async fn search(
        &self,
        ctx: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        let SearchInput::Text { text } = request.query else {
            return Err(SearchError::Unsupported(
                "legacy.explorer.v1 accepts only text input".to_owned(),
            ));
        };
        if !request.filters.predicates.is_empty() {
            return Err(SearchError::Unsupported(
                "legacy.explorer.v1 does not support predicate filters".to_owned(),
            ));
        }

        let graph_filter = scoped_graph_filter(ctx.scope(), &request.filters.graphs);
        let clusters = cluster_map(
            self.cluster
                .all_assignments()
                .await
                .map_err(|error| SearchError::Backend(error.to_string()))?,
        );
        let ranked = self
            .text_search
            .search(&text, 0, RANKED_FETCH, &graph_filter, &clusters)
            .map_err(|error| SearchError::Backend(format!("ranked search: {error}")))?;

        let object_types: HashSet<&str> = request
            .filters
            .object_types
            .iter()
            .map(String::as_str)
            .collect();
        let filtered: Vec<_> = ranked
            .into_iter()
            .filter(|hit| {
                object_types.is_empty()
                    || hit
                        .type_iri
                        .as_deref()
                        .is_some_and(|type_iri| object_types.contains(type_iri))
            })
            .collect();
        let total = filtered.len();
        let offset = decode_cursor(request.page.cursor.as_deref())?;
        let limit = request.page.limit;
        let items = filtered
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(structured_hit)
            .collect();
        let next_offset = offset.saturating_add(limit);

        Ok(SearchPage {
            strategy: StrategyRef {
                id: self.descriptor.id.clone(),
                version: self.descriptor.version.clone(),
            },
            items,
            total: Total::Exact(total),
            next_cursor: (next_offset < total).then(|| next_offset.to_string()),
            execution: Default::default(),
        })
    }
}

fn scoped_graph_filter(scope: &SearchScope, requested: &[String]) -> GraphFilter {
    let requested: HashSet<&str> = requested.iter().map(String::as_str).collect();
    match scope {
        SearchScope::Union if requested.is_empty() => GraphFilter::Any,
        SearchScope::Union => {
            GraphFilter::Only(requested.iter().map(|g| (*g).to_owned()).collect())
        }
        SearchScope::Only(authorized) if requested.is_empty() => {
            GraphFilter::Only(authorized.clone())
        }
        SearchScope::Only(authorized) => GraphFilter::Only(
            authorized
                .iter()
                .filter(|graph| requested.contains(graph.as_str()))
                .cloned()
                .collect(),
        ),
    }
}

fn decode_cursor(cursor: Option<&str>) -> Result<usize, SearchError> {
    cursor
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| SearchError::InvalidRequest("invalid legacy search cursor".to_owned()))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn structured_hit(hit: Hit) -> SearchHit {
    let document_id = DocumentId(hit.subject.clone());
    SearchHit {
        document_id,
        uri: hit.subject,
        graph: None,
        score: hit.score as f32,
        score_kind: ScoreKind::LegacyExplorer,
        display_id: hit.display_id,
        version: hit.version,
        name: hit.name,
        description: hit.description,
        object_types: hit.type_iri.into_iter().collect(),
        evidence: Vec::new(),
    }
}

impl AppServices {
    /// The configured structured-search runtime. By default this is assembled
    /// lazily from the current text index and cluster store and contains only
    /// [`LegacyExplorerStrategy`]. Deployments can install a richer immutable
    /// runtime with [`AppServices::with_search_runtime`].
    pub fn search_runtime(&self) -> Arc<SearchRuntime> {
        self.search_runtime
            .get_or_init(|| {
                let strategies = StrategyRegistry::builder()
                    .register(LegacyExplorerStrategy::new(
                        self.text_search.clone(),
                        self.cluster.clone(),
                    ))
                    .expect("the built-in legacy strategy has a unique non-empty id")
                    .build();
                Arc::new(
                    SearchRuntime::new(strategies, LegacyExplorerStrategy::ID)
                        .expect("the built-in default strategy is registered"),
                )
            })
            .clone()
    }

    /// Execute the structured search surface under the same graph scope the
    /// identity layer computes for all other application reads.
    pub async fn structured_search(
        &self,
        request: SearchRequest,
        scope: GraphScope,
    ) -> Result<SearchPage, SearchError> {
        let budget = sbol_db_search_sdk::SearchBudget {
            timeout_ms: request.options.timeout_ms,
            ..sbol_db_search_sdk::SearchBudget::default()
        };
        let scope = match scope {
            GraphScope::Union => SearchScope::Union,
            GraphScope::Only(graphs) => SearchScope::Only(graphs),
        };
        let mut ctx = SearchContext::new(scope.clone(), budget).with_documents(Arc::new(
            StoreDocumentHydrator {
                store: self.store.clone(),
                scope: scope.clone(),
            },
        ));
        if let Some(router) = &self.search_vectors {
            ctx = ctx.with_vectors(router.scoped(scope));
        }
        self.search_runtime().search(ctx, request).await
    }

    /// Rank the in-scope objects matching the free-text term, narrowed by the
    /// `objectType` facet, and return the requested window plus the total
    /// number of matches. The cluster-duplicate map is built from the persisted
    /// sequence-cluster assignments, so a non-centroid cluster member takes the
    /// index's divide-by-2 penalty. The assignments are scanned per call; the
    /// map is small relative to the candidate pool.
    pub async fn ranked_search(
        &self,
        query: &FacetedSearch,
        scope: GraphScope,
    ) -> Result<(Vec<Hit>, usize), DomainError> {
        let filter = graph_filter(scope);
        let clusters = cluster_map(self.cluster.all_assignments().await?);
        let free_text = query.free_text.clone().unwrap_or_default();
        let ranked = self
            .text_search
            .search(&free_text, 0, RANKED_FETCH, &filter, &clusters)
            .map_err(|e| DomainError::Database(format!("ranked search: {e}")))?;

        let class = query.class.clone();
        let filtered: Vec<Hit> = ranked
            .into_iter()
            .filter(|hit| match &class {
                Some(class) => hit.type_iri.as_deref() == Some(class.as_str()),
                None => true,
            })
            .collect();

        let total = filtered.len();
        let window = filtered
            .into_iter()
            .skip(query.offset)
            .take(query.effective_limit())
            .collect();
        Ok((window, total))
    }

    /// The number of in-scope objects the free-text term matches under the
    /// `objectType` facet: [`ranked_search`](Self::ranked_search)'s total,
    /// discarding the hit window.
    pub async fn ranked_search_count(
        &self,
        query: &FacetedSearch,
        scope: GraphScope,
    ) -> Result<usize, DomainError> {
        Ok(self.ranked_search(query, scope).await?.1)
    }
}

#[cfg(test)]
mod structured_tests {
    use super::*;
    use crate::memory::InMemoryClusterStore;
    use sbol_db_search::ranked_text::IndexedPart;
    use sbol_db_search_sdk::{PageRequest, SearchBudget, SearchFilters, SearchOptions};

    fn request(graphs: Vec<String>) -> SearchRequest {
        SearchRequest {
            strategy: Some(LegacyExplorerStrategy::ID.to_owned()),
            query: SearchInput::Text {
                text: "promoter".to_owned(),
            },
            filters: SearchFilters {
                graphs,
                ..SearchFilters::default()
            },
            page: PageRequest::default(),
            options: SearchOptions::default(),
        }
    }

    #[tokio::test]
    async fn requested_graphs_can_only_narrow_the_authorized_scope() {
        let index = Arc::new(RankedTextIndex::in_ram().expect("index"));
        index
            .rebuild([
                IndexedPart {
                    subject: "https://example.org/public".to_owned(),
                    graph: "https://example.org/graphs/public".to_owned(),
                    display_id: Some("public_promoter".to_owned()),
                    name: Some("Public promoter".to_owned()),
                    description: None,
                    version: None,
                    type_iris: vec!["https://example.org/Component".to_owned()],
                    keywords: "promoter".to_owned(),
                    pagerank: 1.0,
                },
                IndexedPart {
                    subject: "https://example.org/private".to_owned(),
                    graph: "https://example.org/graphs/private".to_owned(),
                    display_id: Some("private_promoter".to_owned()),
                    name: Some("Private promoter".to_owned()),
                    description: None,
                    version: None,
                    type_iris: vec!["https://example.org/Component".to_owned()],
                    keywords: "promoter".to_owned(),
                    pagerank: 2.0,
                },
            ])
            .expect("rebuild");
        let strategy = LegacyExplorerStrategy::new(index, Arc::new(InMemoryClusterStore::new()));
        let ctx = SearchContext::new(
            SearchScope::Only(vec!["https://example.org/graphs/public".to_owned()]),
            SearchBudget::default(),
        );

        let visible = strategy
            .search(ctx.clone(), request(Vec::new()))
            .await
            .expect("authorized search");
        assert_eq!(visible.items.len(), 1);
        assert_eq!(visible.items[0].uri, "https://example.org/public");

        let widened = strategy
            .search(
                ctx,
                request(vec!["https://example.org/graphs/private".to_owned()]),
            )
            .await
            .expect("narrowed search");
        assert!(widened.items.is_empty());
        assert_eq!(widened.total, Total::Exact(0));
    }
}
