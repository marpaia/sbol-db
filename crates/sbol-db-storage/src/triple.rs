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

/// A content-addressed term id used by an id-native backend: a fixed-size key
/// derived from the term, so it is stable and unique per term. An id-native
/// SPARQL dataset joins on these instead of materialized terms, materializing a
/// term only for output rows and filter operands.
pub type TermId = [u8; 16];

/// A bound term to resolve to its id, borrowed from the query.
#[derive(Clone, Copy, Debug)]
pub enum TermKey<'a> {
    Iri(&'a str),
    Blank(&'a str),
    Literal {
        value: &'a str,
        datatype: &'a str,
        language: Option<&'a str>,
    },
}

/// The decoded value of a term id.
#[derive(Clone, Debug)]
pub enum TermValue {
    Iri(String),
    Blank(String),
    Literal {
        value: String,
        datatype: String,
        language: Option<String>,
    },
}

/// Graph-position filter for an id-native scan; like [`GraphFilter`] but the
/// named graph is identified by its id.
#[derive(Clone, Copy, Debug)]
pub enum IdGraphFilter {
    AnyNamed,
    DefaultOnly,
    Iri(TermId),
}

/// A quad of term ids from an id-native scan; `graph` is `None` for the default
/// graph.
#[derive(Clone, Copy, Debug)]
pub struct IdQuad {
    pub graph: Option<TermId>,
    pub subject: TermId,
    pub predicate: TermId,
    pub object: TermId,
}
