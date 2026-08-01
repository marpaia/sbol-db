//! `rebuild_search_index` job handler.
//!
//! Rebuilds the native ranked search index: the MinHash/LSH sketch index, then
//! clusters, then PageRank, then the tantivy text index. The sketch index leads
//! so the sequence-search align path can generate candidates from a fresh index;
//! clusters through tantivy then follow SBOLExplorer's `update_index` order.
//!
//! The clustering stage groups the top-level parts by their sequence elements
//! with [`sbol_db_search::cluster_sequences`] (greedy centroid clustering,
//! `vsearch --cluster_fast --id 0.8`) and persists the assignments atomically
//! through [`ClusterStore`]. Search then reads those assignments as a
//! [`ClusterMap`](sbol_db_search::ranked_text::cluster_map) and divides a hit
//! whose cluster mate already ranked ahead of it by 2. PageRank is computed by
//! [`sbol_db_search::pagerank`] over the store's triples and persisted through
//! [`PageRankStore`]. The text index is then rebuilt from every top-level
//! object's metadata joined with a synthetic keyword field and its fresh
//! PageRank score.
//!
//! The handler reaches the cluster store, the PageRank store, the shared text
//! index, and the triple source through the [`SearchIndexHandles`] on the job
//! context rather than through the [`SbolStore`] surface, so `tantivy`, the
//! aligner, and the ranked-text types never enter the storage traits.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use async_trait::async_trait;
use sbol_db_core::{GraphId, ObjectTerm, SubjectTerm, Triple};
use sbol_db_search::keywords::{build_keywords, SoTerm};
use sbol_db_search::pagerank::{link_graph_for_top_levels, pagerank, top_level_iris};
use sbol_db_search::ranked_text::IndexedPart;
use sbol_db_search::{
    band_hashes, cluster_sequences, sketch, AlignOptions, Signature, SketchParams,
};
use sbol_db_storage::{JobStatus, ListJobsFilter, ListObjectsFilter, RankRow, SbolJob};
use serde_json::Value;

use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};

pub const KIND: &str = "rebuild_search_index";

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
const DISPLAY_ID_V3: &str = "http://sbols.org/v3#displayId";
const VERSION: &str = "http://sbols.org/v2#version";
const SBOL_TYPE: &str = "http://sbols.org/v2#type";
const SBOL_TYPE_V3: &str = "http://sbols.org/v3#type";
const ROLE: &str = "http://sbols.org/v2#role";
const ROLE_V3: &str = "http://sbols.org/v3#role";
/// The `sbol2:sequence` predicate linking a part to its Sequence object.
const SEQUENCE: &str = "http://sbols.org/v2#sequence";
/// The equivalent SBOL 3 predicate.
const SEQUENCE_V3: &str = "http://sbols.org/v3#hasSequence";
/// The `sbol2:elements` predicate carrying a Sequence's nucleotide string.
const ELEMENTS: &str = "http://sbols.org/v2#elements";
/// The `sbol3:elements` predicate. The sketch and clustering stages read both
/// vocabularies so a verbatim SBOL 2 submission and an upgraded SBOL 3 import
/// populate the indexes identically, from the triples rather than the
/// version-specific derived view.
const ELEMENTS_V3: &str = "http://sbols.org/v3#elements";
const TITLE: &str = "http://purl.org/dc/terms/title";
const NAME_V3: &str = "http://sbols.org/v3#name";
const DESCRIPTION: &str = "http://purl.org/dc/terms/description";
const DESCRIPTION_V3: &str = "http://sbols.org/v3#description";
const SBH_TOP_LEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";
const INTERNAL_DOCUMENT_GRAPH_PREFIX: &str = "graph:document:";

/// The Sequence Ontology prefix under which role labels and synonyms are
/// looked up when building keywords.
const SO_PREFIX: &str = "so";

pub struct RebuildSearchIndexHandler;

/// Full compatibility-index rebuilds share process-local index handles and
/// must never run concurrently. The lock also gives a burst of queued rebuild
/// signals a deterministic coalescing point.
static REBUILD_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[async_trait]
impl JobHandler for RebuildSearchIndexHandler {
    /// The payload is empty; any JSON body is accepted and ignored so a bare
    /// `{}` enqueue is valid.
    type Payload = Value;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(&self, ctx: JobContext, _payload: Value) -> Result<JobOutcome, HandlerError> {
        let _rebuild_guard = REBUILD_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;

        // Every committed mutation enqueues a desired-state rebuild signal.
        // When a burst lands (for example, a full SBOLTestSuite seed), only the
        // newest queued/running signal performs the full scan. Older leased
        // jobs wait on the lock, observe the newer signal, and finish as
        // coalesced. A mutation that arrives during the final rebuild creates a
        // still-newer job, which runs afterward and therefore cannot be lost.
        if let Some(newer) = newest_pending_rebuild(&ctx).await? {
            ctx.log(
                "info",
                "search-index rebuild coalesced into newer job",
                serde_json::json!({ "newerJobId": newer.id }),
            )
            .await;
            return Ok(JobOutcome::with_result(serde_json::json!({
                "coalesced": true,
                "newerJobId": newer.id,
            })));
        }

        let search = ctx.search.as_ref().ok_or_else(|| {
            HandlerError::Other(
                "rebuild_search_index requires a search index handle on the job context; \
                 this worker was built without one"
                    .to_owned(),
            )
        })?;

        // Clusters -> PageRank -> tantivy, SBOLExplorer's `update_index` order.
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

        let native = native_objects(&ctx).await?;
        let mut uris_set = top_level_iris(&all_triples);
        uris_set.extend(native.iris.iter().cloned());
        let uris: Vec<String> = uris_set.iter().cloned().collect();

        // Sketch stage: sketch every DNA/RNA sequence and swap the whole
        // MinHash/LSH index in. Sequences come from the raw triples (any subject
        // carrying an `elements` literal, SBOL 2 or SBOL 3), keyed by the Sequence
        // IRI, so the persisted index covers every ingest path -- including a
        // verbatim SBOL 2 submission whose derived typed view is empty -- and its
        // keys match the align path's candidate lookups. A non-nucleotide sequence
        // yields no canonical k-mer and is left unsketched. Sketching is
        // CPU-bound, so it runs on a blocking thread.
        let sequence_elements = collect_all_sequences(&all_triples);
        let sketched_input = sequence_elements.len();
        let sketch_entries = tokio::task::spawn_blocking(move || build_sketches(sequence_elements))
            .await
            .map_err(|e| HandlerError::Other(format!("sketch task join: {e}")))?;
        let sketched = sketch_entries.len();
        search.sketch.replace_all_sketches(sketch_entries).await?;

        // Clustering stage: group each top-level part by its sequence elements
        // and persist the assignments. Alignment is CPU-bound, so it runs on a
        // blocking thread; the persisted assignments then drive the search-time
        // divide-by-2 duplicate penalty and answer `/similar`.
        let part_sequences = collect_part_sequences(&all_triples, &uris_set);
        let clustered_parts = part_sequences.len();
        let assignments = tokio::task::spawn_blocking(move || {
            cluster_sequences(part_sequences, &AlignOptions::default())
        })
        .await
        .map_err(|e| HandlerError::Other(format!("clustering task join: {e}")))?;
        let clusters = distinct_cluster_count(&assignments);
        search.cluster.replace_clusters(assignments).await?;

        let edges = link_graph_for_top_levels(&all_triples, &uris_set);
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
        // The physical graph of a native import is `graph:document:<uuid>`,
        // but callers are authorized against its public document IRI. Store
        // that public identity in the text index so ACL graph filters agree
        // with every other application read.
        for (iri, document_iri) in native.document_graphs {
            metas.entry(iri).or_default().graph = Some(document_iri);
        }

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
            serde_json::json!({
                "ranked": ranked,
                "indexed": indexed,
                "clustered_parts": clustered_parts,
                "clusters": clusters,
                "sketched": sketched,
                "sketched_input": sketched_input,
            }),
        )
        .await;

        Ok(JobOutcome::with_result(serde_json::json!({
            "ranked": ranked,
            "indexed": indexed,
            "clustered_parts": clustered_parts,
            "clusters": clusters,
            "sketched": sketched,
            "sketched_input": sketched_input,
        })))
    }
}

async fn newest_pending_rebuild(ctx: &JobContext) -> Result<Option<SbolJob>, HandlerError> {
    let Some(current) = ctx.jobs.get(ctx.job_id).await? else {
        // Direct handler tests and embedders may invoke the handler without a
        // persisted queue row. There is then nothing to coalesce.
        return Ok(None);
    };
    let mut candidates = Vec::new();
    for status in [JobStatus::Queued, JobStatus::Running] {
        candidates.extend(
            ctx.jobs
                .list(&ListJobsFilter {
                    kind: Some(KIND.to_owned()),
                    status: Some(status),
                    limit: 1,
                    ..ListJobsFilter::default()
                })
                .await?,
        );
    }
    let newest = candidates.into_iter().max_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
    });
    Ok(newest.filter(|job| {
        job.id != current.id
            && (job.created_at, job.id.to_string()) > (current.created_at, current.id.to_string())
    }))
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
            // A compatibility top-level marker identifies the externally
            // addressable graph that search ACLs use. The same subject may
            // also occur in a native physical `graph:document:*` graph; prefer
            // the explicit external marker regardless of scan order.
            SBH_TOP_LEVEL if matches!(&t.object, ObjectTerm::Iri(object) if object.as_str() == subject) => {
                if let Some(candidate) = t.graph_iri.as_ref().map(|graph| graph.as_str()) {
                    let should_replace = match meta.graph.as_deref() {
                        None => true,
                        Some(current) => {
                            current.starts_with(INTERNAL_DOCUMENT_GRAPH_PREFIX)
                                && !candidate.starts_with(INTERNAL_DOCUMENT_GRAPH_PREFIX)
                        }
                    };
                    if should_replace {
                        meta.graph = Some(candidate.to_owned());
                    }
                }
            }
            RDF_TYPE => {
                if let Some(v) = object_iri(&t.object) {
                    meta.types.push(v);
                }
            }
            DISPLAY_ID | DISPLAY_ID_V3 => {
                set_first(&mut meta.display_id, object_literal(&t.object))
            }
            TITLE | NAME_V3 => set_first(&mut meta.name, object_literal(&t.object)),
            DESCRIPTION | DESCRIPTION_V3 => {
                set_first(&mut meta.description, object_literal(&t.object))
            }
            VERSION => set_first(&mut meta.version, object_literal(&t.object)),
            ROLE | ROLE_V3 => set_first(&mut meta.role, object_iri(&t.object)),
            SBOL_TYPE | SBOL_TYPE_V3 => set_first(&mut meta.sbol_type, object_iri(&t.object)),
            _ => {}
        }
    }
    metas
}

/// Every `(sequence_iri, elements)` pair in the triple set: any subject carrying
/// an `elements` literal under the SBOL 2 or SBOL 3 vocabulary. This is the
/// version-agnostic source for the persisted sketch index, so a verbatim SBOL 2
/// graph (whose derived typed view is empty) is sketched identically to an
/// upgraded SBOL 3 import, and the keys are the Sequence IRIs the align path
/// looks up. A subject with more than one elements value keeps the first seen.
fn collect_all_sequences(triples: &[Triple]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for t in triples {
        let pred = t.predicate.as_str();
        if pred != ELEMENTS && pred != ELEMENTS_V3 {
            continue;
        }
        if let (SubjectTerm::Iri(seq), ObjectTerm::Literal { value, .. }) = (&t.subject, &t.object)
        {
            if seen.insert(seq.as_str()) {
                out.push((seq.as_str().to_owned(), value.as_str().to_owned()));
            }
        }
    }
    out
}

/// Gather the `(part_iri, elements)` pairs to cluster, mirroring SBOLExplorer's
/// clustering query: a top-level part linked to a Sequence whose `elements`
/// literal supplies the nucleotide string. The cluster is keyed by the part
/// IRI, so `/similar` on a part resolves its mates directly. A part with more
/// than one sequence keeps the first seen, and a part is included only when it
/// is top-level (the indexed subject the duplicate penalty ranks).
fn collect_part_sequences(
    triples: &[Triple],
    top_levels: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
    let mut elements: HashMap<&str, &str> = HashMap::new();
    for t in triples {
        let pred = t.predicate.as_str();
        if pred != ELEMENTS && pred != ELEMENTS_V3 {
            continue;
        }
        if let (SubjectTerm::Iri(seq), ObjectTerm::Literal { value, .. }) = (&t.subject, &t.object)
        {
            elements.entry(seq.as_str()).or_insert(value.as_str());
        }
    }

    let mut parts: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for t in triples {
        if !matches!(t.predicate.as_str(), SEQUENCE | SEQUENCE_V3) {
            continue;
        }
        let (SubjectTerm::Iri(part), ObjectTerm::Iri(seq)) = (&t.subject, &t.object) else {
            continue;
        };
        let part = part.as_str();
        if !top_levels.contains(part) || seen.contains(part) {
            continue;
        }
        if let Some(seq_elements) = elements.get(seq.as_str()) {
            seen.insert(part);
            parts.push((part.to_owned(), (*seq_elements).to_owned()));
        }
    }
    parts
}

/// Native imports persist their authoritative top-level projection in the
/// object view but do not add the classic `sbh:topLevel` annotation to user
/// RDF. Page through that view so the rebuilt indexes cover native and
/// compatibility ingest paths alike.
struct NativeObjects {
    iris: HashSet<String>,
    document_graphs: HashMap<String, String>,
}

async fn native_objects(ctx: &JobContext) -> Result<NativeObjects, HandlerError> {
    const PAGE_SIZE: u32 = 5_000;
    let mut iris = HashSet::new();
    let mut document_graphs = HashMap::new();
    let mut graph_documents: HashMap<GraphId, Option<String>> = HashMap::new();
    let mut after_iri = None;
    loop {
        let page = ctx
            .service
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
            let iri = record.iri.as_str().to_owned();
            iris.insert(iri.clone());
            let Some(graph_id) = record.graph_id else {
                continue;
            };
            let document_iri = if let Some(document_iri) = graph_documents.get(&graph_id) {
                document_iri.clone()
            } else {
                let document_iri = ctx
                    .service
                    .get_graph(graph_id)
                    .await?
                    .and_then(|graph| graph.document_iri)
                    .map(|iri| iri.into_inner());
                graph_documents.insert(graph_id, document_iri.clone());
                document_iri
            };
            if let Some(document_iri) = document_iri {
                document_graphs.insert(iri, document_iri);
            }
        }
        if page_len < PAGE_SIZE as usize {
            break;
        }
    }
    Ok(NativeObjects {
        iris,
        document_graphs,
    })
}

/// Sketch each `(iri, elements)` into `(iri, signature, band_hashes)`, dropping a
/// sequence that yields no canonical k-mer (too short, or non-nucleotide). The
/// entries feed [`SketchStore::replace_all_sketches`].
fn build_sketches(sequences: Vec<(String, String)>) -> Vec<(String, Signature, Vec<u64>)> {
    let params = SketchParams::default();
    let mut out = Vec::with_capacity(sequences.len());
    for (iri, elements) in sequences {
        if let Some(sig) = sketch(&elements, &params) {
            let bands = band_hashes(&sig, &params);
            out.push((iri, sig, bands));
        }
    }
    out
}

/// The number of distinct clusters among a set of assignments.
fn distinct_cluster_count(assignments: &[(String, sbol_db_search::ClusterId)]) -> usize {
    assignments
        .iter()
        .map(|(_, cluster)| *cluster)
        .collect::<std::collections::HashSet<_>>()
        .len()
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

#[cfg(test)]
mod metadata_tests {
    use sbol_db_core::IriString;

    use super::*;

    const SUBJECT: &str = "https://example.org/component";
    const PUBLIC: &str = "https://example.org/public";
    const COMPONENT: &str = "http://sbols.org/v3#Component";

    fn triple(graph: &str, predicate: &str, object: &str) -> Triple {
        Triple {
            graph_iri: Some(IriString::unchecked(graph)),
            subject: SubjectTerm::Iri(IriString::unchecked(SUBJECT)),
            predicate: IriString::unchecked(predicate),
            object: ObjectTerm::Iri(IriString::unchecked(object)),
        }
    }

    #[test]
    fn external_top_level_graph_wins_over_native_physical_graph() {
        let triples = vec![
            triple("graph:document:1234", RDF_TYPE, COMPONENT),
            triple(PUBLIC, SBH_TOP_LEVEL, SUBJECT),
            // A later physical marker must not make the result private again.
            triple("graph:document:1234", SBH_TOP_LEVEL, SUBJECT),
        ];
        let top_levels = HashSet::from([SUBJECT.to_owned()]);
        let metadata = collect_object_metadata(&triples, &top_levels);

        assert_eq!(
            metadata.get(SUBJECT).and_then(|meta| meta.graph.as_deref()),
            Some(PUBLIC)
        );
    }
}
