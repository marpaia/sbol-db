//! Nucleotide sequence search and `/similar` over the derived view.
//!
//! [`SequenceService`] is the facade's sequence surface, replacing SBOLExplorer's
//! vsearch shell-out with a native pipeline:
//!
//! - `align` runs the k-mer prefilter on the store to gather candidate
//!   sequences, aligns each with the banded aligner
//!   ([`align_pair`](sbol_db_search::align_pair), rust-bio behind the `align`
//!   feature), keeps hits at or above the identity threshold, and orders them by
//!   `pagerank * percentMatch` (SBOLExplorer `create_criteria_bindings`). An
//!   [`AlignMode::Exact`] request short-circuits to the store's exact substring
//!   path (vsearch `--search_exact`).
//! - `similar` returns the other members of the target's cluster ranked by
//!   PageRank alone, carrying no `percentMatch`/CIGAR (SBOLExplorer
//!   `create_similar_criteria`).
//!
//! Both surfaces enforce the caller's [`GraphScope`]: a hit whose object lies
//! outside the authorized graphs is dropped before it reaches the caller, so a
//! private part never leaks through a sequence or `/similar` read.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use sbol_db_core::DomainError;
use sbol_db_search::{
    align_pair, band_hashes, sketch, AlignMode, AlignOptions, ClusterId, SketchParams,
};
use sbol_db_sparql::GraphScope;
use sbol_db_storage::{
    ClusterStore, PageRankStore, SbolStore, SequenceAlignment, SequenceSearchOptions, SketchStore,
};

/// The rank a part carries when it has no stored PageRank score, SBOLExplorer's
/// unknown-part convention (`uri2rank.get(subject, 1)`).
const DEFAULT_RANK: f64 = 1.0;

/// One `/similar` result: a cluster mate and the PageRank score it is ordered
/// by. Unlike a sequence-search hit it carries no `percentMatch`/CIGAR, matching
/// SBOLExplorer's cluster-mate contract.
#[derive(Clone, Debug, PartialEq)]
pub struct SimilarHit {
    pub iri: String,
    pub pagerank: f64,
}

/// The sequence-search and `/similar` facade over the SBOL store, the PageRank
/// scores, and the cluster assignments. Cheap to construct (three `Arc`
/// clones); [`AppServices::sequence`](crate::AppServices::sequence) builds one
/// per call.
#[derive(Clone)]
pub struct SequenceService {
    store: Arc<dyn SbolStore>,
    pagerank: Arc<dyn PageRankStore>,
    cluster: Arc<dyn ClusterStore>,
    sketch: Arc<dyn SketchStore>,
}

impl SequenceService {
    pub fn new(
        store: Arc<dyn SbolStore>,
        pagerank: Arc<dyn PageRankStore>,
        cluster: Arc<dyn ClusterStore>,
        sketch: Arc<dyn SketchStore>,
    ) -> Self {
        Self {
            store,
            pagerank,
            cluster,
            sketch,
        }
    }

    /// Align `query` against the indexed sequences and return the in-scope hits
    /// ordered by `pagerank * percentMatch`, descending (IRI as a deterministic
    /// tie-break), capped at `options.max_accepts`. [`AlignMode::Exact`] takes
    /// the store's exact substring path; every other mode runs the banded
    /// aligner over the k-mer prefilter's candidates.
    pub async fn align(
        &self,
        query: &str,
        options: AlignOptions,
        scope: &GraphScope,
    ) -> Result<Vec<SequenceAlignment>, DomainError> {
        let mut hits = match options.mode {
            AlignMode::Exact => self.exact(query).await?,
            AlignMode::Substring | AlignMode::GlobalAlign => self.global(query, &options).await?,
        };
        hits = self.scope_filter_alignments(hits, scope).await?;
        self.rank_alignments(&mut hits).await?;
        hits.truncate(options.max_accepts as usize);
        Ok(hits)
    }

    /// The other members of `iri`'s cluster, in scope, ranked by PageRank
    /// (descending, IRI tie-break). Empty when `iri` is unclustered or the sole
    /// member, or when every mate is out of scope.
    pub async fn similar(
        &self,
        iri: &str,
        scope: &GraphScope,
    ) -> Result<Vec<SimilarHit>, DomainError> {
        let mates = self.cluster.cluster_mates(iri).await?;
        let mates = self.scope_filter_iris(mates, scope).await?;
        let ranks = self.pagerank.ranks_for(&mates).await?;
        let mut hits: Vec<SimilarHit> = mates
            .into_iter()
            .map(|iri| {
                let pagerank = ranks.get(&iri).copied().unwrap_or(DEFAULT_RANK);
                SimilarHit { iri, pagerank }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.pagerank
                .partial_cmp(&a.pagerank)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.iri.cmp(&b.iri))
        });
        Ok(hits)
    }

    /// The number of in-scope cluster mates of `iri`, the `/similarCount` value.
    pub async fn similar_count(&self, iri: &str, scope: &GraphScope) -> Result<usize, DomainError> {
        let mates = self.cluster.cluster_mates(iri).await?;
        Ok(self.scope_filter_iris(mates, scope).await?.len())
    }

    /// The exact substring path: reuse the store's reverse-complement-aware
    /// substring search and present each matched sequence once at full identity.
    async fn exact(&self, query: &str) -> Result<Vec<SequenceAlignment>, DomainError> {
        let matches = self
            .store
            .search(query, SequenceSearchOptions::default())
            .await?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for m in matches {
            if seen.insert(m.sequence_iri.clone()) {
                let len = m.length.max(0);
                out.push(SequenceAlignment {
                    sequence_iri: m.sequence_iri,
                    percent_match: 1.0,
                    strand: m.strand,
                    cigar: format!("{len}M"),
                    score: len,
                });
            }
        }
        Ok(out)
    }

    /// The banded-align path: draw candidates from the MinHash/LSH sketch index
    /// and align each, keeping only those the aligner accepts (identity at or
    /// above the threshold within the sequence-length window).
    ///
    /// The query is sketched and its LSH band hashes drive
    /// [`SketchStore::candidates_by_bands`], so only sequences sharing a band
    /// (high k-mer Jaccard, either strand since the k-mers are canonical) are
    /// aligned: candidate work is bounded by neighborhood size, not by the whole
    /// corpus. A query too short to sketch has no bands, so it falls back to the
    /// k-mer prefilter (which itself scans the whole corpus for a sub-k-mer
    /// query), preserving recall for degenerate probes.
    async fn global(
        &self,
        query: &str,
        options: &AlignOptions,
    ) -> Result<Vec<SequenceAlignment>, DomainError> {
        let candidates = self.align_candidates(query).await?;
        let mut out = Vec::new();
        for (iri, elements) in candidates {
            if let Some(aln) = align_pair(query, &elements, options) {
                out.push(SequenceAlignment {
                    sequence_iri: iri,
                    percent_match: aln.percent_match,
                    strand: aln.strand,
                    cigar: aln.cigar,
                    score: aln.score,
                });
            }
        }
        Ok(out)
    }

    /// The `(sequence_iri, elements)` candidates the banded aligner verifies for
    /// `query`. Sketches the query and unions its LSH band buckets, then
    /// materializes those candidates' elements; a query too short to sketch falls
    /// back to the store's k-mer prefilter.
    async fn align_candidates(&self, query: &str) -> Result<Vec<(String, String)>, DomainError> {
        match sketch(query, &SketchParams::default()) {
            Some(sig) => {
                let bands = band_hashes(&sig, &SketchParams::default());
                let iris = self.sketch.candidates_by_bands(&bands).await?;
                self.store.sequences_by_iris(&iris).await
            }
            None => self.store.align_candidates(query).await,
        }
    }

    /// Assign one just-written sequence to a cluster and index its sketch,
    /// incrementally, without a full rebuild. Computes the sequence's MinHash
    /// sketch, unions its LSH band buckets to find the near neighbors already
    /// indexed, aligns against them, and joins the first neighbor's cluster at or
    /// above the identity threshold; if none accepts it opens a fresh cluster one
    /// past the largest in use. The sketch is then persisted so later sequences
    /// can find this one. A sequence too short to sketch draws no candidates and
    /// opens its own cluster. Returns the assigned [`ClusterId`].
    ///
    /// This is the derived-view write path's counterpart to the bulk rebuild: a
    /// single new sequence updates the materialized sketch and cluster indexes in
    /// place, so `/similar` and the align path see it immediately without
    /// reclustering the corpus.
    pub async fn assign_new_sequence(
        &self,
        sequence_iri: &str,
        elements: &str,
    ) -> Result<ClusterId, DomainError> {
        let params = SketchParams::default();
        let Some(sig) = sketch(elements, &params) else {
            // Too short to sketch: open its own cluster, no sketch to persist.
            let cluster = self.next_cluster_id().await?;
            self.cluster.assign_cluster(sequence_iri, cluster).await?;
            return Ok(cluster);
        };
        let bands = band_hashes(&sig, &params);

        // Candidate neighbors sharing a band, excluding the sequence itself.
        let candidate_iris: Vec<String> = self
            .sketch
            .candidates_by_bands(&bands)
            .await?
            .into_iter()
            .filter(|iri| iri != sequence_iri)
            .collect();
        let candidates = self.store.sequences_by_iris(&candidate_iris).await?;

        let pair_opts = AlignOptions {
            mode: AlignMode::GlobalAlign,
            min_identity: AlignOptions::default().min_identity,
            max_accepts: 1,
            min_seqlen: 0,
            max_seqlen: u32::MAX,
        };

        let mut assigned: Option<ClusterId> = None;
        for (iri, cand_elements) in &candidates {
            if align_pair(elements, cand_elements, &pair_opts).is_some() {
                if let Some(cluster) = self.cluster.cluster_id_of(iri).await? {
                    assigned = Some(cluster);
                    break;
                }
            }
        }

        let cluster = match assigned {
            Some(cluster) => cluster,
            None => self.next_cluster_id().await?,
        };
        self.sketch.put_sketch(sequence_iri, &sig, &bands).await?;
        self.cluster.assign_cluster(sequence_iri, cluster).await?;
        Ok(cluster)
    }

    /// The id for a fresh cluster: one past the largest in use, or 0 when none.
    async fn next_cluster_id(&self) -> Result<ClusterId, DomainError> {
        Ok(match self.cluster.max_cluster_id().await? {
            Some(ClusterId(max)) => ClusterId(max + 1),
            None => ClusterId(0),
        })
    }

    /// Sort alignments by `pagerank * percentMatch` descending, with the IRI as
    /// a deterministic tie-break.
    async fn rank_alignments(&self, hits: &mut [SequenceAlignment]) -> Result<(), DomainError> {
        let iris: Vec<String> = hits.iter().map(|h| h.sequence_iri.clone()).collect();
        let ranks = self.pagerank.ranks_for(&iris).await?;
        hits.sort_by(|a, b| {
            let ra = ranks.get(&a.sequence_iri).copied().unwrap_or(DEFAULT_RANK) * a.percent_match;
            let rb = ranks.get(&b.sequence_iri).copied().unwrap_or(DEFAULT_RANK) * b.percent_match;
            rb.partial_cmp(&ra)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.sequence_iri.cmp(&b.sequence_iri))
        });
        Ok(())
    }

    async fn scope_filter_alignments(
        &self,
        hits: Vec<SequenceAlignment>,
        scope: &GraphScope,
    ) -> Result<Vec<SequenceAlignment>, DomainError> {
        let GraphScope::Only(graphs) = scope else {
            return Ok(hits);
        };
        let allowed: HashSet<&str> = graphs.iter().map(String::as_str).collect();
        let mut out = Vec::with_capacity(hits.len());
        for hit in hits {
            if self.graph_allowed(&hit.sequence_iri, &allowed).await? {
                out.push(hit);
            }
        }
        Ok(out)
    }

    async fn scope_filter_iris(
        &self,
        iris: Vec<String>,
        scope: &GraphScope,
    ) -> Result<Vec<String>, DomainError> {
        let GraphScope::Only(graphs) = scope else {
            return Ok(iris);
        };
        let allowed: HashSet<&str> = graphs.iter().map(String::as_str).collect();
        let mut out = Vec::with_capacity(iris.len());
        for iri in iris {
            if self.graph_allowed(&iri, &allowed).await? {
                out.push(iri);
            }
        }
        Ok(out)
    }

    /// Whether `iri`'s object lies in one of the `allowed` graphs. The graph is
    /// read straight from the object's triples (the named graph they carry),
    /// mirroring [`AclService::graph_of_subject`](crate::AclService), so it
    /// resolves for every write path: a submission's private graph and the
    /// shared public graph a `makePublic` writes verbatim alike. An object that
    /// appears in no named graph is treated as out of scope, so an unresolvable
    /// hit never leaks past a scoped read.
    async fn graph_allowed(&self, iri: &str, allowed: &HashSet<&str>) -> Result<bool, DomainError> {
        Ok(self
            .store
            .triples_for_subject(iri)
            .await?
            .into_iter()
            .find_map(|t| t.graph_iri)
            .map(|g| allowed.contains(g.as_str()))
            .unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use sbol_db_backend::Backend;
    use sbol_db_core::SerializationFormat;
    use sbol_db_sparql::GraphScope;
    use sbol_db_storage::{ClusterId, ImportInput, ImportOverwrite, ListObjectsFilter, RankRow};
    use tempfile::TempDir;

    use crate::{AlignMode, AlignOptions, AppServices};

    /// An SBOL3 document with one Component and its DNA Sequence, whose elements
    /// the exact-search test queries verbatim.
    const ELEMENTS: &str = "ttgacagctagctcagtcctaggtataatgctagc";
    const DOC: &str = r#"
BASE <https://example.org/sbol-db/seq-test/>
PREFIX :     <https://example.org/sbol-db/seq-test/>
PREFIX SO:   <https://identifiers.org/SO:>
PREFIX EDAM: <https://identifiers.org/edam:>
PREFIX sbol: <http://sbols.org/v3#>

:promoter
    a                  sbol:Component ;
    sbol:displayId     "promoter" ;
    sbol:hasNamespace  <https://example.org/sbol-db/seq-test> ;
    sbol:role          SO:0000167 ;
    sbol:hasSequence   :promoter_seq .

:promoter_seq
    a                  sbol:Sequence ;
    sbol:displayId     "promoter_seq" ;
    sbol:hasNamespace  <https://example.org/sbol-db/seq-test> ;
    sbol:elements      "ttgacagctagctcagtcctaggtataatgctagc" ;
    sbol:encoding      EDAM:format_1207 .
"#;

    async fn sqlite_facade() -> (AppServices, TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let url = format!("sqlite://{}", dir.path().join("seq.db").display());
        let backend = Backend::open(&url).await.expect("open sqlite backend");
        backend
            .migrator
            .as_ref()
            .expect("migrator")
            .run_migrations()
            .await
            .expect("run migrations");
        (AppServices::from_backend(&backend), dir)
    }

    async fn import_sequence(app: &AppServices) -> String {
        let report = app
            .store
            .import_document(ImportInput {
                body: DOC.to_owned(),
                format: SerializationFormat::Turtle,
                namespace: None,
                source_uri: Some("seq-test://doc".to_owned()),
                document_iri: None,
                created_by: None,
                name: None,
                description: None,
                overwrite: ImportOverwrite::Fail,
            })
            .await
            .expect("import_document");
        let objects = app
            .store
            .list_objects(&ListObjectsFilter {
                sbol_class: None,
                role: None,
                graph_id: Some(report.graph_id),
                after_iri: None,
                limit: 100,
            })
            .await
            .expect("list_objects");
        objects
            .iter()
            .find(|o| o.sbol_class.ends_with("#Sequence"))
            .expect("a Sequence object")
            .iri
            .as_str()
            .to_owned()
    }

    #[tokio::test]
    async fn exact_search_returns_the_seeded_sequence_at_full_identity() {
        let (app, _dir) = sqlite_facade().await;
        let sequence_iri = import_sequence(&app).await;

        let opts = AlignOptions {
            mode: AlignMode::Exact,
            ..AlignOptions::default()
        };
        let hits = app
            .sequence()
            .align(&ELEMENTS.to_uppercase(), opts, &GraphScope::Union)
            .await
            .expect("align");

        let hit = hits
            .iter()
            .find(|h| h.sequence_iri == sequence_iri)
            .expect("the seeded sequence is a hit");
        assert_eq!(hit.percent_match, 1.0, "an exact match is full identity");
        assert_eq!(hit.cigar, format!("{}M", ELEMENTS.len()));
    }

    #[tokio::test]
    async fn global_align_finds_a_near_identical_variant() {
        let (app, _dir) = sqlite_facade().await;
        let seq = app.sequence();

        // The align path draws candidates from the MinHash/LSH sketch index, so
        // the target must be sketched (real parts run hundreds of bases; a point
        // mutation then keeps the k-mer Jaccard high enough to collide) and its
        // sketch must be indexed, which the incremental write path does.
        let target = rand_seq(101, 400);
        let sequence_iri = import_labeled_sequence(&app, "galign", &target).await;
        seq.assign_new_sequence(&sequence_iri, &target)
            .await
            .expect("index the target sketch");

        // Flip one base: a ~0.997-identity variant the banded aligner accepts
        // above the 0.8 floor, reached through the sketch candidate query.
        let variant = mutate(&target, 9, 1);

        let hits = seq
            .align(&variant, AlignOptions::default(), &GraphScope::Union)
            .await
            .expect("align");
        let hit = hits
            .iter()
            .find(|h| h.sequence_iri == sequence_iri)
            .expect("the near-identical sequence is a hit");
        assert!(
            hit.percent_match >= 0.9 && hit.percent_match < 1.0,
            "a single-mismatch variant is high but not full identity: {}",
            hit.percent_match
        );
        assert!(!hit.cigar.is_empty(), "a well-formed CIGAR is returned");
    }

    #[tokio::test]
    async fn similar_returns_cluster_mates_in_pagerank_order() {
        let (app, _dir) = sqlite_facade().await;

        let target = "http://synbiohub.org/public/target/1".to_owned();
        let low = "http://synbiohub.org/public/low/1".to_owned();
        let high = "http://synbiohub.org/public/high/1".to_owned();
        let other = "http://synbiohub.org/public/other/1".to_owned();

        // target, low, and high share a cluster; other stands alone.
        app.cluster
            .replace_clusters(vec![
                (target.clone(), ClusterId(0)),
                (low.clone(), ClusterId(0)),
                (high.clone(), ClusterId(0)),
                (other.clone(), ClusterId(1)),
            ])
            .await
            .expect("replace_clusters");
        app.pagerank
            .replace_all_ranks(vec![
                RankRow {
                    iri: low.clone(),
                    score: 1.0,
                },
                RankRow {
                    iri: high.clone(),
                    score: 5.0,
                },
            ])
            .await
            .expect("replace_all_ranks");

        let hits = app
            .sequence()
            .similar(&target, &GraphScope::Union)
            .await
            .expect("similar");
        let order: Vec<&str> = hits.iter().map(|h| h.iri.as_str()).collect();
        assert_eq!(
            order,
            vec![high.as_str(), low.as_str()],
            "mates are the other members, ranked by pagerank descending, never the target"
        );

        let count = app
            .sequence()
            .similar_count(&target, &GraphScope::Union)
            .await
            .expect("similar_count");
        assert_eq!(count, 2, "two cluster mates, excluding the target");
    }

    /// SplitMix64 finalizer, a deterministic 64-bit mixer for test fixtures.
    fn splitmix64(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A deterministic pseudo-random uppercase ACGT sequence of length `len`.
    fn rand_seq(seed: u64, len: usize) -> String {
        let bases = *b"ACGT";
        let mut x = seed.wrapping_add(1);
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            x = splitmix64(x);
            s.push(bases[(x >> 40) as usize % 4] as char);
        }
        s
    }

    /// Flip `count` bases at positions derived from `seed`.
    fn mutate(seq: &str, seed: u64, count: usize) -> String {
        let mut v: Vec<u8> = seq.bytes().collect();
        let mut x = seed.wrapping_add(1);
        for _ in 0..count {
            x = splitmix64(x);
            let at = (x >> 40) as usize % v.len();
            v[at] = if v[at] == b'A' { b'C' } else { b'A' };
        }
        String::from_utf8(v).unwrap()
    }

    /// Import a one-Component-one-Sequence SBOL3 document under `label` and
    /// return the Sequence object's IRI.
    async fn import_labeled_sequence(app: &AppServices, label: &str, elements: &str) -> String {
        let doc = format!(
            r#"
BASE <https://example.org/sbol-db/{label}/>
PREFIX :     <https://example.org/sbol-db/{label}/>
PREFIX SO:   <https://identifiers.org/SO:>
PREFIX EDAM: <https://identifiers.org/edam:>
PREFIX sbol: <http://sbols.org/v3#>

:part
    a                  sbol:Component ;
    sbol:displayId     "part" ;
    sbol:hasNamespace  <https://example.org/sbol-db/{label}> ;
    sbol:role          SO:0000167 ;
    sbol:hasSequence   :part_seq .

:part_seq
    a                  sbol:Sequence ;
    sbol:displayId     "part_seq" ;
    sbol:hasNamespace  <https://example.org/sbol-db/{label}> ;
    sbol:elements      "{elements}" ;
    sbol:encoding      EDAM:format_1207 .
"#
        );
        let report = app
            .store
            .import_document(ImportInput {
                body: doc,
                format: SerializationFormat::Turtle,
                namespace: None,
                source_uri: Some(format!("seq-test://{label}")),
                document_iri: None,
                created_by: None,
                name: None,
                description: None,
                overwrite: ImportOverwrite::Fail,
            })
            .await
            .expect("import_document");
        let objects = app
            .store
            .list_objects(&ListObjectsFilter {
                sbol_class: None,
                role: None,
                graph_id: Some(report.graph_id),
                after_iri: None,
                limit: 100,
            })
            .await
            .expect("list_objects");
        objects
            .iter()
            .find(|o| o.sbol_class.ends_with("#Sequence"))
            .expect("a Sequence object")
            .iri
            .as_str()
            .to_owned()
    }

    /// A single near-duplicate sequence written after another lands in its
    /// cluster through the incremental assign path, with no full rebuild: the
    /// sketch index surfaces the base as a candidate, the aligner accepts, and
    /// the new sequence joins the base's cluster. An unrelated sequence opens its
    /// own. The align path then finds the near-dup off the just-updated sketch.
    #[tokio::test]
    async fn incremental_assign_lands_near_dup_in_the_expected_cluster() {
        let (app, _dir) = sqlite_facade().await;
        let seq = app.sequence();

        let base = rand_seq(11, 400);
        let near = mutate(&base, 5, 2);
        let unrelated = rand_seq(50_000, 400);

        let base_iri = import_labeled_sequence(&app, "base", &base).await;
        let near_iri = import_labeled_sequence(&app, "near", &near).await;
        let unrelated_iri = import_labeled_sequence(&app, "unrelated", &unrelated).await;

        // No clustering exists yet: nothing has been assigned or rebuilt.
        assert!(
            app.cluster.all_assignments().await.unwrap().is_empty(),
            "no assignments before the incremental writes"
        );

        // Each sequence is assigned incrementally as it is written, one at a
        // time, never triggering a corpus rebuild.
        let base_cluster = seq
            .assign_new_sequence(&base_iri, &base)
            .await
            .expect("assign base");
        let near_cluster = seq
            .assign_new_sequence(&near_iri, &near)
            .await
            .expect("assign near-dup");
        let unrelated_cluster = seq
            .assign_new_sequence(&unrelated_iri, &unrelated)
            .await
            .expect("assign unrelated");

        assert_eq!(
            near_cluster, base_cluster,
            "the near-duplicate joins the base's cluster incrementally"
        );
        assert_ne!(
            unrelated_cluster, base_cluster,
            "the unrelated sequence opens its own cluster"
        );

        // The persisted clustering answers `/similar`: the base's only mate is
        // the near-duplicate.
        let mates = seq
            .similar(&base_iri, &GraphScope::Union)
            .await
            .expect("similar");
        let mate_iris: Vec<&str> = mates.iter().map(|m| m.iri.as_str()).collect();
        assert_eq!(
            mate_iris,
            vec![near_iri.as_str()],
            "the base's cluster mate is the near-duplicate"
        );

        // The align path draws candidates from the incrementally updated sketch
        // index and finds the near-duplicate for the base query.
        let hits = seq
            .align(&base, AlignOptions::default(), &GraphScope::Union)
            .await
            .expect("align");
        assert!(
            hits.iter().any(|h| h.sequence_iri == near_iri),
            "the align path finds the near-duplicate off the fresh sketch index"
        );
    }
}
