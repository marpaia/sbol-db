//! IRIs used during projection. Kept as `&'static str` to mirror the
//! upstream `sbol::vocab` module (whose constants are crate-private).

pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const SBOL_TYPE: &str = "http://sbols.org/v3#type";
pub const SBOL_ROLE: &str = "http://sbols.org/v3#role";
