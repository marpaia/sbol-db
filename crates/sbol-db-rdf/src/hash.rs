use sbol_rdf::{Resource, Term, Triple};
use sha3::{Digest, Sha3_256};

/// SHA3-256 of arbitrary bytes.
pub fn hash_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha3_256::new();
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

/// Content hash for a set of RDF triples, computed over the sorted canonical
/// N-Triples-like rendering so the same logical graph always produces the
/// same digest regardless of input order.
pub fn content_hash(triples: &[Triple]) -> Vec<u8> {
    let mut lines: Vec<String> = triples.iter().map(canonical_line).collect();
    lines.sort_unstable();
    let mut hasher = Sha3_256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_vec()
}

fn canonical_line(triple: &Triple) -> String {
    format!(
        "{} <{}> {} .",
        render_resource(&triple.subject),
        triple.predicate.as_str(),
        render_term(&triple.object),
    )
}

fn render_resource(resource: &Resource) -> String {
    match resource {
        Resource::Iri(iri) => format!("<{}>", iri.as_str()),
        Resource::BlankNode(node) => format!("_:{}", node.as_str()),
        _ => format!("_:{resource}"),
    }
}

fn render_term(term: &Term) -> String {
    match term {
        Term::Resource(resource) => render_resource(resource),
        Term::Literal(literal) => {
            let value = escape_literal(literal.value());
            match literal.language() {
                Some(lang) => format!("\"{value}\"@{lang}"),
                None => format!("\"{value}\"^^<{}>", literal.datatype().as_str()),
            }
        }
        _ => "\"\"".to_owned(),
    }
}

fn escape_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}
