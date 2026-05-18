//! Roundtrip tests for `quads_to_rdf` and the underlying N-Triples renderer.
//! Particular focus on the `escape` helper, which has no direct test in the
//! source tree but is the only thing keeping pathological literal content
//! from corrupting output.

use sbol::{Document, RdfFormat};
use sbol_db_core::{IriString, ObjectTerm, Quad, SerializationFormat, SubjectTerm};
use sbol_db_rdf::quads_to_rdf;

fn iri_quad(subject: &str, predicate: &str, object: &str) -> Quad {
    Quad {
        graph_iri: None,
        subject: SubjectTerm::Iri(IriString::unchecked(subject)),
        predicate: IriString::unchecked(predicate),
        object: ObjectTerm::Iri(IriString::unchecked(object)),
    }
}

fn literal_quad(subject: &str, predicate: &str, value: &str) -> Quad {
    Quad {
        graph_iri: None,
        subject: SubjectTerm::Iri(IriString::unchecked(subject)),
        predicate: IriString::unchecked(predicate),
        object: ObjectTerm::Literal {
            value: value.to_owned(),
            datatype: IriString::unchecked("http://www.w3.org/2001/XMLSchema#string"),
            language: None,
        },
    }
}

#[test]
fn ntriples_render_then_reparse_round_trips_iris() {
    let quads = vec![iri_quad(
        "https://example.org/s",
        "https://example.org/p",
        "https://example.org/o",
    )];
    let nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    let graph = sbol_rdf::Graph::parse(&nt, RdfFormat::NTriples).expect("reparse");
    assert_eq!(graph.triples().len(), 1);
    let triple = &graph.triples()[0];
    match &triple.subject {
        sbol::Resource::Iri(iri) => assert_eq!(iri.as_str(), "https://example.org/s"),
        other => panic!("expected IRI subject, got {other:?}"),
    }
    assert_eq!(triple.predicate.as_str(), "https://example.org/p");
}

#[test]
fn literal_with_quote_round_trips() {
    let quads = vec![literal_quad(
        "https://example.org/s",
        "https://example.org/p",
        r#"a "quoted" string"#,
    )];
    let nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    // Output should contain the escaped form.
    assert!(
        nt.contains(r#"\"quoted\""#),
        "expected escaped quotes in {nt:?}"
    );
    // And must reparse back to the original literal value.
    let graph = sbol_rdf::Graph::parse(&nt, RdfFormat::NTriples).expect("reparse");
    assert_eq!(graph.triples().len(), 1);
    match &graph.triples()[0].object {
        sbol::Term::Literal(lit) => assert_eq!(lit.value(), r#"a "quoted" string"#),
        other => panic!("expected literal, got {other:?}"),
    }
}

#[test]
fn literal_with_backslash_round_trips() {
    let value = r"path\to\file";
    let quads = vec![literal_quad(
        "https://example.org/s",
        "https://example.org/p",
        value,
    )];
    let nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    let graph = sbol_rdf::Graph::parse(&nt, RdfFormat::NTriples).expect("reparse");
    assert_eq!(graph.triples().len(), 1);
    match &graph.triples()[0].object {
        sbol::Term::Literal(lit) => assert_eq!(lit.value(), value),
        other => panic!("expected literal, got {other:?}"),
    }
}

#[test]
fn literal_with_newline_and_tab_round_trips() {
    let value = "line1\nline2\tindented\rmore";
    let quads = vec![literal_quad(
        "https://example.org/s",
        "https://example.org/p",
        value,
    )];
    let nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    // No raw newlines/tabs may leak through into the rendered triple line.
    let body = nt.trim_end_matches('\n');
    assert!(
        !body.contains('\n') && !body.contains('\t') && !body.contains('\r'),
        "unescaped control char in {body:?}"
    );
    let graph = sbol_rdf::Graph::parse(&nt, RdfFormat::NTriples).expect("reparse");
    match &graph.triples()[0].object {
        sbol::Term::Literal(lit) => assert_eq!(lit.value(), value),
        other => panic!("expected literal, got {other:?}"),
    }
}

#[test]
fn literal_with_non_bmp_codepoint_round_trips() {
    let value = "emoji: \u{1F600} mathematical: \u{1D11E}";
    let quads = vec![literal_quad(
        "https://example.org/s",
        "https://example.org/p",
        value,
    )];
    let nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    let graph = sbol_rdf::Graph::parse(&nt, RdfFormat::NTriples).expect("reparse");
    match &graph.triples()[0].object {
        sbol::Term::Literal(lit) => assert_eq!(lit.value(), value),
        other => panic!("expected literal, got {other:?}"),
    }
}

#[test]
fn language_tagged_literal_round_trips() {
    let quads = vec![Quad {
        graph_iri: None,
        subject: SubjectTerm::Iri(IriString::unchecked("https://example.org/s")),
        predicate: IriString::unchecked("http://www.w3.org/2000/01/rdf-schema#label"),
        object: ObjectTerm::Literal {
            value: "hello".to_owned(),
            datatype: IriString::unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"),
            language: Some("en".to_owned()),
        },
    }];
    let nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    assert!(nt.contains("\"hello\"@en"), "got {nt}");
    let graph = sbol_rdf::Graph::parse(&nt, RdfFormat::NTriples).expect("reparse");
    match &graph.triples()[0].object {
        sbol::Term::Literal(lit) => {
            assert_eq!(lit.value(), "hello");
            assert_eq!(lit.language(), Some("en"));
        }
        other => panic!("expected literal, got {other:?}"),
    }
}

#[test]
fn blank_node_subject_round_trips_through_ntriples() {
    let quads = vec![Quad {
        graph_iri: None,
        subject: SubjectTerm::BlankNode("b0".to_owned()),
        predicate: IriString::unchecked("https://example.org/p"),
        object: ObjectTerm::BlankNode("b1".to_owned()),
    }];
    let nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    assert!(nt.contains("_:b0"));
    assert!(nt.contains("_:b1"));
    let graph = sbol_rdf::Graph::parse(&nt, RdfFormat::NTriples).expect("reparse");
    assert_eq!(graph.triples().len(), 1);
}

const FIXTURE: &str = include_str!("../../sbol-db-postgres/tests/fixtures/simple_component.ttl");

#[test]
fn turtle_export_is_stable_under_repeat_application() {
    // Render the fixture → quads → NTriples → Turtle, then re-parse the Turtle
    // and re-render. The N-Triples form (canonical, sortable) must match.
    let doc = Document::read(FIXTURE, RdfFormat::Turtle).expect("parse");
    let graph_iri = IriString::unchecked("https://example.org/g/abc");
    let quads = sbol_db_rdf::document_to_quads(&doc, &graph_iri);

    let nt_first = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("nt first");
    let turtle = quads_to_rdf(&quads, SerializationFormat::Turtle).expect("turtle");
    let doc2 = Document::read(&turtle, RdfFormat::Turtle).expect("reparse turtle");
    let quads2 = sbol_db_rdf::document_to_quads(&doc2, &graph_iri);
    let nt_second = quads_to_rdf(&quads2, SerializationFormat::NTriples).expect("nt second");

    let mut first_lines: Vec<_> = nt_first.lines().collect();
    let mut second_lines: Vec<_> = nt_second.lines().collect();
    first_lines.sort_unstable();
    second_lines.sort_unstable();
    assert_eq!(first_lines, second_lines, "Turtle round-trip lost a triple");
}

#[test]
fn ntriples_snapshot_of_fixture_is_stable() {
    let doc = Document::read(FIXTURE, RdfFormat::Turtle).expect("parse");
    let graph_iri = IriString::unchecked("https://example.org/g/abc");
    let quads = sbol_db_rdf::document_to_quads(&doc, &graph_iri);
    let mut nt = quads_to_rdf(&quads, SerializationFormat::NTriples).expect("render");
    // Sort lines for a stable snapshot (the renderer's output order is the
    // input order, which we treat as not load-bearing here).
    let mut lines: Vec<&str> = nt.lines().collect();
    lines.sort_unstable();
    nt = lines.join("\n");
    insta::assert_snapshot!("simple_component_ntriples", nt);
}
