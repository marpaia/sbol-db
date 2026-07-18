//! MinHash sketches and LSH banding for scalable sequence similarity.
//!
//! The k=8 substring index answers short exact-motif lookups but does not
//! discriminate similar sequences: over a 4-letter alphabet almost every
//! sequence shares some 8-mer with almost every other, so a "shares a k-mer"
//! prefilter admits nearly the whole corpus. This module builds a discriminating
//! similarity index instead.
//!
//! A [`Signature`] is a fixed-size MinHash sketch over the canonical k-mers of a
//! sequence (default k=14, reusing [`sbol_db_core::kmer`] canonicalization so
//! DNA/RNA and reverse-complement variants collapse). Two signatures agree in a
//! slot with probability equal to the k-mer Jaccard similarity of their
//! sequences, so the fraction of agreeing slots estimates that Jaccard
//! ([`Signature::estimated_jaccard`]).
//!
//! [`band_hashes`] splits a signature into `bands` bands of `rows` rows and
//! hashes each band. Two sequences share a band hash iff every slot in some band
//! agrees, so the probability they land in a shared bucket rises sharply around
//! a Jaccard threshold set by the band/row split. The default 8 bands of 4 rows
//! puts that threshold near 0.6 and makes sequences at >=0.7 k-mer Jaccard (well
//! inside 80% identity) collide with high probability while unrelated sequences
//! almost never do. Candidate generation is then the union of a query's band
//! buckets, and the banded aligner verifies only those candidates.

use serde::{Deserialize, Serialize};

use sbol_db_core::kmer::canonical_kmers_u64;

/// Sketch and banding parameters. `sketch_k` is the canonical k-mer width; the
/// signature has `bands * rows` slots, split into `bands` bands of `rows` rows
/// for LSH. All producers and consumers of a signature must agree on these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchParams {
    pub sketch_k: usize,
    pub bands: usize,
    pub rows: usize,
}

impl Default for SketchParams {
    /// k=14 k-mers into 32 MinHash slots (8 bands of 4 rows). k=14 gives a
    /// 4^14 token space, discriminating enough that unrelated sequences share
    /// essentially no canonical k-mers; 8x4 banding collides sequences at
    /// >=~0.6 Jaccard with rising probability and near-certainly by 0.7.
    fn default() -> Self {
        Self {
            sketch_k: 14,
            bands: 8,
            rows: 4,
        }
    }
}

impl SketchParams {
    /// The signature length these parameters produce.
    pub fn num_hashes(&self) -> usize {
        self.bands * self.rows
    }
}

/// A fixed-size MinHash signature: one minimum hash per slot over a sequence's
/// canonical k-mers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature(pub Vec<u64>);

impl Signature {
    /// Serialize to little-endian bytes (8 per slot), the on-disk BLOB form.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.0.len() * 8);
        for v in &self.0 {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Reconstruct a signature from [`to_bytes`](Self::to_bytes) output.
    /// Returns `None` when the length is not a whole number of `u64` slots.
    pub fn from_bytes(bytes: &[u8]) -> Option<Signature> {
        if !bytes.len().is_multiple_of(8) {
            return None;
        }
        let mut out = Vec::with_capacity(bytes.len() / 8);
        for chunk in bytes.chunks_exact(8) {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(chunk);
            out.push(u64::from_le_bytes(arr));
        }
        Some(Signature(out))
    }

    /// Estimate the k-mer Jaccard similarity of the two sequences as the
    /// fraction of signature slots that agree. `0.0` when either signature is
    /// empty. Both signatures must come from the same [`SketchParams`].
    pub fn estimated_jaccard(&self, other: &Signature) -> f64 {
        let n = self.0.len().min(other.0.len());
        if n == 0 {
            return 0.0;
        }
        let agree = self
            .0
            .iter()
            .zip(other.0.iter())
            .filter(|(a, b)| a == b)
            .count();
        agree as f64 / n as f64
    }
}

/// Compute a sequence's MinHash signature under `params`. Returns `None` when
/// the sequence yields no canonical k-mer (shorter than `sketch_k`, or every
/// window contains an ambiguous base), so the caller simply leaves it
/// unsketched.
pub fn sketch(elements: &str, params: &SketchParams) -> Option<Signature> {
    let n = params.num_hashes();
    let mut mins = vec![u64::MAX; n];
    let mut any = false;
    for token in canonical_kmers_u64(elements, params.sketch_k) {
        any = true;
        // Broder's two-hash trick: derive `n` near-independent hashes of the
        // token as `a + i * b`, so each slot tracks the minimum of one hash
        // family over all tokens in a single pass.
        let (a, b) = base_hashes(token);
        let mut hi = a;
        for slot in mins.iter_mut() {
            if hi < *slot {
                *slot = hi;
            }
            hi = hi.wrapping_add(b);
        }
    }
    if any {
        Some(Signature(mins))
    } else {
        None
    }
}

/// Hash each band of `sig` into a bucket key. The result has `params.bands`
/// entries; two sequences share an entry iff every row of some band agrees,
/// which is the LSH collision event. The band index is folded in so identical
/// row values in different bands do not alias. `sig` must have
/// `params.num_hashes()` slots.
pub fn band_hashes(sig: &Signature, params: &SketchParams) -> Vec<u64> {
    let mut out = Vec::with_capacity(params.bands);
    for band in 0..params.bands {
        let start = band * params.rows;
        let slots = &sig.0[start..start + params.rows];
        out.push(fold_band(band as u64, slots));
    }
    out
}

/// SplitMix64 finalizer, a fast well-distributed 64-bit mixer.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Two independent 64-bit hashes of a token for the two-hash MinHash family.
/// The second is forced odd so `a + i * b` visits distinct residues.
fn base_hashes(token: u64) -> (u64, u64) {
    let a = splitmix64(token);
    let b = splitmix64(token ^ 0xA5A5_A5A5_A5A5_A5A5) | 1;
    (a, b)
}

/// Fold a band's rows plus its index into one bucket key.
fn fold_band(band_index: u64, slots: &[u64]) -> u64 {
    let mut acc = splitmix64(band_index ^ 0x243F_6A88_85A3_08D3);
    for &s in slots {
        acc = splitmix64(acc ^ s);
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbol_db_core::kmer::reverse_complement_string;
    use std::collections::HashSet;

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

    /// Flip one base to a different one.
    fn mutate(seq: &str, at: usize) -> String {
        let mut v: Vec<u8> = seq.bytes().collect();
        v[at] = if v[at] == b'A' { b'C' } else { b'A' };
        String::from_utf8(v).unwrap()
    }

    fn shares_band(a: &Signature, b: &Signature, p: &SketchParams) -> bool {
        let ba: HashSet<u64> = band_hashes(a, p).into_iter().collect();
        band_hashes(b, p).into_iter().any(|h| ba.contains(&h))
    }

    #[test]
    fn near_identical_sequences_share_a_band() {
        let p = SketchParams::default();
        let a = rand_seq(1, 400);
        // Two point mutations keep k-mer Jaccard high for a 400 bp sequence.
        let b = mutate(&mutate(&a, 100), 275);
        let sa = sketch(&a, &p).unwrap();
        let sb = sketch(&b, &p).unwrap();
        assert!(
            sa.estimated_jaccard(&sb) > 0.7,
            "two mismatches leave a high estimated Jaccard"
        );
        assert!(
            shares_band(&sa, &sb, &p),
            "near-identical sequences collide in at least one band"
        );
    }

    #[test]
    fn unrelated_sequences_do_not_share_a_band() {
        let p = SketchParams::default();
        let a = sketch(&rand_seq(1, 400), &p).unwrap();
        let b = sketch(&rand_seq(9_999, 400), &p).unwrap();
        assert!(
            a.estimated_jaccard(&b) < 0.2,
            "unrelated sequences have a low estimated Jaccard"
        );
        assert!(
            !shares_band(&a, &b, &p),
            "unrelated sequences do not collide in any band"
        );
    }

    #[test]
    fn identical_sequences_estimate_full_jaccard() {
        let p = SketchParams::default();
        let a = rand_seq(42, 300);
        let sa = sketch(&a, &p).unwrap();
        let sb = sketch(&a, &p).unwrap();
        assert_eq!(sa, sb, "the sketch is deterministic");
        assert_eq!(sa.estimated_jaccard(&sb), 1.0);
    }

    #[test]
    fn reverse_complement_yields_the_same_signature() {
        let p = SketchParams::default();
        let a = rand_seq(7, 300);
        let rc = reverse_complement_string(&a);
        assert_eq!(
            sketch(&a, &p).unwrap(),
            sketch(&rc, &p).unwrap(),
            "canonical k-mers make a sequence and its reverse complement sketch identically"
        );
    }

    #[test]
    fn rna_and_dna_sketch_identically() {
        let p = SketchParams::default();
        let dna = rand_seq(3, 300);
        let rna = dna.replace('T', "U");
        assert_eq!(
            sketch(&dna, &p).unwrap(),
            sketch(&rna, &p).unwrap(),
            "T and U share a base code, so DNA and its RNA transcript sketch identically"
        );
    }

    #[test]
    fn too_short_sequence_has_no_sketch() {
        let p = SketchParams::default();
        assert!(sketch("ACGT", &p).is_none());
        assert!(sketch("", &p).is_none());
    }

    #[test]
    fn signature_bytes_round_trip() {
        let sig = Signature(vec![1, 2, u64::MAX, 0, 987_654_321]);
        let bytes = sig.to_bytes();
        assert_eq!(Signature::from_bytes(&bytes), Some(sig));
        assert_eq!(Signature::from_bytes(&[0, 1, 2]), None);
    }
}
