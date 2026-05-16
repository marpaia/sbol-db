use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{DocumentId, ObjectId};
use crate::iri::IriString;
use crate::validation::ValidationStatus;

/// RDF serialization formats `sbol-db` understands. Matches the
/// `serialization_format` CHECK in `sbol_documents`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerializationFormat {
    Json,
    JsonLd,
    RdfXml,
    Turtle,
    TriG,
    NTriples,
    NQuads,
}

impl SerializationFormat {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::JsonLd => "jsonld",
            Self::RdfXml => "rdfxml",
            Self::Turtle => "turtle",
            Self::TriG => "trig",
            Self::NTriples => "ntriples",
            Self::NQuads => "nquads",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "jsonld" => Some(Self::JsonLd),
            "rdf" | "xml" | "rdfxml" => Some(Self::RdfXml),
            "ttl" | "turtle" => Some(Self::Turtle),
            "trig" => Some(Self::TriG),
            "nt" | "ntriples" => Some(Self::NTriples),
            "nq" | "nquads" => Some(Self::NQuads),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDocument {
    pub document_iri: Option<IriString>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub serialization_format: SerializationFormat,
    pub source_uri: Option<String>,
    pub raw_payload: Option<serde_json::Value>,
    pub content_hash: Vec<u8>,
    pub created_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub id: DocumentId,
    pub document_iri: Option<IriString>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub serialization_format: SerializationFormat,
    pub source_uri: Option<String>,
    pub content_hash: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SbolObjectRecord {
    pub id: ObjectId,
    pub iri: IriString,
    pub sbol_class: String,
    pub display_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub document_id: Option<DocumentId>,
    pub types: Vec<String>,
    pub roles: Vec<String>,
    pub data: serde_json::Value,
    pub content_hash: Vec<u8>,
}

/// Lightweight per-object summary extracted from a parsed sbol::Document.
/// Repositories fill in `id` and `document_id` before insert.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSummary {
    pub iri: IriString,
    pub sbol_class: String,
    pub display_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub types: Vec<String>,
    pub roles: Vec<String>,
    pub data: serde_json::Value,
    pub content_hash: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub document_id: DocumentId,
    pub object_count: usize,
    pub quad_count: usize,
    pub validation_status: ValidationStatus,
    pub validation_issue_count: usize,
}
