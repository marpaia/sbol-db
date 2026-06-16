//! Triple-pattern scan inputs shared by the storage contract.

/// Filter on the named-graph position for a pattern scan.
///
/// Mirrors SPARQL `graph_name` semantics: `None` (no filter at the call site)
/// means any graph including the default graph, `AnyNamed` is any named graph
/// (default graph excluded), `DefaultOnly` is the default graph only, and
/// `Iri(g)` is a specific named graph.
#[derive(Clone, Debug)]
pub enum GraphFilter {
    AnyNamed,
    DefaultOnly,
    Iri(String),
}

/// A bound subject position in a triple pattern.
#[derive(Clone, Debug)]
pub enum PatternSubject {
    Iri(String),
    Blank(String),
}

/// A bound object position in a triple pattern.
#[derive(Clone, Debug)]
pub enum PatternObject {
    Iri(String),
    Blank(String),
    Literal {
        value: String,
        datatype: String,
        language: Option<String>,
    },
}
