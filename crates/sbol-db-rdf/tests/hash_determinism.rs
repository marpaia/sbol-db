//! Tests that pin `content_hash` to its documented contract: deterministic,
//! order-independent, stable across runs. Change detection downstream depends
//! on the hash being a pure function of the unordered triple set.

use sbol::{Document, RdfFormat};
use sbol_db_rdf::content_hash;

const FIXTURE: &str = include_str!("../../sbol-db-postgres/tests/fixtures/simple_component.ttl");

fn fixture_triples() -> Vec<sbol::Triple> {
    let doc = Document::read(FIXTURE, RdfFormat::Turtle).expect("parse fixture");
    doc.rdf_graph().triples().to_vec()
}

#[test]
fn hash_is_deterministic_across_runs() {
    let triples = fixture_triples();
    let first = content_hash(&triples);
    for _ in 0..100 {
        assert_eq!(content_hash(&triples), first);
    }
}

#[test]
fn hash_is_order_independent() {
    let triples = fixture_triples();
    let expected = content_hash(&triples);

    // Reverse.
    let reversed: Vec<_> = triples.iter().rev().cloned().collect();
    assert_eq!(content_hash(&reversed), expected, "reversed order");

    // Deterministic shuffle via a fixed-seed LCG so the test is reproducible
    // without pulling in `rand` as a dev-dep.
    let mut shuffled = triples.clone();
    let mut state: u64 = 0xdead_beef_cafe_babe;
    for i in (1..shuffled.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = (state >> 33) as usize % (i + 1);
        shuffled.swap(i, j);
    }
    assert_eq!(content_hash(&shuffled), expected, "shuffled order");
}

#[test]
fn hash_changes_when_triple_changes() {
    let mut triples = fixture_triples();
    let first = content_hash(&triples);
    // Drop one triple; the hash must change.
    triples.pop();
    let second = content_hash(&triples);
    assert_ne!(first, second, "removing a triple must change the hash");
}

#[test]
fn hash_of_empty_graph_is_stable() {
    let h = content_hash(&[]);
    insta::assert_snapshot!("empty_graph_hash", hex::encode(&h));
}

#[test]
fn hash_of_fixture_is_stable() {
    let triples = fixture_triples();
    let h = content_hash(&triples);
    // Snapshot the hex hash. If `canonical_line` or the digest changes, this
    // diff makes the breakage explicit.
    insta::assert_snapshot!("simple_component_hash", hex::encode(&h));
}
