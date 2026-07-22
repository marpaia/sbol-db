//! K-mer encoding for nucleotide substring search.
//!
//! Encodes IUPAC DNA/RNA k-mers as 2-bit packed integers and computes
//! reverse complements at the same packed level. Ambiguous bases (`N`, `R`,
//! `Y`, ...) interrupt the k-mer stream — the iterator skips any window
//! containing one rather than emitting partial / degenerate values.
//!
//! Storage shape (sbol_sequence_kmers): kmer = canonical_kmer(forward, rc),
//! strand = '+' if forward was lex-smaller (or equal), '-' otherwise. This
//! halves the index footprint and turns reverse-complement search into a
//! single lookup per seed position.

/// k-mer width in bases. Fits in 2*K = 16 bits, so each canonical k-mer
/// rides in a 32-bit `integer` Postgres column with room to spare.
pub const KMER_K: usize = 8;
pub const KMER_BITS: usize = KMER_K * 2;
pub const KMER_MASK: u32 = (1u32 << KMER_BITS) - 1;

/// Strand the canonical form of a k-mer was drawn from. `Forward` means the
/// forward k-mer was lex-≤ to its reverse complement; `Reverse` means the
/// reverse complement was strictly smaller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KmerStrand {
    Forward,
    Reverse,
}

impl KmerStrand {
    pub fn as_char(self) -> char {
        match self {
            Self::Forward => '+',
            Self::Reverse => '-',
        }
    }

    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '+' => Some(Self::Forward),
            '-' => Some(Self::Reverse),
            _ => None,
        }
    }
}

/// One emitted k-mer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KmerHit {
    pub canonical: u32,
    pub position: usize,
    pub strand: KmerStrand,
}

/// Map a single base to its 2-bit code or return `None` for ambiguous /
/// non-IUPAC characters. `T` and `U` collapse to the same code, so RNA
/// queries match DNA sequences (and vice versa) under the canonical
/// encoding without explicit transcription.
#[inline]
fn base_code(b: u8) -> Option<u32> {
    match b {
        b'A' | b'a' => Some(0),
        b'C' | b'c' => Some(1),
        b'G' | b'g' => Some(2),
        b'T' | b't' | b'U' | b'u' => Some(3),
        _ => None,
    }
}

/// Reverse-complement a packed k-mer of width [`KMER_K`].
pub fn reverse_complement(kmer: u32) -> u32 {
    let mut k = kmer;
    let mut rc: u32 = 0;
    for _ in 0..KMER_K {
        let base = k & 0b11;
        let comp = base ^ 0b11;
        rc = (rc << 2) | comp;
        k >>= 2;
    }
    rc & KMER_MASK
}

/// Canonical form: min(forward, reverse_complement). Returns the canonical
/// integer alongside the strand that produced it.
pub fn canonical(kmer: u32) -> (u32, KmerStrand) {
    let rc = reverse_complement(kmer);
    if kmer <= rc {
        (kmer, KmerStrand::Forward)
    } else {
        (rc, KmerStrand::Reverse)
    }
}

/// Iterate canonical k-mers of `elements`, skipping any window that contains
/// an ambiguous base. Positions are 0-indexed against the input string.
pub fn canonical_kmers(elements: &str) -> impl Iterator<Item = KmerHit> + '_ {
    let bytes = elements.as_bytes();
    CanonicalIter {
        bytes,
        index: 0,
        primed: false,
        forward: 0,
    }
}

struct CanonicalIter<'a> {
    bytes: &'a [u8],
    index: usize,
    primed: bool,
    forward: u32,
}

impl Iterator for CanonicalIter<'_> {
    type Item = KmerHit;

    fn next(&mut self) -> Option<KmerHit> {
        let n = self.bytes.len();
        if n < KMER_K {
            return None;
        }
        let last_start = n - KMER_K;
        loop {
            if !self.primed {
                if self.index > last_start {
                    return None;
                }
                // Try to build a k-mer starting at `self.index`. If we hit an
                // ambiguous base, restart immediately after it.
                let mut acc: u32 = 0;
                let mut i = self.index;
                let stop = i + KMER_K;
                let mut bad: Option<usize> = None;
                while i < stop {
                    match base_code(self.bytes[i]) {
                        Some(code) => acc = ((acc << 2) | code) & KMER_MASK,
                        None => {
                            bad = Some(i);
                            break;
                        }
                    }
                    i += 1;
                }
                if let Some(b) = bad {
                    self.index = b + 1;
                    continue;
                }
                self.forward = acc;
                self.primed = true;
                let (c, s) = canonical(acc);
                let pos = self.index;
                self.index += 1;
                return Some(KmerHit {
                    canonical: c,
                    position: pos,
                    strand: s,
                });
            } else {
                // Slide one base.
                let new_idx = self.index + KMER_K - 1;
                if new_idx >= n {
                    return None;
                }
                match base_code(self.bytes[new_idx]) {
                    Some(code) => {
                        self.forward = ((self.forward << 2) | code) & KMER_MASK;
                        let (c, s) = canonical(self.forward);
                        let pos = self.index;
                        self.index += 1;
                        return Some(KmerHit {
                            canonical: c,
                            position: pos,
                            strand: s,
                        });
                    }
                    None => {
                        self.primed = false;
                        self.index = new_idx + 1;
                        continue;
                    }
                }
            }
        }
    }
}

/// Reverse-complement a packed k-mer of width `k` (2 bits per base) held in a
/// `u64`. The complement is taken over the low `2 * k` bits; higher bits are
/// assumed clear. `k` must satisfy `1 <= k <= 32`.
pub fn reverse_complement_u64(kmer: u64, k: usize) -> u64 {
    let mut src = kmer;
    let mut rc: u64 = 0;
    for _ in 0..k {
        let comp = (src & 0b11) ^ 0b11;
        rc = (rc << 2) | comp;
        src >>= 2;
    }
    rc
}

/// Iterate canonical packed k-mers of arbitrary width `k` over `elements`,
/// yielding the canonical (`min(forward, reverse_complement)`) `u64` for each
/// window and skipping any window containing an ambiguous base. This is the
/// wider-`k`, `u64`-packed sibling of [`canonical_kmers`]: the similarity sketch
/// picks a `k` beyond [`KMER_K`] for the specificity a substring seed does not
/// need, while sharing the same base encoding so DNA/RNA (`T`/`U`) and
/// reverse-complement variants collapse identically. `k` must satisfy
/// `1 <= k <= 32`.
pub fn canonical_kmers_u64(elements: &str, k: usize) -> impl Iterator<Item = u64> + '_ {
    let mask = if k >= 32 {
        u64::MAX
    } else {
        (1u64 << (2 * k)) - 1
    };
    CanonicalIterU64 {
        bytes: elements.as_bytes(),
        k,
        mask,
        index: 0,
        primed: false,
        forward: 0,
    }
}

struct CanonicalIterU64<'a> {
    bytes: &'a [u8],
    k: usize,
    mask: u64,
    index: usize,
    primed: bool,
    forward: u64,
}

impl CanonicalIterU64<'_> {
    fn canonical_of(&self, forward: u64) -> u64 {
        forward.min(reverse_complement_u64(forward, self.k))
    }
}

impl Iterator for CanonicalIterU64<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        let n = self.bytes.len();
        if self.k == 0 || n < self.k {
            return None;
        }
        let last_start = n - self.k;
        loop {
            if !self.primed {
                if self.index > last_start {
                    return None;
                }
                let mut acc: u64 = 0;
                let mut i = self.index;
                let stop = i + self.k;
                let mut bad: Option<usize> = None;
                while i < stop {
                    match base_code(self.bytes[i]) {
                        Some(code) => acc = ((acc << 2) | code as u64) & self.mask,
                        None => {
                            bad = Some(i);
                            break;
                        }
                    }
                    i += 1;
                }
                if let Some(b) = bad {
                    self.index = b + 1;
                    continue;
                }
                self.forward = acc;
                self.primed = true;
                self.index += 1;
                return Some(self.canonical_of(acc));
            } else {
                let new_idx = self.index + self.k - 1;
                if new_idx >= n {
                    return None;
                }
                match base_code(self.bytes[new_idx]) {
                    Some(code) => {
                        self.forward = ((self.forward << 2) | code as u64) & self.mask;
                        self.index += 1;
                        return Some(self.canonical_of(self.forward));
                    }
                    None => {
                        self.primed = false;
                        self.index = new_idx + 1;
                        continue;
                    }
                }
            }
        }
    }
}

/// Encode a fixed-width k-mer string into its packed integer. Returns `None`
/// if the input length differs from [`KMER_K`] or contains ambiguous bases.
pub fn encode_kmer(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    if bytes.len() != KMER_K {
        return None;
    }
    let mut acc: u32 = 0;
    for &b in bytes {
        let code = base_code(b)?;
        acc = ((acc << 2) | code) & KMER_MASK;
    }
    Some(acc)
}

/// Lowercase reverse complement of a nucleotide string. Ambiguous bases
/// pass through (lower-cased) so that callers can still use the result in a
/// substring `position(...)` check; mismatches will simply fail to align.
pub fn reverse_complement_string(elements: &str) -> String {
    let mut out = String::with_capacity(elements.len());
    for ch in elements.chars().rev() {
        out.push(match ch {
            'A' | 'a' => 't',
            'T' | 't' => 'a',
            'U' | 'u' => 'a',
            'C' | 'c' => 'g',
            'G' | 'g' => 'c',
            'N' | 'n' => 'n',
            'R' | 'r' => 'y',
            'Y' | 'y' => 'r',
            'K' | 'k' => 'm',
            'M' | 'm' => 'k',
            'S' | 's' => 's',
            'W' | 'w' => 'w',
            'B' | 'b' => 'v',
            'V' | 'v' => 'b',
            'D' | 'd' => 'h',
            'H' | 'h' => 'd',
            other => other.to_ascii_lowercase(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_complement_is_involutive() {
        for k in [0, 1, 0b10_01_11_00_10_01_11_00] {
            assert_eq!(reverse_complement(reverse_complement(k)), k);
        }
    }

    #[test]
    fn canonical_form_is_strand_minimum() {
        // GAATTC is a palindrome under RC.
        let f = encode_kmer("GAATTCAA").unwrap();
        let rc = reverse_complement(f);
        let (c, _) = canonical(f);
        assert_eq!(c, std::cmp::min(f, rc));
    }

    #[test]
    fn iter_skips_ambiguous_windows() {
        let s = "AAAAAAAANCCCCCCCC";
        let positions: Vec<_> = canonical_kmers(s).map(|h| h.position).collect();
        // 9..17 should resume after the 'N' at index 8.
        assert!(positions.iter().all(|&p| p != 1 && p != 7));
        assert!(positions.contains(&0));
        assert!(positions.contains(&9));
    }

    #[test]
    fn empty_or_short_input_yields_nothing() {
        assert_eq!(canonical_kmers("").count(), 0);
        assert_eq!(canonical_kmers("ACGT").count(), 0);
    }

    #[test]
    fn rc_string_round_trip() {
        let s = "GAATTC";
        let rc = reverse_complement_string(s);
        let rcrc = reverse_complement_string(&rc);
        assert_eq!(rcrc.to_ascii_uppercase(), s);
    }
}
