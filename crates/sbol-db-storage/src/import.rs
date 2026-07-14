//! Document-import and graph-store write inputs.

use sbol_db_core::{IriString, SerializationFormat};

/// How an import combines with an existing document graph that carries the same
/// `document_iri`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImportOverwrite {
    /// Reject the import if `document_iri` already exists (the `UNIQUE`
    /// constraint surfaces as an error). The default.
    #[default]
    Fail,
    /// Delete the existing graph with this `document_iri`, then import the new
    /// document in its place, atomically.
    Replace,
    /// Union the existing graph's triples with the new document's, then import
    /// the combined document in place of the old one. Objects present in both
    /// versions accumulate the union of their properties; conflicting
    /// single-valued properties surface as a validation error.
    Merge,
}

/// One document to import: its serialized body plus the metadata that drives
/// graph creation and namespace resolution.
pub struct ImportInput {
    pub body: String,
    pub format: SerializationFormat,
    pub namespace: Option<String>,
    pub source_uri: Option<String>,
    pub document_iri: Option<IriString>,
    pub created_by: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub overwrite: ImportOverwrite,
}

/// How a Graph Store write combines with a graph's existing contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphWriteMode {
    /// `POST`: append to the graph (SynBioHub uploads submissions as a sequence
    /// of chunks POSTed to the same graph, so this must accumulate).
    Merge,
    /// `PUT`: replace the graph's entire contents.
    Replace,
}
