//! Property-based tests for invariants in `sbol-db-core` that, if broken,
//! cause silent data corruption (k-mer canonicalization, IRI shape,
//! serialization-format extension mapping, ID newtype roundtrips).

use proptest::prelude::*;
use sbol_db_core::kmer::{
    canonical, canonical_kmers, encode_kmer, reverse_complement, reverse_complement_string,
    KmerStrand, KMER_BITS, KMER_K, KMER_MASK,
};
use sbol_db_core::{
    GraphId, IriString, IriValidationError, JobId, ObjectId, SerializationFormat, ValidationRunId,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// k-mer invariants
// ---------------------------------------------------------------------------

prop_compose! {
    fn arb_kmer()(k in any::<u32>().prop_map(|k| k & KMER_MASK)) -> u32 { k }
}

fn arb_acgt_string(min_len: usize, max_len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(b"ACGT".to_vec()), min_len..=max_len)
        .prop_map(|v| String::from_utf8(v).unwrap())
}

fn arb_acgtn_string(min_len: usize, max_len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(b"ACGTN".to_vec()), min_len..=max_len)
        .prop_map(|v| String::from_utf8(v).unwrap())
}

/// IUPAC alphabet *excluding* U/u, because `reverse_complement_string`
/// deliberately collapses U to A's complement (T) — useful for RNA/DNA
/// cross-matching but not involutive on the U-bearing alphabet.
fn arb_iupac_no_u(min_len: usize, max_len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(b"ACGTNRYKMSWBVDHacgtnrykmswbvdh".to_vec()),
        min_len..=max_len,
    )
    .prop_map(|v| String::from_utf8(v).unwrap())
}

proptest! {
    /// `reverse_complement` is involutive on the masked k-mer space.
    #[test]
    fn rc_is_involutive(k in arb_kmer()) {
        prop_assert_eq!(reverse_complement(reverse_complement(k)), k);
    }

    /// `canonical(k)` returns `min(k, rc(k))` paired with the strand it came from.
    #[test]
    fn canonical_is_strand_minimum(k in arb_kmer()) {
        let rc = reverse_complement(k);
        let (c, strand) = canonical(k);
        prop_assert_eq!(c, std::cmp::min(k, rc));
        if k <= rc {
            prop_assert_eq!(strand, KmerStrand::Forward);
        } else {
            prop_assert_eq!(strand, KmerStrand::Reverse);
        }
    }

    /// For any ACGT string of length ≥ K, the iterator emits exactly
    /// `len - K + 1` k-mers and every position is in range.
    #[test]
    fn iter_emits_every_window_on_clean_input(s in arb_acgt_string(KMER_K, 200)) {
        let hits: Vec<_> = canonical_kmers(&s).collect();
        prop_assert_eq!(hits.len(), s.len() - KMER_K + 1);
        for h in &hits {
            prop_assert!(h.position + KMER_K <= s.len());
            prop_assert!(h.canonical <= KMER_MASK);
        }
    }

    /// Windows containing `N` (or any non-ACGTU base) are skipped: the
    /// substring underneath each emitted hit must contain no ambiguous bases.
    #[test]
    fn iter_skips_windows_with_ambiguous_bases(s in arb_acgtn_string(KMER_K, 200)) {
        for h in canonical_kmers(&s) {
            let window = &s[h.position..h.position + KMER_K];
            for c in window.chars() {
                prop_assert!(
                    matches!(c, 'A' | 'C' | 'G' | 'T'),
                    "emitted window {:?} at pos {} contains ambiguous base {:?}",
                    window, h.position, c
                );
            }
        }
    }

    /// The sliding-window iterator and the one-shot `encode_kmer` agree on the
    /// forward integer for every window. This is the test that catches a bug
    /// in the primed/unprimed branch of `CanonicalIter`.
    #[test]
    fn sliding_matches_encode_kmer(s in arb_acgt_string(KMER_K, 100)) {
        let hits: Vec<_> = canonical_kmers(&s).collect();
        for h in hits {
            let window = &s[h.position..h.position + KMER_K];
            let forward = encode_kmer(window).expect("clean ACGT window encodes");
            let rc = reverse_complement(forward);
            let expected_canonical = std::cmp::min(forward, rc);
            prop_assert_eq!(h.canonical, expected_canonical);
            let expected_strand = if forward <= rc { KmerStrand::Forward } else { KmerStrand::Reverse };
            prop_assert_eq!(h.strand, expected_strand);
        }
    }

    /// `reverse_complement_string` is involutive (modulo case) over the IUPAC
    /// alphabet *excluding U*: every code in the table maps to a partner that
    /// maps back to itself, so a double application returns the input.
    /// U is intentionally collapsed to A's complement (T) so RNA inputs can
    /// align against DNA — that's a one-way mapping and not part of the
    /// involution claim.
    #[test]
    fn rc_string_is_involutive(s in arb_iupac_no_u(0, 100)) {
        let rc = reverse_complement_string(&s);
        let rcrc = reverse_complement_string(&rc);
        prop_assert!(rcrc.eq_ignore_ascii_case(&s),
            "rc(rc({:?})) = {:?}", s, rcrc);
    }

    /// U/u is intentionally mapped to A's complement (T) under rev-comp, so
    /// a single application turns RNA into DNA. From there on, the function
    /// *is* involutive — so `rc(rc(rc(s))) == rc(s)` for any RNA/DNA input.
    #[test]
    fn rc_string_stabilizes_after_one_pass(s in "[ACGTUacgtu]{0,50}") {
        let once = reverse_complement_string(&s);
        prop_assert!(
            !once.contains('U') && !once.contains('u'),
            "single rev-comp must eliminate U/u; got {:?} from {:?}", once, s
        );
        let thrice = reverse_complement_string(&reverse_complement_string(&once));
        prop_assert_eq!(thrice, once);
    }

    /// `encode_kmer` is the identity inverse of "decode": building a k-mer
    /// from its 2-bit codes and re-encoding lands on the same integer.
    #[test]
    fn encode_kmer_roundtrips_through_string(k in arb_kmer()) {
        // Decode k into its 8 bases (most-significant first).
        let mut bases = [b'A'; KMER_K];
        for (i, slot) in bases.iter_mut().enumerate() {
            let shift = (KMER_K - 1 - i) * 2;
            let code = (k >> shift) & 0b11;
            *slot = match code {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                3 => b'T',
                _ => unreachable!(),
            };
        }
        let s = std::str::from_utf8(&bases).unwrap();
        prop_assert_eq!(encode_kmer(s), Some(k));
    }
}

/// `KMER_BITS` and `KMER_MASK` are coherent (compile-time relation made
/// explicit so an accidental edit to one without the other fails fast).
#[test]
fn kmer_constants_are_coherent() {
    assert_eq!(KMER_BITS, KMER_K * 2);
    assert_eq!(KMER_MASK as u64, (1u64 << KMER_BITS) - 1);
}

/// `encode_kmer` rejects strings of the wrong length and strings with
/// ambiguous bases.
#[test]
fn encode_kmer_rejects_invalid_inputs() {
    assert_eq!(encode_kmer(""), None);
    assert_eq!(encode_kmer("ACGT"), None);
    assert_eq!(encode_kmer("ACGTACGTA"), None);
    assert_eq!(encode_kmer("ACGTNCGT"), None);
    assert_eq!(encode_kmer("ACGT NCGT"), None);
}

// ---------------------------------------------------------------------------
// IRI invariants
// ---------------------------------------------------------------------------

prop_compose! {
    fn arb_scheme()(
        head in prop::sample::select((b'a'..=b'z').chain(b'A'..=b'Z').collect::<Vec<u8>>()),
        tail in prop::collection::vec(
            prop::sample::select(
                (b'a'..=b'z')
                    .chain(b'A'..=b'Z')
                    .chain(b'0'..=b'9')
                    .chain([b'+', b'.', b'-'])
                    .collect::<Vec<u8>>(),
            ),
            0..16,
        ),
    ) -> String {
        let mut bytes = Vec::with_capacity(1 + tail.len());
        bytes.push(head);
        bytes.extend(tail);
        String::from_utf8(bytes).unwrap()
    }
}

proptest! {
    /// Any well-formed `scheme:body` (scheme matching the regex, non-empty
    /// body) must validate.
    #[test]
    fn well_formed_iris_are_accepted(scheme in arb_scheme(), body in "[^\\x00-\\x1f]+") {
        let iri = format!("{scheme}:{body}");
        prop_assert!(IriString::new(iri.clone()).is_ok(),
            "expected {:?} to validate", iri);
    }

    /// Validation does not normalize: a successful round-trip returns the
    /// exact input string.
    #[test]
    fn validation_does_not_normalize(scheme in arb_scheme(), body in "[^\\x00-\\x1f]+") {
        let raw = format!("{scheme}:{body}");
        let iri = IriString::new(raw.clone()).unwrap();
        prop_assert_eq!(iri.as_str(), raw.as_str());
    }

    /// Idempotence: validating an already-validated IRI's body returns the
    /// same value.
    #[test]
    fn validation_is_idempotent(scheme in arb_scheme(), body in "[^\\x00-\\x1f]+") {
        let raw = format!("{scheme}:{body}");
        let first = IriString::new(raw).unwrap();
        let again = IriString::new(first.clone().into_inner()).unwrap();
        prop_assert_eq!(first, again);
    }

    /// Arbitrary byte garbage must not panic the validator. It can return
    /// either `Ok` or `Err`, but never panic.
    #[test]
    fn validator_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        // We only feed valid UTF-8 strings; an invalid UTF-8 byte sequence
        // can't reach `IriString::new` because the type takes a `String`.
        if let Ok(s) = String::from_utf8(bytes) {
            let _ = IriString::new(s);
        }
    }

    /// Arbitrary unicode strings must not panic the validator.
    #[test]
    fn validator_never_panics_on_unicode(s in "\\PC*") {
        let _ = IriString::new(s);
    }
}

#[test]
fn iri_rejects_empty() {
    assert!(matches!(IriString::new(""), Err(IriValidationError::Empty)));
}

#[test]
fn iri_rejects_blank_node() {
    assert!(matches!(
        IriString::new("_:b0"),
        Err(IriValidationError::MissingScheme(_))
    ));
}

#[test]
fn iri_rejects_scheme_only() {
    assert!(IriString::new("https:").is_err());
    assert!(IriString::new("urn:").is_err());
}

#[test]
fn iri_rejects_leading_digit() {
    assert!(IriString::new("1http:foo").is_err());
}

#[test]
fn iri_rejects_invalid_scheme_chars() {
    assert!(IriString::new("ht_tp:foo").is_err());
    assert!(IriString::new("ht tp:foo").is_err());
    assert!(IriString::new("http/x:foo").is_err());
}

// ---------------------------------------------------------------------------
// SerializationFormat extension mapping
// ---------------------------------------------------------------------------

/// Documented extension aliases for `SerializationFormat::from_extension`,
/// paired with the format they must map to.
fn documented_aliases() -> &'static [(&'static str, SerializationFormat)] {
    &[
        ("json", SerializationFormat::Json),
        ("jsonld", SerializationFormat::JsonLd),
        ("rdf", SerializationFormat::RdfXml),
        ("xml", SerializationFormat::RdfXml),
        ("rdfxml", SerializationFormat::RdfXml),
        ("ttl", SerializationFormat::Turtle),
        ("turtle", SerializationFormat::Turtle),
        ("trig", SerializationFormat::TriG),
        ("nt", SerializationFormat::NTriples),
        ("ntriples", SerializationFormat::NTriples),
        ("nq", SerializationFormat::NQuads),
        ("nquads", SerializationFormat::NQuads),
        ("gb", SerializationFormat::GenBank),
        ("gbk", SerializationFormat::GenBank),
        ("genbank", SerializationFormat::GenBank),
        ("fa", SerializationFormat::Fasta),
        ("fasta", SerializationFormat::Fasta),
        ("fna", SerializationFormat::Fasta),
        ("faa", SerializationFormat::Fasta),
    ]
}

#[test]
fn from_extension_is_case_insensitive() {
    for (ext, expected) in documented_aliases() {
        let lower = SerializationFormat::from_extension(&ext.to_ascii_lowercase());
        let upper = SerializationFormat::from_extension(&ext.to_ascii_uppercase());
        let mixed: String = ext
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        let mixed = SerializationFormat::from_extension(&mixed);
        assert_eq!(lower, Some(*expected), "lowercase {ext:?}");
        assert_eq!(upper, Some(*expected), "uppercase {ext:?}");
        assert_eq!(mixed, Some(*expected), "mixed case {ext:?}");
    }
}

#[test]
fn from_extension_aliases_match_table() {
    for (ext, expected) in documented_aliases() {
        assert_eq!(
            SerializationFormat::from_extension(ext),
            Some(*expected),
            "{ext:?} should map to {expected:?}"
        );
    }
}

#[test]
fn from_extension_rejects_unknown() {
    for ext in ["", "yaml", "csv", "tsv", "rdfjson", "owl"] {
        assert_eq!(
            SerializationFormat::from_extension(ext),
            None,
            "{ext:?} should not be recognized"
        );
    }
}

#[test]
fn as_db_str_distinct_per_variant() {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for f in [
        SerializationFormat::Json,
        SerializationFormat::JsonLd,
        SerializationFormat::RdfXml,
        SerializationFormat::Turtle,
        SerializationFormat::TriG,
        SerializationFormat::NTriples,
        SerializationFormat::NQuads,
        SerializationFormat::GenBank,
        SerializationFormat::Fasta,
    ] {
        assert!(
            seen.insert(f.as_db_str()),
            "duplicate as_db_str for {f:?}: {:?}",
            f.as_db_str()
        );
    }
}

// ---------------------------------------------------------------------------
// ID newtype invariants
// ---------------------------------------------------------------------------

fn arb_uuid() -> impl Strategy<Value = Uuid> {
    any::<[u8; 16]>().prop_map(Uuid::from_bytes)
}

macro_rules! id_roundtrip_props {
    ($mod_name:ident, $ty:ty) => {
        mod $mod_name {
            use super::*;

            proptest! {
                #[test]
                fn display_uuid_roundtrips(u in arb_uuid()) {
                    let id: $ty = u.into();
                    let s = id.to_string();
                    let parsed: Uuid = s.parse().unwrap();
                    let from_uuid: $ty = parsed.into();
                    prop_assert_eq!(id, from_uuid);
                }

                #[test]
                fn serde_json_roundtrips(u in arb_uuid()) {
                    let id: $ty = u.into();
                    let value = serde_json::to_value(id).unwrap();
                    let back: $ty = serde_json::from_value(value).unwrap();
                    prop_assert_eq!(id, back);
                }

                #[test]
                fn into_uuid_preserves_bytes(u in arb_uuid()) {
                    let id: $ty = u.into();
                    let back: Uuid = id.into();
                    prop_assert_eq!(u, back);
                }
            }
        }
    };
}

id_roundtrip_props!(graph_id, GraphId);
id_roundtrip_props!(object_id, ObjectId);
id_roundtrip_props!(validation_run_id, ValidationRunId);
id_roundtrip_props!(job_id, JobId);

/// The four ID newtypes must serialize as bare UUID strings (transparent
/// serde representation). This is load-bearing for column shape in Postgres.
#[test]
fn ids_serialize_as_bare_uuid_string() {
    let id = GraphId::new();
    let value = serde_json::to_value(id).unwrap();
    assert!(
        value.is_string(),
        "GraphId must serialize as string, got {value}"
    );
    assert_eq!(value.as_str().unwrap(), id.to_string());
}
