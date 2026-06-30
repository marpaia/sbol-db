//! Pure SBOL ingest derivation for sbol-db.
//!
//! A storage backend's job at import time splits in two: decide *what* to
//! persist (parse the submitted body, derive its triples, the typed SBOL
//! view, and a validation run) and then *write* it. This crate owns the first
//! half as pure functions over owned data, so every backend derives an
//! identical [`ImportPlan`] and differs only in how it commits it. No type
//! here names a database.

mod import;
mod ontology;

pub use import::{build_import_plan, parse_import_document, to_rdf_format, ImportPlan};
pub use ontology::{build_ontology_plan, OntologyPlan, OntologyTermRow};
