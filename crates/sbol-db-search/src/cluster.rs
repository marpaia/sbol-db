//! Sequence clustering identity and the greedy centroid clustering algorithm.
//!
//! This module holds the [`ClusterId`] the `ClusterStore` persists, always
//! compiled so the storage contract can name it, and [`cluster_sequences`], the
//! greedy centroid clustering that assigns those ids. The algorithm reproduces
//! `vsearch --cluster_fast --id 0.8`: sequences are sorted by length descending
//! and each is assigned to the first existing centroid it aligns to at or above
//! the identity threshold, else it opens a new centroid. Clustering reuses the
//! banded aligner, so it is gated behind the `align` feature and rust-bio never
//! enters the storage backends.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A cluster's numeric id. One cluster gathers sequences within the identity
/// threshold of a shared centroid; `/similar` returns a target's cluster mates.
/// The width matches the backends' signed integer columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ClusterId(pub i64);

impl fmt::Display for ClusterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(feature = "align")]
pub use clustering::cluster_sequences;

#[cfg(feature = "align")]
mod clustering {
    use std::collections::{HashMap, HashSet};

    use sbol_db_core::kmer::canonical_kmers;

    use super::ClusterId;
    use crate::align::{align_pair, AlignMode, AlignOptions};

    /// Greedily cluster `(iri, elements)` sequences by identity, reproducing
    /// `vsearch --cluster_fast --id 0.8`.
    ///
    /// Sequences are sorted by element length descending (with the IRI as a
    /// deterministic tie-break) and processed in that order. Each sequence is
    /// aligned against the existing centroids that share a canonical k-mer with
    /// it, in centroid-creation order, and is assigned to the first whose
    /// identity meets `opts.min_identity`; if none does it opens a new centroid.
    /// The returned assignments are in the sorted processing order, one per input
    /// sequence, and are stable across runs for a given input.
    ///
    /// Clustering uses only the identity threshold, applying no sequence-length
    /// window: `--cluster_fast` has no `minseqlength`/`maxseqlength` unlike
    /// `usearch_global`, so the per-pair alignment neutralizes that window. Both
    /// strands are aligned (the aligner keeps the better-scoring one), so a
    /// reverse-complement duplicate clusters with its centroid.
    pub fn cluster_sequences(
        mut sequences: Vec<(String, String)>,
        opts: &AlignOptions,
    ) -> Vec<(String, ClusterId)> {
        sequences.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

        // Per-pair options: only the identity threshold matters for clustering,
        // and the length window is disabled so a centroid of any length can
        // accept a member.
        let pair_opts = AlignOptions {
            mode: AlignMode::GlobalAlign,
            min_identity: opts.min_identity,
            max_accepts: 1,
            min_seqlen: 0,
            max_seqlen: u32::MAX,
        };

        struct Centroid {
            elements: String,
            cluster: ClusterId,
        }
        let mut centroids: Vec<Centroid> = Vec::new();
        // Canonical k-mer -> the centroid indices whose elements contain it, the
        // in-memory prefilter that narrows the align candidates to centroids that
        // share sequence content.
        let mut kmer_index: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut assignments: Vec<(String, ClusterId)> = Vec::with_capacity(sequences.len());

        for (iri, elements) in sequences {
            let q_kmers: HashSet<u32> = canonical_kmers(&elements).map(|h| h.canonical).collect();

            let mut candidates: HashSet<usize> = HashSet::new();
            for k in &q_kmers {
                if let Some(list) = kmer_index.get(k) {
                    candidates.extend(list.iter().copied());
                }
            }
            // Align against candidate centroids in creation order and take the
            // first that accepts, the `--cluster_fast` first-match rule.
            let mut candidates: Vec<usize> = candidates.into_iter().collect();
            candidates.sort_unstable();

            let mut assigned: Option<ClusterId> = None;
            for &ci in &candidates {
                if align_pair(&elements, &centroids[ci].elements, &pair_opts).is_some() {
                    assigned = Some(centroids[ci].cluster);
                    break;
                }
            }

            let cluster = match assigned {
                Some(cluster) => cluster,
                None => {
                    let idx = centroids.len();
                    let cluster = ClusterId(idx as i64);
                    for k in &q_kmers {
                        kmer_index.entry(*k).or_default().push(idx);
                    }
                    centroids.push(Centroid {
                        elements: elements.clone(),
                        cluster,
                    });
                    cluster
                }
            };
            assignments.push((iri, cluster));
        }

        assignments
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashSet;

        const SEQ: &str = "ACGTACGTACGTTTGGCCAAGGTTCCAAGGATCGATCGAT";

        fn one_mismatch(seq: &str) -> String {
            let mut v: Vec<char> = seq.chars().collect();
            v[20] = if v[20] == 'A' { 'C' } else { 'A' };
            v.into_iter().collect()
        }

        #[test]
        fn near_identical_share_a_cluster_and_unrelated_stands_alone() {
            let near = one_mismatch(SEQ);
            let unrelated = "TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT";
            let assignments = cluster_sequences(
                vec![
                    ("http://example.org/a".to_owned(), SEQ.to_owned()),
                    ("http://example.org/b".to_owned(), near),
                    ("http://example.org/c".to_owned(), unrelated.to_owned()),
                ],
                &AlignOptions::default(),
            );

            let by_iri: HashMap<&str, ClusterId> =
                assignments.iter().map(|(i, c)| (i.as_str(), *c)).collect();
            let distinct: HashSet<ClusterId> = by_iri.values().copied().collect();
            assert_eq!(distinct.len(), 2, "two near-identical + one unrelated");
            assert_eq!(
                by_iri["http://example.org/a"], by_iri["http://example.org/b"],
                "the near-identical pair shares one cluster"
            );
            assert_ne!(
                by_iri["http://example.org/a"], by_iri["http://example.org/c"],
                "the unrelated sequence stands alone"
            );
        }

        #[test]
        fn empty_input_yields_no_assignments() {
            assert!(cluster_sequences(Vec::new(), &AlignOptions::default()).is_empty());
        }

        #[test]
        fn assignment_is_deterministic_across_runs() {
            let inputs = vec![
                ("http://example.org/x".to_owned(), SEQ.to_owned()),
                ("http://example.org/y".to_owned(), one_mismatch(SEQ)),
                (
                    "http://example.org/z".to_owned(),
                    "GGGGCCCCGGGGCCCCGGGGCCCCGGGGCCCCGGGG".to_owned(),
                ),
            ];
            let first = cluster_sequences(inputs.clone(), &AlignOptions::default());
            let second = cluster_sequences(inputs, &AlignOptions::default());
            assert_eq!(first, second, "same input yields identical assignments");
        }
    }
}
