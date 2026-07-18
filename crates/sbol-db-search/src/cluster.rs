//! Sequence clustering identity and the near-linear clustering algorithm.
//!
//! This module holds the [`ClusterId`] the `ClusterStore` persists, always
//! compiled so the storage contract can name it, and [`cluster_sequences`], the
//! linclust-style clustering that assigns those ids. Sequences are sorted by
//! length descending and each is aligned only against the representatives that
//! share a MinHash/LSH band with it (the discriminating similarity index from
//! [`crate::minhash`]); it joins the first such representative at or above the
//! identity threshold, else it opens a new representative. Only representatives
//! enter the band buckets, so per-sequence work is bounded by bucket size rather
//! than by the number of clusters so far and clustering is near-linear.
//! Clustering reuses the banded aligner, so it is gated behind the `align`
//! feature and rust-bio never enters the storage backends.

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
    use std::collections::HashMap;

    use super::ClusterId;
    use crate::align::{align_pair, AlignMode, AlignOptions};
    use crate::minhash::{band_hashes, sketch, SketchParams};

    /// Cluster `(iri, elements)` sequences by identity at near-linear cost using
    /// the MinHash/LSH similarity index for candidate generation.
    ///
    /// Sequences are sorted by element length descending (with the IRI as a
    /// deterministic tie-break) and processed in that order, the same
    /// representative-selection bias as `vsearch --cluster_fast`. Each sequence's
    /// MinHash sketch is banded into LSH bucket keys; its candidates are the
    /// representatives already sharing at least one band, ranked by shared-band
    /// count descending (creation order breaking ties). It is aligned against
    /// those candidates in that order and joins the first whose identity meets
    /// `opts.min_identity`; if none does it opens a new representative. Only
    /// representatives are registered into the band buckets, so a large family of
    /// near-duplicates contributes a single bucket entry and per-sequence work is
    /// bounded by the number of representatives sharing a band, not by the total
    /// number of clusters. The returned assignments are in the sorted processing
    /// order, one per input sequence, and are stable across runs for a given
    /// input.
    ///
    /// Clustering uses only the identity threshold, applying no sequence-length
    /// window: `--cluster_fast` has no `minseqlength`/`maxseqlength` unlike
    /// `usearch_global`, so the per-pair alignment neutralizes that window.
    /// Reverse-complement duplicates cluster together because canonical k-mers
    /// give a sequence and its reverse complement the same sketch and bands, and
    /// the aligner keeps the better-scoring strand. A sequence too short to sketch
    /// (shorter than the sketch k-mer width) has no bands, so it draws no
    /// candidates and opens its own representative.
    pub fn cluster_sequences(
        mut sequences: Vec<(String, String)>,
        opts: &AlignOptions,
    ) -> Vec<(String, ClusterId)> {
        sequences.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

        // Per-pair options: only the identity threshold matters for clustering,
        // and the length window is disabled so a representative of any length can
        // accept a member.
        let pair_opts = AlignOptions {
            mode: AlignMode::GlobalAlign,
            min_identity: opts.min_identity,
            max_accepts: 1,
            min_seqlen: 0,
            max_seqlen: u32::MAX,
        };

        let params = SketchParams::default();

        struct Representative {
            elements: String,
            cluster: ClusterId,
        }
        let mut representatives: Vec<Representative> = Vec::new();
        // LSH band hash -> the representative indices whose sketch falls in that
        // bucket. Only representatives are indexed, so the candidate set for a
        // sequence is the small set of representatives it could plausibly match.
        let mut band_index: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut assignments: Vec<(String, ClusterId)> = Vec::with_capacity(sequences.len());

        for (iri, elements) in sequences {
            let bands = sketch(&elements, &params)
                .map(|sig| band_hashes(&sig, &params))
                .unwrap_or_default();

            // Count how many bands each candidate representative shares, then
            // align in descending shared-band order (creation index breaking
            // ties) and take the first that accepts.
            let mut shared: HashMap<usize, u32> = HashMap::new();
            for band in &bands {
                if let Some(list) = band_index.get(band) {
                    for &ri in list {
                        *shared.entry(ri).or_insert(0) += 1;
                    }
                }
            }
            let mut candidates: Vec<(usize, u32)> = shared.into_iter().collect();
            candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            let mut assigned: Option<ClusterId> = None;
            for &(ri, _) in &candidates {
                if align_pair(&elements, &representatives[ri].elements, &pair_opts).is_some() {
                    assigned = Some(representatives[ri].cluster);
                    break;
                }
            }

            let cluster = match assigned {
                Some(cluster) => cluster,
                None => {
                    let idx = representatives.len();
                    let cluster = ClusterId(idx as i64);
                    for band in &bands {
                        band_index.entry(*band).or_default().push(idx);
                    }
                    representatives.push(Representative { elements, cluster });
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
        use std::time::Instant;

        const SEQ: &str = "ACGTACGTACGTTTGGCCAAGGTTCCAAGGATCGATCGAT";

        fn one_mismatch(seq: &str) -> String {
            let mut v: Vec<char> = seq.chars().collect();
            v[20] = if v[20] == 'A' { 'C' } else { 'A' };
            v.into_iter().collect()
        }

        /// SplitMix64 finalizer, a deterministic 64-bit mixer for test fixtures.
        fn splitmix64(mut x: u64) -> u64 {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Deterministic pseudo-random ACGT sequence of length `len`.
        fn rand_seq(seed: u64, len: usize) -> String {
            let bases = [b'A', b'C', b'G', b'T'];
            let mut x = seed.wrapping_add(1);
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                x = splitmix64(x);
                s.push(bases[(x >> 40) as usize % 4] as char);
            }
            s
        }

        /// Flip `count` bases at positions derived from `seed` to a different base.
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

        #[test]
        fn near_identical_share_a_cluster_and_unrelated_stands_alone() {
            // The MinHash/LSH candidate path discriminates on k=14 k-mers, so a
            // near-identical pair must be long enough for a point mutation to
            // leave the k-mer Jaccard high; real parts run hundreds of bases.
            let seq = rand_seq(11, 400);
            let near = mutate(&seq, 5, 2);
            let unrelated = rand_seq(50_000, 400);
            let assignments = cluster_sequences(
                vec![
                    ("http://example.org/a".to_owned(), seq),
                    ("http://example.org/b".to_owned(), near),
                    ("http://example.org/c".to_owned(), unrelated),
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

        #[test]
        fn unrelated_sequences_get_distinct_clusters() {
            let inputs: Vec<(String, String)> = (0..8u64)
                .map(|s| {
                    (
                        format!("http://example.org/seq/{s}"),
                        rand_seq(s * 1_000 + 1, 400),
                    )
                })
                .collect();
            let assignments = cluster_sequences(inputs, &AlignOptions::default());
            let distinct: HashSet<ClusterId> = assignments.iter().map(|(_, c)| *c).collect();
            assert_eq!(
                distinct.len(),
                8,
                "unrelated random sequences each open their own cluster"
            );
        }

        #[test]
        fn near_duplicate_family_shares_one_cluster() {
            // A base sequence plus 40 two-mutation variants all land in one
            // cluster: the LSH band index surfaces the base as a candidate and
            // the aligner accepts each variant.
            let base = rand_seq(7, 400);
            let mut inputs = vec![("http://example.org/base".to_owned(), base.clone())];
            for i in 0..40u64 {
                inputs.push((
                    format!("http://example.org/variant/{i}"),
                    mutate(&base, i + 1, 2),
                ));
            }
            let assignments = cluster_sequences(inputs, &AlignOptions::default());
            let distinct: HashSet<ClusterId> = assignments.iter().map(|(_, c)| *c).collect();
            assert_eq!(
                distinct.len(),
                1,
                "the whole near-duplicate family clusters together"
            );
        }

        #[test]
        fn scale_test_is_near_linear() {
            // Many near-duplicate families plus random singletons. The old
            // prefilter shared 8-mers across nearly every pair, so this many
            // sequences would drive an effectively O(n^2) alignment count; the
            // LSH band candidate set keeps per-sequence work bounded, so this
            // completes quickly and each family collapses to one cluster.
            const FAMILIES: u64 = 40;
            const VARIANTS_PER_FAMILY: u64 = 30;
            const SINGLETONS: u64 = 600;

            let mut inputs: Vec<(String, String)> = Vec::new();
            for f in 0..FAMILIES {
                let base = rand_seq(f * 10_007 + 3, 400);
                inputs.push((format!("http://example.org/fam/{f}/base"), base.clone()));
                for v in 0..VARIANTS_PER_FAMILY {
                    inputs.push((
                        format!("http://example.org/fam/{f}/var/{v}"),
                        mutate(&base, f * 100 + v + 1, 2),
                    ));
                }
            }
            for s in 0..SINGLETONS {
                inputs.push((
                    format!("http://example.org/single/{s}"),
                    rand_seq(9_000_000 + s * 31, 400),
                ));
            }
            let total = inputs.len();

            let start = Instant::now();
            let assignments = cluster_sequences(inputs, &AlignOptions::default());
            let elapsed = start.elapsed();

            assert_eq!(assignments.len(), total);
            let distinct: HashSet<ClusterId> = assignments.iter().map(|(_, c)| *c).collect();
            // Each family collapses toward one representative and each singleton
            // is its own cluster. LSH band collision is probabilistic, so a
            // handful of variants may miss their family and open an extra
            // cluster; the count stays at or just above the ideal and far below
            // the total, which is the near-linear collapse the test proves.
            let ideal = (FAMILIES + SINGLETONS) as usize;
            assert!(
                distinct.len() >= ideal,
                "singletons never merge, so clusters cannot drop below the ideal"
            );
            assert!(
                distinct.len() <= ideal + FAMILIES as usize,
                "families collapse: {} clusters is near the ideal {ideal}, not the {total} inputs",
                distinct.len()
            );
            assert!(
                elapsed.as_secs() < 30,
                "clustering {total} sequences stays near-linear (took {elapsed:?})"
            );
        }
    }
}
