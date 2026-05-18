//! Criterion benchmarks for `content_hash`. The hash is on the write path
//! for every object, so a regression in `canonical_line` rendering or the
//! sort-and-feed loop directly shows up as ingest slowdown.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use sbol::{Document, RdfFormat};
use sbol_db_rdf::content_hash;

fn make_fixture(triples_target: usize) -> Vec<sbol::Triple> {
    let base = include_str!("../../sbol-db-postgres/tests/fixtures/simple_component.ttl");
    let doc = Document::read(base, RdfFormat::Turtle).expect("parse fixture");
    let one = doc.rdf_graph().triples().to_vec();
    let mut out = Vec::with_capacity(triples_target);
    while out.len() < triples_target {
        out.extend(one.iter().cloned());
    }
    out.truncate(triples_target);
    out
}

fn bench_content_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("content_hash");
    for size in [100usize, 1_000, 10_000] {
        let triples = make_fixture(size);
        group.throughput(Throughput::Elements(triples.len() as u64));
        group.bench_function(format!("{size}_triples"), |b| {
            b.iter(|| content_hash(black_box(&triples)))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_content_hash);
criterion_main!(benches);
