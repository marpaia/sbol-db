//! `rebuild_search_index` job handler.
//!
//! Rebuilds the native ranked search index in SBOLExplorer's `update_index`
//! order: clusters, then PageRank, then the tantivy text index. The clusters
//! step is deferred (clustering is a later phase), so this handler runs the
//! PageRank and text-index steps and leaves the cluster-duplicate set empty; a
//! Sequence-typed hit is still penalized at search time, and the duplicate
//! penalty becomes live once clustering supplies a map.
//!
//! PageRank is computed by [`sbol_db_search::pagerank`] over the store's
//! triples and persisted atomically through [`PageRankStore`]. The text index
//! is then rebuilt from every top-level object's metadata joined with a
//! synthetic keyword field and its fresh PageRank score.
//!
//! The handler reaches the PageRank store, the shared text index, and the
//! triple source through the [`SearchIndexHandles`] on the job context rather
//! than through the [`SbolStore`] surface, so `tantivy` and the ranked-text
//! types never enter the storage traits.

use std::collections::HashMap;

use async_trait::async_trait;
use sbol_db_core::{ObjectTerm, SubjectTerm, Triple};
use sbol_db_search::keywords::{build_keywords, SoTerm};
use sbol_db_search::pagerank::{pagerank, top_level_iris, top_level_link_graph};
use sbol_db_search::ranked_text::IndexedPart;
use sbol_db_storage::RankRow;
use serde_json::Value;

use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};

pub const KIND: &str = "rebuild_search_index";

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
const VERSION: &str = "http://sbols.org/v2#version";
const SBOL_TYPE: &str = "http://sbols.org/v2#type";
const ROLE: &str = "http://sbols.org/v2#role";
const TITLE: &str = "http://purl.org/dc/terms/title";
const DESCRIPTION: &str = "http://purl.org/dc/terms/description";

/// The Sequence Ontology prefix under which role labels and synonyms are
/// looked up when building keywords.
const SO_PREFIX: &str = "so";

pub struct RebuildSearchIndexHandler;

#[async_trait]
impl JobHandler for RebuildSearchIndexHandler {
    /// The payload is empty; any JSON body is accepted and ignored so a bare
    /// `{}` enqueue is valid.
    type Payload = Value;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(&self, ctx: JobContext, _payload: Value) -> Result<JobOutcome, HandlerError> {
        let search = ctx.search.as_ref().ok_or_else(|| {
            HandlerError::Other(
                "rebuild_search_index requires a search index handle on the job context; \
                 this worker was built without one"
                    .to_owned(),
            )
        })?;

        // Clusters -> PageRank -> tantivy, with the clusters step deferred to a
        // later phase: the cluster-duplicate set is empty, so the divide-by-2
        // penalty stays inert while the Sequence divide-by-10 penalty is live.
        //
        // The triple source drives its async backend to completion synchronously,
        // so the full-store scan runs on a blocking thread rather than a runtime
        // worker.
        let triples = search.triples.clone();
        let all_triples = tokio::task::spawn_blocking(move || {
            triples.scan_pattern(None, None, None, None, i64::MAX)
        })
        .await
        .map_err(|e| HandlerError::Other(format!("triple scan task join: {e}")))??;

        let uris_set = top_level_iris(&all_triples);
        let uris: Vec<String> = uris_set.iter().cloned().collect();
        let edges = top_level_link_graph(&all_triples);
        let ranks = pagerank(&edges, &uris);

        let rank_rows: Vec<RankRow> = ranks
            .iter()
            .map(|(iri, score)| RankRow {
                iri: iri.clone(),
                score: *score,
            })
            .collect();
        let ranked = rank_rows.len();
        search.pagerank.replace_all_ranks(rank_rows).await?;

        let so_terms = load_so_terms(&ctx).await?;
        let mut metas = collect_object_metadata(&all_triples, &uris_set);

        let mut parts = Vec::with_capacity(uris.len());
        for uri in &uris {
            let meta = metas.remove(uri).unwrap_or_default();
            let keywords = build_keywords(
                meta.display_id.as_deref(),
                meta.role.as_deref(),
                meta.sbol_type.as_deref(),
                &so_terms,
            );
            // A URI absent from the map is unranked; the combine step reads it
            // as 1.0, SBOLExplorer's unknown-part convention.
            let rank = ranks.get(uri).copied().unwrap_or(1.0);
            parts.push(IndexedPart {
                subject: uri.clone(),
                graph: meta.graph.unwrap_or_default(),
                display_id: meta.display_id,
                name: meta.name,
                description: meta.description,
                version: meta.version,
                type_iris: meta.types,
                keywords,
                pagerank: rank,
            });
        }
        let indexed = parts.len();

        search
            .text_index
            .rebuild(parts)
            .map_err(|e| HandlerError::Other(format!("rebuilding ranked text index: {e}")))?;

        ctx.log(
            "info",
            "search index rebuilt",
            serde_json::json!({ "ranked": ranked, "indexed": indexed }),
        )
        .await;

        Ok(JobOutcome::with_result(
            serde_json::json!({ "ranked": ranked, "indexed": indexed }),
        ))
    }
}

/// The projected metadata for one top-level object, gathered from its triples.
#[derive(Default)]
struct ObjectMeta {
    graph: Option<String>,
    display_id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    types: Vec<String>,
    role: Option<String>,
    sbol_type: Option<String>,
}

/// Project each top-level object's searchable metadata out of the triple set,
/// mirroring SBOLExplorer's index projection (displayId, title, description,
/// version, rdf:type, SO role, biopax type).
fn collect_object_metadata(
    triples: &[Triple],
    top_levels: &std::collections::HashSet<String>,
) -> HashMap<String, ObjectMeta> {
    let mut metas: HashMap<String, ObjectMeta> = HashMap::new();
    for t in triples {
        let SubjectTerm::Iri(subject) = &t.subject else {
            continue;
        };
        let subject = subject.as_str();
        if !top_levels.contains(subject) {
            continue;
        }
        let meta = metas.entry(subject.to_owned()).or_default();
        if meta.graph.is_none() {
            if let Some(g) = &t.graph_iri {
                meta.graph = Some(g.as_str().to_owned());
            }
        }
        match t.predicate.as_str() {
            RDF_TYPE => {
                if let Some(v) = object_iri(&t.object) {
                    meta.types.push(v);
                }
            }
            DISPLAY_ID => set_first(&mut meta.display_id, object_literal(&t.object)),
            TITLE => set_first(&mut meta.name, object_literal(&t.object)),
            DESCRIPTION => set_first(&mut meta.description, object_literal(&t.object)),
            VERSION => set_first(&mut meta.version, object_literal(&t.object)),
            ROLE => set_first(&mut meta.role, object_iri(&t.object)),
            SBOL_TYPE => set_first(&mut meta.sbol_type, object_iri(&t.object)),
            _ => {}
        }
    }
    metas
}

/// Keep the first value seen for a single-valued field.
fn set_first(slot: &mut Option<String>, value: Option<String>) {
    if slot.is_none() {
        *slot = value;
    }
}

fn object_iri(object: &ObjectTerm) -> Option<String> {
    match object {
        ObjectTerm::Iri(iri) => Some(iri.as_str().to_owned()),
        _ => None,
    }
}

fn object_literal(object: &ObjectTerm) -> Option<String> {
    match object {
        ObjectTerm::Literal { value, .. } => Some(value.clone()),
        _ => None,
    }
}

/// Load the Sequence Ontology terms that resolve role labels and synonyms into
/// keywords. The store supplies them, so this crate stays free of an ontology
/// dependency; an instance with no SO ontology loaded yields an empty slice and
/// keywords fall back to the display id and biopax type.
async fn load_so_terms(ctx: &JobContext) -> Result<Vec<SoTerm>, HandlerError> {
    let mut terms = Vec::new();
    for ontology in ctx.service.list_ontologies().await? {
        if !ontology.prefix.eq_ignore_ascii_case(SO_PREFIX) {
            continue;
        }
        let (rows, _total) = ctx
            .service
            .list_ontology_terms(&ontology.prefix, i64::MAX, 0, None)
            .await?;
        for row in rows {
            terms.push(SoTerm {
                id: row.iri,
                label: row.name,
                synonyms: row.synonyms,
            });
        }
    }
    Ok(terms)
}
