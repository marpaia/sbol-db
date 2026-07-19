//! Round-trips the MinHash/LSH sketch store over a real SQLite database: a
//! signature and its band buckets persist, band candidate lookup finds the
//! sequence, and a re-index leaves no stale postings.

use sbol_db_search::minhash::{band_hashes, sketch, SketchParams};
use sbol_db_sqlite::{connect_and_migrate, SqliteSketchStore};
use sbol_db_storage::SketchStore;
use tempfile::TempDir;

async fn fresh_store() -> (SqliteSketchStore, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sketch.sqlite");
    let url = format!("sqlite://{}", path.display());
    let pool = connect_and_migrate(&url).await.expect("open + migrate");
    (SqliteSketchStore::new(pool), dir)
}

fn seq(seed: u64, len: usize) -> String {
    let bases = *b"ACGT";
    let mut x = seed.wrapping_add(1);
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        // A tiny xorshift keeps the fixture self-contained.
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.push(bases[(x >> 40) as usize % 4] as char);
    }
    s
}

#[tokio::test]
async fn sketch_persists_and_bands_find_the_sequence() {
    let (store, _dir) = fresh_store().await;
    let params = SketchParams::default();

    let iri = "http://example.org/seq/a";
    let elements = seq(1, 400);
    let signature = sketch(&elements, &params).expect("sketch");
    let bands = band_hashes(&signature, &params);

    store
        .put_sketch(iri, &signature, &bands)
        .await
        .expect("put_sketch");

    // The signature round-trips byte-for-byte.
    assert_eq!(
        store.sketch_of(iri).await.expect("sketch_of"),
        Some(signature.clone())
    );

    // A missing IRI has no sketch.
    assert_eq!(
        store
            .sketch_of("http://example.org/seq/missing")
            .await
            .expect("sketch_of"),
        None
    );

    // Every band the sequence falls into finds it back.
    let candidates = store.candidates_by_bands(&bands).await.expect("candidates");
    assert!(candidates.contains(&iri.to_owned()));

    // A near-identical sequence shares a band and so appears as a candidate.
    let mut near: Vec<u8> = elements.bytes().collect();
    near[100] = if near[100] == b'A' { b'C' } else { b'A' };
    let near = String::from_utf8(near).unwrap();
    let near_sig = sketch(&near, &params).expect("sketch near");
    let near_bands = band_hashes(&near_sig, &params);
    let via_near = store
        .candidates_by_bands(&near_bands)
        .await
        .expect("candidates near");
    assert!(
        via_near.contains(&iri.to_owned()),
        "a one-mismatch neighbour collides in at least one band"
    );

    // all_sketches sees the one stored sketch.
    let all = store.all_sketches().await.expect("all_sketches");
    assert_eq!(all, vec![(iri.to_owned(), signature)]);
}

#[tokio::test]
async fn reindex_replaces_bands_without_stale_postings() {
    let (store, _dir) = fresh_store().await;
    let params = SketchParams::default();
    let iri = "http://example.org/seq/b";

    let first = sketch(&seq(2, 400), &params).expect("first sketch");
    let first_bands = band_hashes(&first, &params);
    store
        .put_sketch(iri, &first, &first_bands)
        .await
        .expect("put first");

    // Re-index with an unrelated sequence: its bands differ, and the old bands
    // must no longer point at the sequence.
    let second = sketch(&seq(777, 400), &params).expect("second sketch");
    let second_bands = band_hashes(&second, &params);
    store
        .put_sketch(iri, &second, &second_bands)
        .await
        .expect("put second");

    assert_eq!(
        store.sketch_of(iri).await.expect("sketch_of"),
        Some(second.clone())
    );

    let stale: Vec<u64> = first_bands
        .iter()
        .copied()
        .filter(|b| !second_bands.contains(b))
        .collect();
    assert!(
        !stale.is_empty(),
        "the two unrelated sketches use different bands"
    );
    let via_stale = store
        .candidates_by_bands(&stale)
        .await
        .expect("candidates stale");
    assert!(
        !via_stale.contains(&iri.to_owned()),
        "old band postings are gone after re-index"
    );
}
