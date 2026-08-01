//! Compatibility ranking and native faceted discovery over the shared corpus.
//!
//! [`FacetedSearch`] is the facade's typed search value object: the classic
//! SynBioHub `/search/<key>=<value>&.../<freetext>` path grammar is parsed in
//! the HTTP adapter and never reaches here, so the wire quirk stays quarantined
//! and the facade sees only a typed query.
//!
//! [`AppServices::ranked_search`] answers the compatibility free-text path against
//! the [`RankedTextIndex`](sbol_db_search::ranked_text::RankedTextIndex) the
//! facade owns, enforcing the caller's [`GraphScope`] inside the index and
//! narrowing the ranked hits by the `objectType` facet. Purely faceted queries
//! (no free text) are answered by the SPARQL engine in the adapter, so this
//! verb is only the relevance surface.
//!
//! [`AppServices::discover`] is the native product contract. It combines the
//! same ranking with backend-neutral SPARQL facets, exact totals, deterministic
//! sorting, and stable offset paging. HTTP adapters only translate wire values
//! into [`DiscoveryQuery`]; they do not own discovery semantics.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use sbol_db_core::{DomainError, GraphId, IriString, SbolObjectRecord};
use sbol_db_rdf::GRAPH_IRI_PREFIX;
use sbol_db_search::ranked_text::{cluster_map, GraphFilter, Hit, RankedTextIndex};
use sbol_db_search::SearchRuntime;
use sbol_db_search_sdk::{
    DataEgress, DocumentId, FilterCapability, FilterKind, HydratedDocument, PaginationCapability,
    ScopedDocumentHydrator, ScoreKind, SearchContext, SearchError, SearchHit, SearchInput,
    SearchInputKind, SearchPage, SearchRequest, SearchScope, SearchStrategy, StrategyCapabilities,
    StrategyDescriptor, StrategyRef, StrategyRegistry, StrategyRequirements, Total,
    TotalCapability,
};
use sbol_db_sparql::{GraphScope, ResultFormat, SparqlOptions};
use sbol_db_storage::{ClusterStore, ListObjectsFilter, SbolStore};
use serde::{Deserialize, Serialize};

use crate::AppServices;

/// The classic SynBioHub default page size when a request names no `limit`.
const DEFAULT_LIMIT: usize = 50;

/// The ranked candidate pool pulled from the index before the offset/limit
/// window is taken. Matches the index's own fetch cap so a facet filter never
/// silently drops in-scope hits below the window.
const RANKED_FETCH: usize = 10_000;

const DISCOVERY_PREFIXES: &str = r#"
PREFIX dcterms: <http://purl.org/dc/terms/>
PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
PREFIX sbol2: <http://sbols.org/v2#>
PREFIX sbol3: <http://sbols.org/v3#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
"#;

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

/// Stable native discovery order. Every non-IRI sort uses the object IRI as a
/// final ascending tie-breaker so adjacent pages cannot reshuffle equal rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySort {
    #[default]
    Relevance,
    Name,
    Created,
    Modified,
    Iri,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Asc,
    #[default]
    Desc,
}

/// Backend-neutral native registry discovery. Values that name resources are
/// full IRIs; HTTP adapters validate and translate convenient wire aliases
/// before constructing this value.
#[derive(Clone, Debug)]
pub struct DiscoveryQuery {
    pub text: Option<String>,
    pub object_type: Option<String>,
    pub role: Option<String>,
    pub collection: Option<String>,
    pub owner: Option<String>,
    /// Case-insensitive substring over `sbh:mutableProvenance`.
    pub provenance: Option<String>,
    pub created_after: Option<NaiveDate>,
    pub created_before: Option<NaiveDate>,
    pub modified_after: Option<NaiveDate>,
    pub modified_before: Option<NaiveDate>,
    pub sort: DiscoverySort,
    pub direction: SortDirection,
    pub offset: usize,
    pub limit: usize,
}

impl Default for DiscoveryQuery {
    fn default() -> Self {
        Self {
            text: None,
            object_type: None,
            role: None,
            collection: None,
            owner: None,
            provenance: None,
            created_after: None,
            created_before: None,
            modified_after: None,
            modified_before: None,
            sort: DiscoverySort::Relevance,
            direction: SortDirection::Desc,
            offset: 0,
            limit: DEFAULT_LIMIT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DiscoveryHit {
    pub uri: String,
    pub display_id: Option<String>,
    pub version: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub object_type: Option<String>,
    pub roles: Vec<String>,
    pub owners: Vec<String>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DiscoveryPage {
    pub items: Vec<DiscoveryHit>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub sort: DiscoverySort,
    pub direction: SortDirection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscoveryFacetValue {
    pub iri: String,
    pub label: String,
    pub curie: Option<String>,
    pub count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DiscoveryFacets {
    pub types: Vec<DiscoveryFacetValue>,
    pub roles: Vec<DiscoveryFacetValue>,
}

#[derive(Clone, Debug, Default)]
struct DiscoveryMetadata {
    display_id: Option<String>,
    version: Option<String>,
    name: Option<String>,
    description: Option<String>,
    types: BTreeSet<String>,
    roles: BTreeSet<String>,
    owners: BTreeSet<String>,
    created_at: Option<String>,
    modified_at: Option<String>,
    compatibility_top_level: bool,
}

/// Map the caller's authorized [`GraphScope`] onto the index's graph filter so
/// the scope is enforced in the query rather than by post-filtering.
fn graph_filter(scope: &GraphScope) -> GraphFilter {
    match scope {
        GraphScope::Union => GraphFilter::Any,
        GraphScope::Only(graphs) => GraphFilter::Only(graphs.clone()),
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
                        .type_iris
                        .iter()
                        .any(|type_iri| object_types.contains(type_iri.as_str()))
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
        object_types: hit.type_iris,
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

    /// Execute the normalized, ACL-scoped registry discovery contract.
    ///
    /// SPARQL supplies the authoritative facet membership and metadata for
    /// every top-level object, including verbatim compatibility submissions
    /// that have no typed derived record. Tantivy supplies text matching and
    /// relevance scores. The application layer intersects them before sorting
    /// and paging so every HTTP presentation observes one semantic result set.
    pub async fn discover(
        &self,
        query: &DiscoveryQuery,
        scope: GraphScope,
    ) -> Result<DiscoveryPage, DomainError> {
        validate_discovery_query(query)?;
        let metadata = self.discovery_metadata(query, scope.clone()).await?;
        let text = query
            .text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let needs_ranked = text.is_some() || query.sort == DiscoverySort::Relevance;

        let mut ranked_by_iri: HashMap<String, Hit> = HashMap::new();
        if needs_ranked {
            let filter = graph_filter(&scope);
            let clusters = cluster_map(self.cluster.all_assignments().await?);
            let ranked = self
                .text_search
                .search_all(text.unwrap_or_default(), &filter, &clusters)
                .map_err(|error| {
                    DomainError::Database(format!("native discovery ranking: {error}"))
                })?;
            ranked_by_iri.extend(ranked.into_iter().map(|hit| (hit.subject.clone(), hit)));
        }

        let mut items = Vec::with_capacity(metadata.len());
        for (uri, meta) in metadata {
            let ranked = ranked_by_iri.remove(&uri);
            if text.is_some() && ranked.is_none() {
                continue;
            }
            let score = ranked.as_ref().map(|hit| hit.score).unwrap_or_default();
            let display_id = meta
                .display_id
                .or_else(|| ranked.as_ref().and_then(|hit| hit.display_id.clone()));
            let version = meta
                .version
                .or_else(|| ranked.as_ref().and_then(|hit| hit.version.clone()));
            let name = meta
                .name
                .or_else(|| ranked.as_ref().and_then(|hit| hit.name.clone()));
            let description = meta
                .description
                .or_else(|| ranked.as_ref().and_then(|hit| hit.description.clone()));
            let object_type = query
                .object_type
                .clone()
                .or_else(|| primary_type(&meta.types))
                .or_else(|| ranked.as_ref().and_then(|hit| hit.type_iri.clone()));
            items.push(DiscoveryHit {
                uri,
                display_id,
                version,
                name,
                description,
                object_type,
                roles: meta.roles.into_iter().collect(),
                owners: meta.owners.into_iter().collect(),
                created_at: meta.created_at,
                modified_at: meta.modified_at,
                score,
            });
        }

        sort_discovery_hits(&mut items, query.sort, query.direction);
        let total = items.len();
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(DiscoveryPage {
            items,
            total,
            offset: query.offset,
            limit: query.limit,
            sort: query.sort,
            direction: query.direction,
        })
    }

    /// Distinct in-scope object types and roles with exact subject counts.
    /// Loaded ontology metadata supplies human labels when available; unknown
    /// imported IRIs retain a deterministic compact fallback label.
    pub async fn discovery_facets(
        &self,
        scope: GraphScope,
    ) -> Result<DiscoveryFacets, DomainError> {
        let metadata = self
            .discovery_metadata(&DiscoveryQuery::default(), scope)
            .await?;
        let mut type_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut role_counts: BTreeMap<String, usize> = BTreeMap::new();
        for meta in metadata.values() {
            for object_type in &meta.types {
                *type_counts.entry(object_type.clone()).or_default() += 1;
            }
            for role in &meta.roles {
                *role_counts.entry(role.clone()).or_default() += 1;
            }
        }
        let types = self.facet_values(type_counts).await?;
        let roles = self.facet_values(role_counts).await?;
        Ok(DiscoveryFacets { types, roles })
    }

    async fn discovery_metadata(
        &self,
        query: &DiscoveryQuery,
        scope: GraphScope,
    ) -> Result<BTreeMap<String, DiscoveryMetadata>, DomainError> {
        let native = self.visible_native_objects(&scope).await?;
        let sparql = discovery_metadata_query(query)?;
        // Native imports keep their RDF in stable internal graph IRIs while
        // ACLs are expressed in terms of the graph's public document IRI.
        // Expand only the already-authorized native graphs for this internal
        // query; compatibility graphs retain their external names.
        let sparql_scope = match scope {
            GraphScope::Union => GraphScope::Union,
            GraphScope::Only(mut graphs) => {
                graphs.extend(
                    native
                        .values()
                        .filter_map(|record| record.graph_id)
                        .map(|graph_id| format!("{GRAPH_IRI_PREFIX}{graph_id}")),
                );
                graphs.sort();
                graphs.dedup();
                GraphScope::Only(graphs)
            }
        };
        let value = self
            .discovery_sparql_json(&sparql, sparql_scope, 1_000_000)
            .await?;
        let mut metadata = parse_discovery_metadata(&value)?;
        metadata.retain(|uri, meta| meta.compatibility_top_level || native.contains_key(uri));
        for (uri, record) in native {
            let Some(meta) = metadata.get_mut(&uri) else {
                // The SPARQL query owns the active facet criteria. A native
                // object absent from its results did not satisfy them.
                continue;
            };
            merge_native_metadata(meta, record);
        }
        Ok(metadata)
    }

    async fn visible_native_objects(
        &self,
        scope: &GraphScope,
    ) -> Result<BTreeMap<String, SbolObjectRecord>, DomainError> {
        const PAGE_SIZE: u32 = 5_000;
        let allowed = match scope {
            GraphScope::Union => None,
            GraphScope::Only(graphs) => Some(graphs.iter().cloned().collect::<HashSet<_>>()),
        };
        let mut graph_allowed: HashMap<GraphId, bool> = HashMap::new();
        let mut objects = BTreeMap::new();
        let mut after_iri = None;
        loop {
            let page = self
                .store
                .list_objects(&ListObjectsFilter {
                    after_iri: after_iri.clone(),
                    limit: PAGE_SIZE,
                    ..ListObjectsFilter::default()
                })
                .await?;
            if page.is_empty() {
                break;
            }
            let page_len = page.len();
            after_iri = page.last().map(|record| record.iri.as_str().to_owned());
            for record in page {
                let in_scope = match (&allowed, record.graph_id) {
                    (None, _) => true,
                    (Some(_), None) => false,
                    (Some(allowed), Some(graph_id)) => {
                        if let Some(in_scope) = graph_allowed.get(&graph_id) {
                            *in_scope
                        } else {
                            let in_scope = self
                                .store
                                .get_graph(graph_id)
                                .await?
                                .and_then(|graph| graph.document_iri)
                                .is_some_and(|iri| allowed.contains(iri.as_str()));
                            graph_allowed.insert(graph_id, in_scope);
                            in_scope
                        }
                    }
                };
                if in_scope {
                    objects.insert(record.iri.as_str().to_owned(), record);
                }
            }
            if page_len < PAGE_SIZE as usize {
                break;
            }
        }
        Ok(objects)
    }

    async fn facet_values(
        &self,
        counts: BTreeMap<String, usize>,
    ) -> Result<Vec<DiscoveryFacetValue>, DomainError> {
        let mut values = Vec::with_capacity(counts.len());
        for (iri, count) in counts {
            let term = self.store.get_ontology_term(&iri).await?;
            values.push(DiscoveryFacetValue {
                label: term
                    .as_ref()
                    .map(|term| term.name.clone())
                    .unwrap_or_else(|| compact_iri(&iri)),
                curie: term.map(|term| term.curie),
                iri,
                count,
            });
        }
        values.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
                .then_with(|| left.iri.cmp(&right.iri))
        });
        Ok(values)
    }

    async fn discovery_sparql_json(
        &self,
        query: &str,
        scope: GraphScope,
        max_rows: usize,
    ) -> Result<serde_json::Value, DomainError> {
        let options = SparqlOptions {
            max_rows,
            authorized_graphs: scope,
            ..SparqlOptions::default()
        };
        let outcome = self
            .sparql
            .execute(query, Some(ResultFormat::Json), None, &options)
            .await
            .map_err(DomainError::from)?;
        if outcome.payload.truncated {
            return Err(DomainError::Unavailable(format!(
                "discovery result exceeded its {max_rows}-row safety bound"
            )));
        }
        serde_json::from_slice(&outcome.payload.body).map_err(DomainError::from)
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
        let filter = graph_filter(&scope);
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
                Some(class) => hit.type_iris.iter().any(|value| value == class),
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

fn validate_discovery_query(query: &DiscoveryQuery) -> Result<(), DomainError> {
    if !(1..=1000).contains(&query.limit) {
        return Err(DomainError::InvalidInput(
            "discovery limit must be between 1 and 1000".to_owned(),
        ));
    }
    if query.text.as_ref().is_some_and(|value| value.len() > 4096) {
        return Err(DomainError::InvalidInput(
            "discovery text exceeds 4096 bytes".to_owned(),
        ));
    }
    if query
        .provenance
        .as_ref()
        .is_some_and(|value| value.len() > 1024)
    {
        return Err(DomainError::InvalidInput(
            "provenance filter exceeds 1024 bytes".to_owned(),
        ));
    }
    for value in [
        query.object_type.as_ref(),
        query.role.as_ref(),
        query.collection.as_ref(),
        query.owner.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        IriString::new(value.clone())?;
    }
    validate_date_range("created", query.created_after, query.created_before)?;
    validate_date_range("modified", query.modified_after, query.modified_before)?;
    Ok(())
}

fn validate_date_range(
    label: &str,
    after: Option<NaiveDate>,
    before: Option<NaiveDate>,
) -> Result<(), DomainError> {
    if matches!((after, before), (Some(after), Some(before)) if after > before) {
        return Err(DomainError::InvalidInput(format!(
            "{label}_after must not be later than {label}_before"
        )));
    }
    Ok(())
}

fn discovery_metadata_query(query: &DiscoveryQuery) -> Result<String, DomainError> {
    validate_discovery_query(query)?;
    let mut body = String::from(
        "SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type ?role ?owner \
         ?created ?modified ?compatTopLevel WHERE {\n\
           ?subject a ?type .\n",
    );

    if let Some(object_type) = &query.object_type {
        body.push_str(&format!("  ?subject a {} .\n", discovery_iri(object_type)?));
    }
    if let Some(role) = &query.role {
        let role = discovery_iri(role)?;
        body.push_str(&format!(
            "  {{ ?subject sbol2:role {role} . }} UNION \
             {{ ?subject sbol3:role {role} . }}\n"
        ));
    }
    if let Some(collection) = &query.collection {
        let collection = discovery_iri(collection)?;
        body.push_str(&format!(
            "  {{ {collection} sbol2:member ?subject . }} UNION \
             {{ {collection} sbol3:member ?subject . }}\n"
        ));
    }
    if let Some(owner) = &query.owner {
        body.push_str(&format!(
            "  ?subject sbh:ownedBy {} .\n",
            discovery_iri(owner)?
        ));
    }
    if let Some(provenance) = query
        .provenance
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.push_str(&format!(
            "  ?subject sbh:mutableProvenance ?matchedProvenance .\n  \
             FILTER(CONTAINS(LCASE(STR(?matchedProvenance)), LCASE({})))\n",
            discovery_literal(provenance)?
        ));
    }

    append_date_criteria(
        &mut body,
        "created",
        "dcterms:created",
        query.created_after,
        query.created_before,
    );
    append_date_criteria(
        &mut body,
        "modified",
        "dcterms:modified",
        query.modified_after,
        query.modified_before,
    );

    body.push_str(
        "  OPTIONAL { ?subject sbh:topLevel ?compatTopLevel . \
         FILTER(?compatTopLevel = ?subject) }\n\
           OPTIONAL { { ?subject sbol2:displayId ?displayId . } UNION \
         { ?subject sbol3:displayId ?displayId . } }\n\
           OPTIONAL { ?subject sbol2:version ?version . }\n\
           OPTIONAL { { ?subject dcterms:title ?name . } UNION \
         { ?subject sbol3:name ?name . } }\n\
           OPTIONAL { { ?subject dcterms:description ?description . } UNION \
         { ?subject sbol3:description ?description . } }\n\
           OPTIONAL { { ?subject sbol2:role ?role . } UNION \
         { ?subject sbol3:role ?role . } }\n\
           OPTIONAL { ?subject sbh:ownedBy ?owner . }\n",
    );
    if query.created_after.is_none() && query.created_before.is_none() {
        body.push_str("  OPTIONAL { ?subject dcterms:created ?created . }\n");
    }
    if query.modified_after.is_none() && query.modified_before.is_none() {
        body.push_str("  OPTIONAL { ?subject dcterms:modified ?modified . }\n");
    }
    body.push_str("}\nORDER BY ?subject");
    Ok(format!("{DISCOVERY_PREFIXES}\n{body}"))
}

fn append_date_criteria(
    body: &mut String,
    variable: &str,
    predicate: &str,
    after: Option<NaiveDate>,
    before: Option<NaiveDate>,
) {
    if after.is_none() && before.is_none() {
        return;
    }
    body.push_str(&format!("  ?subject {predicate} ?{variable} .\n"));
    if let Some(after) = after {
        body.push_str(&format!(
            "  FILTER(xsd:dateTime(?{variable}) >= \"{after}T00:00:00Z\"^^xsd:dateTime)\n"
        ));
    }
    if let Some(before) = before {
        body.push_str(&format!(
            "  FILTER(xsd:dateTime(?{variable}) <= \"{before}T23:59:59Z\"^^xsd:dateTime)\n"
        ));
    }
}

fn discovery_iri(value: &str) -> Result<String, DomainError> {
    let iri = IriString::new(value.to_owned())?;
    Ok(format!("<{}>", iri.as_str()))
}

fn discovery_literal(value: &str) -> Result<String, DomainError> {
    serde_json::to_string(value).map_err(DomainError::from)
}

fn parse_discovery_metadata(
    value: &serde_json::Value,
) -> Result<BTreeMap<String, DiscoveryMetadata>, DomainError> {
    let mut rows: BTreeMap<String, DiscoveryMetadata> = BTreeMap::new();
    for binding in solution_bindings(value)? {
        let Some(subject) = binding_value(binding, "subject") else {
            continue;
        };
        let metadata = rows.entry(subject.to_owned()).or_default();
        keep_smallest(
            &mut metadata.display_id,
            binding_value(binding, "displayId"),
        );
        keep_smallest(&mut metadata.version, binding_value(binding, "version"));
        keep_smallest(&mut metadata.name, binding_value(binding, "name"));
        keep_smallest(
            &mut metadata.description,
            binding_value(binding, "description"),
        );
        if let Some(value) = binding_value(binding, "type") {
            metadata.types.insert(value.to_owned());
        }
        if let Some(value) = binding_value(binding, "role") {
            metadata.roles.insert(value.to_owned());
        }
        if let Some(value) = binding_value(binding, "owner") {
            metadata.owners.insert(value.to_owned());
        }
        keep_smallest(&mut metadata.created_at, binding_value(binding, "created"));
        keep_largest(
            &mut metadata.modified_at,
            binding_value(binding, "modified"),
        );
        if binding_value(binding, "compatTopLevel") == Some(subject) {
            metadata.compatibility_top_level = true;
        }
    }
    Ok(rows)
}

fn merge_native_metadata(meta: &mut DiscoveryMetadata, record: SbolObjectRecord) {
    if meta.display_id.is_none() {
        meta.display_id = record.display_id;
    }
    if meta.name.is_none() {
        meta.name = record.name;
    }
    if meta.description.is_none() {
        meta.description = record.description;
    }
    meta.types.insert(record.sbol_class);
    meta.roles.extend(record.roles);
}

fn solution_bindings(value: &serde_json::Value) -> Result<&[serde_json::Value], DomainError> {
    value
        .get("results")
        .and_then(|results| results.get("bindings"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            DomainError::Serialization(
                "SPARQL discovery response has no results.bindings array".to_owned(),
            )
        })
}

fn binding_value<'a>(binding: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    binding
        .get(name)
        .and_then(|value| value.get("value"))
        .and_then(serde_json::Value::as_str)
}

fn keep_smallest(slot: &mut Option<String>, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    if slot.as_deref().is_none_or(|existing| value < existing) {
        *slot = Some(value.to_owned());
    }
}

fn keep_largest(slot: &mut Option<String>, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    if slot.as_deref().is_none_or(|existing| value > existing) {
        *slot = Some(value.to_owned());
    }
}

fn primary_type(types: &BTreeSet<String>) -> Option<String> {
    types
        .iter()
        .min_by_key(|value| (!value.starts_with("http://sbols.org/v"), value.as_str()))
        .cloned()
}

fn sort_discovery_hits(hits: &mut [DiscoveryHit], sort: DiscoverySort, direction: SortDirection) {
    hits.sort_by(|left, right| {
        let primary = match sort {
            DiscoverySort::Relevance => compare_f64(left.score, right.score, direction),
            DiscoverySort::Name => {
                let left_name = discovery_name(left).to_lowercase();
                let right_name = discovery_name(right).to_lowercase();
                apply_direction(left_name.cmp(&right_name), direction)
            }
            DiscoverySort::Created => compare_optional(
                left.created_at.as_deref(),
                right.created_at.as_deref(),
                direction,
            ),
            DiscoverySort::Modified => compare_optional(
                left.modified_at.as_deref(),
                right.modified_at.as_deref(),
                direction,
            ),
            DiscoverySort::Iri => apply_direction(left.uri.cmp(&right.uri), direction),
        };
        if sort == DiscoverySort::Iri {
            primary
        } else {
            primary.then_with(|| left.uri.cmp(&right.uri))
        }
    });
}

fn discovery_name(hit: &DiscoveryHit) -> &str {
    hit.name
        .as_deref()
        .or(hit.display_id.as_deref())
        .unwrap_or(&hit.uri)
}

fn compare_f64(left: f64, right: f64, direction: SortDirection) -> Ordering {
    let order = left.partial_cmp(&right).unwrap_or(Ordering::Equal);
    apply_direction(order, direction)
}

fn compare_optional(left: Option<&str>, right: Option<&str>, direction: SortDirection) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => apply_direction(left.cmp(right), direction),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn apply_direction(order: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Asc => order,
        SortDirection::Desc => order.reverse(),
    }
}

fn compact_iri(iri: &str) -> String {
    iri.rsplit(['#', '/'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(iri)
        .to_owned()
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
