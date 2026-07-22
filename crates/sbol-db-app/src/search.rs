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

use sbol_db_core::DomainError;
use sbol_db_search::ranked_text::{cluster_map, GraphFilter, Hit};
use sbol_db_sparql::GraphScope;

use crate::AppServices;

/// The classic SynBioHub default page size when a request names no `limit`.
const DEFAULT_LIMIT: usize = 50;

/// The ranked candidate pool pulled from the index before the offset/limit
/// window is taken. Matches the index's own fetch cap so a facet filter never
/// silently drops in-scope hits below the window.
const RANKED_FETCH: usize = 10_000;

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

impl AppServices {
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
