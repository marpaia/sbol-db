//! Document-import and graph-store write inputs.

use sbol_db_core::{IriString, SerializationFormat};

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
