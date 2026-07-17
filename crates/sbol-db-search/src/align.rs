//! K-mer-seeded banded pairwise alignment, replacing vsearch.
//!
//! The pure option and result types ([`AlignOptions`], [`AlignMode`],
//! [`Alignment`]) are always compiled so the storage contract can name them
//! without pulling rust-bio. The aligner itself ([`align_pair`]) is gated behind
//! the `align` feature, so rust-bio enters only the search and job layers that
//! run alignment and never the storage backends.
//!
//! The query is aligned in full against the target with the target's flanks
//! free (a banded semiglobal alignment, matching vsearch `usearch_global`), so
//! identity follows vsearch's `iddef=2`: matches divided by the alignment length
//! counting internal indel columns but excluding the target's terminal gaps. A
//! native banded aligner produces an identity and CIGAR that are equivalent to
//! vsearch's but not bit-identical on gapped hits.

use serde::Serialize;

/// How a query is matched against indexed sequences. `Substring` is the exact
/// reverse-complement-aware substring path; `GlobalAlign` runs the banded
/// aligner; `Exact` is the fast exact path (vsearch `--search_exact`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum AlignMode {
    #[default]
    Substring,
    GlobalAlign,
    Exact,
}

/// Alignment parameters mirroring vsearch `--usearch_global` defaults: identity
/// threshold `id=0.8`, `maxaccepts=50`, and the sequence-length window
/// `minseqlength=20`..`maxseqlength=5000`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignOptions {
    pub mode: AlignMode,
    /// Minimum fractional identity (`iddef=2`) for a hit to be accepted.
    pub min_identity: f64,
    /// Maximum number of accepted hits.
    pub max_accepts: u32,
    /// Shortest indexed sequence considered.
    pub min_seqlen: u32,
    /// Longest indexed sequence considered.
    pub max_seqlen: u32,
}

impl Default for AlignOptions {
    fn default() -> Self {
        Self {
            mode: AlignMode::Substring,
            min_identity: 0.8,
            max_accepts: 50,
            min_seqlen: 20,
            max_seqlen: 5000,
        }
    }
}

/// One pairwise alignment result, without a target IRI: [`align_pair`] aligns a
/// query against a bare target sequence, so the caller attaches the target IRI
/// when it builds the store-facing `SequenceAlignment`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Alignment {
    /// Fractional identity per `iddef=2`, in `0.0..=1.0`.
    pub percent_match: f64,
    /// `'+'` when the forward query aligned best, `'-'` when its reverse
    /// complement did.
    pub strand: char,
    /// CIGAR over the aligned core: `M` (match or mismatch), `I` (insertion in
    /// the query), `D` (deletion from the query).
    pub cigar: String,
    /// The banded Smith-Waterman score of the winning strand.
    pub score: i32,
}

#[cfg(feature = "align")]
mod aligner {
    use super::{AlignOptions, Alignment};
    use bio::alignment::pairwise::banded::Aligner;
    use bio::alignment::AlignmentOperation;
    use sbol_db_core::kmer::{reverse_complement_string, KMER_K};

    /// Match reward and mismatch penalty for the scoring function.
    const MATCH: i32 = 1;
    const MISMATCH: i32 = -1;
    /// Affine gap costs (open then per-extension), both negative.
    const GAP_OPEN: i32 = -5;
    const GAP_EXTEND: i32 = -1;
    /// Banded-aligner window width around the k-mer seed diagonal.
    const BAND_WIDTH: usize = 6;

    /// Align `query` against `target` on both strands with a k-mer-seeded banded
    /// semiglobal alignment, returning the better-scoring strand's alignment or
    /// `None` when the target falls outside the sequence-length window, no
    /// alignment covers any columns, or the identity is below
    /// `opts.min_identity`.
    ///
    /// Both inputs are treated case-insensitively; only A/C/G/T/U bases score.
    pub fn align_pair(query: &str, target: &str, opts: &AlignOptions) -> Option<Alignment> {
        let q: Vec<u8> = normalize(query);
        let t: Vec<u8> = normalize(target);
        if t.len() < opts.min_seqlen as usize || t.len() > opts.max_seqlen as usize {
            return None;
        }
        if q.is_empty() || t.is_empty() {
            return None;
        }

        let q_rc: Vec<u8> = normalize(&reverse_complement_string(query));

        let fwd = align_one(&q, &t);
        let rev = align_one(&q_rc, &t);
        let (aln, strand) = match (fwd, rev) {
            (Some(f), Some(r)) => {
                if r.score > f.score {
                    (r, '-')
                } else {
                    (f, '+')
                }
            }
            (Some(f), None) => (f, '+'),
            (None, Some(r)) => (r, '-'),
            (None, None) => return None,
        };

        if aln.percent_match < opts.min_identity {
            return None;
        }
        Some(Alignment {
            percent_match: aln.percent_match,
            strand,
            cigar: aln.cigar,
            score: aln.score,
        })
    }

    /// Uppercase and drop whitespace so RNA/DNA and mixed case compare byte-wise.
    fn normalize(s: &str) -> Vec<u8> {
        s.bytes()
            .filter(|b| !b.is_ascii_whitespace())
            .map(|b| b.to_ascii_uppercase())
            .collect()
    }

    struct OneStrand {
        percent_match: f64,
        cigar: String,
        score: i32,
    }

    /// Run a banded semiglobal alignment of `q` against `t` and reduce it to
    /// identity, CIGAR, and score. Semiglobal aligns the whole query with the
    /// target's flanks free, so the operations span exactly the query and the
    /// target's terminal gaps are excluded, matching vsearch `usearch_global`
    /// under `iddef=2`. Returns `None` when the alignment spans no columns.
    fn align_one(q: &[u8], t: &[u8]) -> Option<OneStrand> {
        let score_fn = |a: u8, b: u8| if a == b { MATCH } else { MISMATCH };
        let mut aligner = Aligner::new(GAP_OPEN, GAP_EXTEND, score_fn, KMER_K, BAND_WIDTH);
        let alignment = aligner.semiglobal(q, t);

        let mut matches = 0usize;
        let mut columns = 0usize;
        let mut cigar = CigarBuilder::default();
        for op in &alignment.operations {
            match op {
                AlignmentOperation::Match => {
                    matches += 1;
                    columns += 1;
                    cigar.push('M');
                }
                AlignmentOperation::Subst => {
                    columns += 1;
                    cigar.push('M');
                }
                AlignmentOperation::Ins => {
                    columns += 1;
                    cigar.push('I');
                }
                AlignmentOperation::Del => {
                    columns += 1;
                    cigar.push('D');
                }
                // Terminal soft-clips are excluded from the iddef=2 length.
                AlignmentOperation::Xclip(_) | AlignmentOperation::Yclip(_) => {}
            }
        }
        if columns == 0 {
            return None;
        }
        Some(OneStrand {
            percent_match: matches as f64 / columns as f64,
            cigar: cigar.finish(),
            score: alignment.score,
        })
    }

    /// Run-length CIGAR accumulator.
    #[derive(Default)]
    struct CigarBuilder {
        out: String,
        current: Option<char>,
        run: usize,
    }

    impl CigarBuilder {
        fn push(&mut self, op: char) {
            match self.current {
                Some(c) if c == op => self.run += 1,
                Some(c) => {
                    self.flush(c);
                    self.current = Some(op);
                    self.run = 1;
                }
                None => {
                    self.current = Some(op);
                    self.run = 1;
                }
            }
        }

        fn flush(&mut self, op: char) {
            use std::fmt::Write;
            let _ = write!(self.out, "{}{}", self.run, op);
        }

        fn finish(mut self) -> String {
            if let Some(c) = self.current.take() {
                self.flush(c);
            }
            self.out
        }
    }
}

#[cfg(feature = "align")]
pub use aligner::align_pair;

#[cfg(all(test, feature = "align"))]
mod tests {
    use super::*;

    const SEQ: &str = "ACGTACGTACGTTTGGCCAAGGTTCCAAGGATCGATCGAT";

    #[test]
    fn identical_sequence_is_full_identity() {
        let opts = AlignOptions::default();
        let aln = align_pair(SEQ, SEQ, &opts).expect("self-alignment");
        assert_eq!(aln.percent_match, 1.0);
        assert_eq!(aln.strand, '+');
        assert_eq!(aln.cigar, format!("{}M", SEQ.len()));
    }

    #[test]
    fn single_mismatch_drops_below_full() {
        // Flip one internal base; the local alignment still spans the whole
        // length, so identity is (len-1)/len < 1.0 and above the 0.8 floor.
        let mut variant: Vec<char> = SEQ.chars().collect();
        variant[20] = if variant[20] == 'A' { 'C' } else { 'A' };
        let variant: String = variant.into_iter().collect();
        let opts = AlignOptions::default();
        let aln = align_pair(&variant, SEQ, &opts).expect("variant alignment");
        assert!(aln.percent_match < 1.0, "one mismatch is not full identity");
        let expected = (SEQ.len() - 1) as f64 / SEQ.len() as f64;
        assert!((aln.percent_match - expected).abs() < 1e-9);
    }

    #[test]
    fn reverse_complement_query_aligns_on_minus_strand() {
        let rc = sbol_db_core::kmer::reverse_complement_string(SEQ);
        let opts = AlignOptions::default();
        let aln = align_pair(&rc, SEQ, &opts).expect("reverse-complement alignment");
        assert_eq!(aln.strand, '-');
        assert_eq!(aln.percent_match, 1.0);
    }

    #[test]
    fn below_min_identity_returns_none() {
        let unrelated = "TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT";
        let opts = AlignOptions::default();
        assert!(align_pair(unrelated, SEQ, &opts).is_none());
    }

    #[test]
    fn target_outside_seqlen_window_is_skipped() {
        let opts = AlignOptions {
            min_seqlen: 20,
            ..AlignOptions::default()
        };
        // 16 bp target is below the 20 bp floor.
        assert!(align_pair("ACGTACGTACGTACGT", "ACGTACGTACGTACGT", &opts).is_none());
    }
}
