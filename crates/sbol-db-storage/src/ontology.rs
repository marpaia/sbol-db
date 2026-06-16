//! Ontology query results.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct OntologyRecord {
    pub prefix: String,
    pub name: String,
    pub source_url: Option<String>,
    pub version: Option<String>,
    pub term_count: i32,
    pub imported_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OntologyTermRecord {
    pub iri: String,
    pub prefix: String,
    pub curie: String,
    pub name: String,
    pub definition: Option<String>,
    pub is_obsolete: bool,
    pub synonyms: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OntologyLoadReport {
    pub prefix: String,
    pub source_url: Option<String>,
    pub version: Option<String>,
    pub term_count: usize,
    pub closure_count: usize,
    pub alias_count: usize,
}
