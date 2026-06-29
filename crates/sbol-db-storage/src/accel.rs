//! The SynBioHub query accelerator contract.
//!
//! SynBioHub issues a fixed set of SPARQL templates whose results are, in
//! effect, "enumerate a set of top-level objects and return a metadata bundle
//! per object, with counts and facets." A backend that supports the accelerator
//! answers these from purpose-built indexes (range scans + point lookups) rather
//! than evaluating the graph pattern, with the expensive parts (counts, the
//! membership anti-join) precomputed at write time.
//!
//! The recognizer in `sbol-db-sparql` turns a parsed query into an
//! [`AcceleratedQuery`]; the backend returns [`AccelSolutions`]. Recognition is
//! best-effort: anything not recognized, or any backend without accelerator
//! support, falls back to the generic SPARQL engine, so results never depend on
//! the accelerator being used.

use crate::TermValue;

/// A metadata field an accelerated projection can return for an object. Maps a
/// SELECT variable to the value the accelerator fills it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The object IRI (`?subject`/`?uri`).
    Subject,
    DisplayId,
    Version,
    Name,
    Description,
    /// An `rdf:type` of the object (multi-valued ⇒ one row per type).
    Type,
    /// A `sbol2:type` restricted to BioPAX (multi-valued, optional).
    SbolType,
    /// A `sbol2:role` restricted to the Sequence Ontology (multi-valued, optional).
    Role,
}

/// Which objects an accelerated query ranges over.
#[derive(Clone, Debug)]
pub enum Scope {
    /// Every top-level object (`?s sbh:topLevel ?s`).
    TopLevel,
    /// Every object with a given `rdf:type` (not restricted to top-level), e.g.
    /// `Count` over `ComponentDefinition`, or `getCollections` over `Collection`.
    ByType(String),
    /// Members of a collection. With `root_only`, only members not referenced by
    /// another member (directly or via a child) — SynBioHub's "top-level members"
    /// view, whose `FILTER NOT EXISTS` anti-join is precomputed at derive time.
    Collection { collection: String, root_only: bool },
}

/// A distinct-value facet over top-level objects.
#[derive(Clone, Copy, Debug)]
pub enum FacetKind {
    /// Distinct `rdf:type` values (`getTypes`).
    Types,
    /// Distinct `sbol2:role` values (`getRoles`).
    Roles,
    /// Distinct `dc:creator` values (`getCreators`).
    Creators,
}

/// A recognized SynBioHub query resolved to accelerator parameters.
#[derive(Clone, Debug)]
pub enum AcceleratedQuery {
    /// List objects in `scope` with a per-object metadata projection, ordered by
    /// displayId, paginated. Reproduces the template's `SELECT DISTINCT` over the
    /// multi-valued `type`/`sbolType`/`role` columns (one row per combination).
    ObjectList {
        graph: String,
        scope: Scope,
        projection: Vec<(String, Field)>,
        offset: usize,
        limit: Option<usize>,
        /// A `STRSTARTS(str(?subject), prefix)` filter from the template (the
        /// member-namespace filter on collection-member queries), if present.
        subject_prefix: Option<String>,
    },
    /// Count distinct objects in `scope` (`Count`, `searchCount`). `var` is the
    /// count's result variable.
    Count {
        graph: String,
        scope: Scope,
        var: String,
        subject_prefix: Option<String>,
    },
    /// Distinct facet values over top-level objects (`getTypes`/`getRoles`/
    /// `getCreators`). `var` is the projected variable.
    Facet {
        graph: String,
        kind: FacetKind,
        var: String,
    },
}

/// A backend's answer to an [`AcceleratedQuery`]: a SPARQL solution sequence.
/// `vars` are the projected variable names (without `?`); each row has one cell
/// per variable, `None` for an unbound (optional) value.
#[derive(Clone, Debug, Default)]
pub struct AccelSolutions {
    pub vars: Vec<String>,
    pub rows: Vec<Vec<Option<TermValue>>>,
}
