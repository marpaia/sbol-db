//! SBOLExplorer compatibility policy. Native discovery keeps its own ranker.

use std::collections::{HashMap, HashSet};

use sbol_db_core::DomainError;
use sbol_db_search::ranked_text::GraphFilter;
use sbol_db_sparql::{ExplorerSearchQuery, GraphScope, Paging, ResultFormat, SparqlOptions};
use serde_json::Value;

use crate::{AppServices, Hit};

const MAX_STORE_ROWS: usize = 1_000_000;
const SEQUENCE: &str = "http://sbols.org/v2#Sequence";

pub struct ExplorerSearchHit {
    pub hit: Hit,
    pub role: Option<String>,
    pub sbol_type: Option<String>,
}

pub struct ExplorerSearchPage {
    pub hits: Vec<ExplorerSearchHit>,
    pub total: usize,
}

struct Candidate {
    result: ExplorerSearchHit,
    private: bool,
    ranked: bool,
}

impl AppServices {
    /// Apply SBOLExplorer issue #158: live private matching, ranked public text,
    /// and private-first merging before count/paging. Non-text criteria use live
    /// matching on both sides and put unranked private/public parts ahead of the
    /// combined ranked tail. The scope is supplied by the trusted listener.
    pub async fn explorer_search(
        &self,
        query: &ExplorerSearchQuery,
        paging: &Paging,
        scope: GraphScope,
    ) -> Result<ExplorerSearchPage, DomainError> {
        let GraphScope::Only(graphs) = scope else {
            return Err(DomainError::InvalidInput(
                "Explorer search requires an explicit graph scope".into(),
            ));
        };
        let (private, public): (Vec<_>, Vec<_>) = graphs
            .into_iter()
            .partition(|graph| graph.contains("/user/"));
        let browse = query.terms.is_empty() && !query.structural;
        let structural_only = query.terms.is_empty() && query.structural;
        let mut rows = if browse {
            self.explorer_index_rows(query, &private, true).await?
        } else {
            self.explorer_store_rows(&query.private_query, &private, true)
                .await?
        };
        rows.extend(if structural_only {
            self.explorer_store_rows(&query.public_query, &public, false)
                .await?
        } else {
            let mut hits = self.explorer_index_rows(query, &public, false).await?;
            if query.structural {
                let allowed: HashSet<_> = self
                    .explorer_store_rows(&query.public_query, &public, false)
                    .await?
                    .into_iter()
                    .map(|row| row.result.hit.subject)
                    .collect();
                hits.retain(|row| allowed.contains(&row.result.hit.subject));
            }
            hits
        });
        rows.sort_by(|left, right| {
            tier(left, structural_only)
                .cmp(&tier(right, structural_only))
                .then_with(|| right.result.hit.score.total_cmp(&left.result.hit.score))
                .then_with(|| left.result.hit.subject.cmp(&right.result.hit.subject))
        });
        // The same identity can occur in several authorized graphs or have
        // multiple RDF metadata rows. Prefer its private representation.
        let private_subjects: HashSet<_> = rows
            .iter()
            .filter(|row| row.private)
            .map(|row| row.result.hit.subject.clone())
            .collect();
        let mut seen = HashSet::new();
        rows.retain(|row| {
            (row.private || !private_subjects.contains(&row.result.hit.subject))
                && seen.insert(row.result.hit.subject.clone())
        });
        let total = rows.len();
        let hits = if paging.count {
            Vec::new()
        } else {
            rows.into_iter()
                .skip(paging.offset)
                .take(paging.limit)
                .map(|row| row.result)
                .collect()
        };
        Ok(ExplorerSearchPage { hits, total })
    }

    async fn explorer_index_rows(
        &self,
        query: &ExplorerSearchQuery,
        graphs: &[String],
        private: bool,
    ) -> Result<Vec<Candidate>, DomainError> {
        if graphs.is_empty() {
            return Ok(Vec::new());
        }
        let filter = GraphFilter::Only(graphs.to_vec());
        let text = query.terms.join(" ");
        let count = self
            .text_search
            .count(&text, &filter)
            .map_err(|error| DomainError::Database(error.to_string()))?;
        if count > MAX_STORE_ROWS {
            return Err(DomainError::Unavailable(
                "Explorer index result exceeds safety bound".into(),
            ));
        }
        let clusters = self.cached_cluster_map().await?;
        let hits = self
            .text_search
            .search_all(&text, &filter, &clusters)
            .map_err(|error| DomainError::Database(error.to_string()))?;
        Ok(hits
            .into_iter()
            .map(|hit| Candidate {
                result: ExplorerSearchHit {
                    hit,
                    role: None,
                    sbol_type: None,
                },
                private,
                ranked: true,
            })
            .collect())
    }

    async fn explorer_store_rows(
        &self,
        query: &str,
        graphs: &[String],
        private: bool,
    ) -> Result<Vec<Candidate>, DomainError> {
        if graphs.is_empty() {
            return Ok(Vec::new());
        }
        let options = SparqlOptions {
            authorized_graphs: GraphScope::Only(graphs.to_vec()),
            max_rows: MAX_STORE_ROWS,
            ..SparqlOptions::default()
        };
        let result = self
            .sparql
            .execute(query, Some(ResultFormat::Json), None, &options)
            .await?;
        if result.payload.truncated {
            return Err(DomainError::Unavailable(
                "Explorer store result exceeds safety bound".into(),
            ));
        }
        let value: Value = serde_json::from_slice(&result.payload.body)?;
        let bindings = value["results"]["bindings"]
            .as_array()
            .ok_or_else(|| DomainError::Serialization("missing Explorer store bindings".into()))?;
        let mut best: HashMap<String, &Value> = HashMap::new();
        for binding in bindings {
            if let Some(subject) = binding_value(binding, "subject") {
                best.entry(subject)
                    .and_modify(|previous| {
                        if preference(binding) > preference(previous) {
                            *previous = binding;
                        }
                    })
                    .or_insert(binding);
            }
        }
        let subjects: Vec<_> = best.keys().cloned().collect();
        let ranks = self.pagerank.ranks_for(&subjects).await?;
        Ok(best
            .into_iter()
            .map(|(subject, row)| {
                let rank = ranks.get(&subject).copied();
                let type_iri = binding_value(row, "type");
                let score = rank.unwrap_or(1.0)
                    / if type_iri.as_deref() == Some(SEQUENCE) {
                        10.0
                    } else {
                        1.0
                    };
                Candidate {
                    private,
                    ranked: rank.is_some(),
                    result: ExplorerSearchHit {
                        role: binding_value(row, "role"),
                        sbol_type: binding_value(row, "sbolType"),
                        hit: Hit {
                            subject,
                            display_id: binding_value(row, "displayId"),
                            version: binding_value(row, "version"),
                            name: binding_value(row, "name"),
                            description: binding_value(row, "description"),
                            type_iris: type_iri.iter().cloned().collect(),
                            type_iri,
                            score,
                        },
                    },
                }
            })
            .collect())
    }
}

fn binding_value(row: &Value, field: &str) -> Option<String> {
    row[field]["value"].as_str().map(str::to_owned)
}

fn preference(row: &Value) -> (bool, bool, String) {
    (
        row["role"]["value"]
            .as_str()
            .is_some_and(|role| role.starts_with("http://identifiers.org/so/")),
        row["sbolType"]["value"].as_str().is_some_and(|kind| {
            kind.starts_with("http://www.biopax.org/release/biopax-level3.owl#")
        }),
        row.to_string(),
    )
}

fn tier(row: &Candidate, structural_only: bool) -> u8 {
    if structural_only && row.ranked {
        2
    } else {
        u8::from(!row.private)
    }
}
