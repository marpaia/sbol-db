//! Criterion benchmarks for SPARQL query parsing. We bench the underlying
//! `spargebra::SparqlParser` directly — that's the heavy lift wrapped by
//! the engine's `parse_query_strict`. The point of this bench is to catch
//! parser-cost regressions when `spargebra` is upgraded.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use spargebra::SparqlParser;

const SIMPLE_SELECT: &str = r#"
PREFIX sbol: <http://sbols.org/v3#>
SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 100
"#;

const FILTERED_SELECT: &str = r#"
PREFIX sbol: <http://sbols.org/v3#>
PREFIX SO:   <https://identifiers.org/SO:>
SELECT ?comp ?seq WHERE {
    ?comp a sbol:Component ;
          sbol:role SO:0000167 ;
          sbol:hasSequence ?seq .
    ?seq sbol:elements ?elements .
    FILTER(STRLEN(?elements) > 100)
}
LIMIT 1000
"#;

const CONSTRUCT_QUERY: &str = r#"
PREFIX sbol: <http://sbols.org/v3#>
CONSTRUCT {
    ?a sbol:partOf ?b .
} WHERE {
    ?a sbol:hasComponent ?b .
}
"#;

const ASK_QUERY: &str = r#"
PREFIX sbol: <http://sbols.org/v3#>
ASK { ?s a sbol:Component }
"#;

const NESTED_OPTIONAL: &str = r#"
PREFIX sbol: <http://sbols.org/v3#>
SELECT ?s ?name ?desc ?role WHERE {
    ?s a sbol:Component .
    OPTIONAL {
        ?s sbol:name ?name .
        OPTIONAL { ?s sbol:description ?desc }
    }
    OPTIONAL {
        ?s sbol:role ?role .
        FILTER(ISIRI(?role))
    }
}
"#;

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("sparql_parse");
    for (name, query) in [
        ("simple_select", SIMPLE_SELECT),
        ("filtered_select", FILTERED_SELECT),
        ("construct", CONSTRUCT_QUERY),
        ("ask", ASK_QUERY),
        ("nested_optional", NESTED_OPTIONAL),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                SparqlParser::new()
                    .parse_query(black_box(query))
                    .expect("parse")
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
