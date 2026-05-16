use sbol_db_core::{
    DomainError, EdgeInfo, EdgeObject, IriString, NeighborhoodResult, ObjectTerm, Quad,
    SerializationFormat, SubjectTerm,
};

/// Convert traversal edges back into the domain `Quad` shape so they can be
/// re-serialized via [`quads_to_rdf`].
pub fn neighborhood_to_quads(result: &NeighborhoodResult) -> Vec<Quad> {
    result.edges.iter().map(edge_to_quad).collect()
}

/// Serialize a [`NeighborhoodResult`]'s edges directly as an RDF document.
pub fn neighborhood_to_rdf(
    result: &NeighborhoodResult,
    format: SerializationFormat,
) -> Result<String, DomainError> {
    let quads = neighborhood_to_quads(result);
    quads_to_rdf(&quads, format)
}

fn edge_to_quad(edge: &EdgeInfo) -> Quad {
    let subject = if edge.subject_is_blank {
        SubjectTerm::BlankNode(edge.subject.clone())
    } else {
        SubjectTerm::Iri(IriString::unchecked(&edge.subject))
    };
    let object = match &edge.object {
        EdgeObject::Iri { value } => ObjectTerm::Iri(IriString::unchecked(value)),
        EdgeObject::BlankNode { value } => ObjectTerm::BlankNode(value.clone()),
        EdgeObject::Literal {
            value,
            datatype,
            language,
        } => ObjectTerm::Literal {
            value: value.clone(),
            datatype: IriString::unchecked(datatype),
            language: language.clone(),
        },
    };
    Quad {
        graph_iri: None,
        subject,
        predicate: IriString::unchecked(&edge.predicate),
        object,
    }
}

/// Re-serialize a quad set as an RDF document in the requested format. The
/// quads are first rendered to N-Triples (the canonical lossless wire form
/// for our subject-position blank nodes) then re-parsed by `sbol-rdf` for
/// non-N-Triples formats.
pub fn quads_to_rdf(quads: &[Quad], format: SerializationFormat) -> Result<String, DomainError> {
    let ntriples = render_ntriples(quads);
    if matches!(format, SerializationFormat::NTriples) {
        return Ok(ntriples);
    }
    let graph = sbol_rdf::Graph::parse(&ntriples, sbol_rdf::RdfFormat::NTriples)
        .map_err(|e| DomainError::Serialization(e.to_string()))?;
    let rdf_format = match format {
        SerializationFormat::Turtle => sbol_rdf::RdfFormat::Turtle,
        SerializationFormat::JsonLd => sbol_rdf::RdfFormat::JsonLd,
        SerializationFormat::RdfXml => sbol_rdf::RdfFormat::RdfXml,
        SerializationFormat::NTriples => sbol_rdf::RdfFormat::NTriples,
        other => {
            return Err(DomainError::InvalidInput(format!(
                "export format {other:?} not supported"
            )))
        }
    };
    graph
        .write(rdf_format)
        .map_err(|e| DomainError::Serialization(e.to_string()))
}

fn render_ntriples(quads: &[Quad]) -> String {
    let mut out = String::new();
    for q in quads {
        let subject = match &q.subject {
            SubjectTerm::Iri(iri) => format!("<{}>", iri.as_str()),
            SubjectTerm::BlankNode(node) => format!("_:{}", node),
        };
        let predicate = format!("<{}>", q.predicate.as_str());
        let object = match &q.object {
            ObjectTerm::Iri(iri) => format!("<{}>", iri.as_str()),
            ObjectTerm::BlankNode(node) => format!("_:{}", node),
            ObjectTerm::Literal {
                value,
                datatype,
                language,
            } => {
                let escaped = escape(value);
                if let Some(lang) = language {
                    format!("\"{escaped}\"@{lang}")
                } else {
                    format!("\"{escaped}\"^^<{}>", datatype.as_str())
                }
            }
        };
        out.push_str(&subject);
        out.push(' ');
        out.push_str(&predicate);
        out.push(' ');
        out.push_str(&object);
        out.push_str(" .\n");
    }
    out
}

fn escape(value: &str) -> String {
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
