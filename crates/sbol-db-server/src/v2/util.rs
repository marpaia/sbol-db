//! Small shared helpers for the V2 resource handlers: JSON body parsing that
//! renders the V2 error envelope, the collision-policy and submission-format
//! parsers, and the percent-encoder that builds a `Location` back into the
//! `/objects/{iri}` route. None of these hold business logic; they are wire glue
//! shared by the collections, objects, and search handlers.

use axum::body::Bytes;
use sbol_db_core::SerializationFormat;
use sbol_db_storage::ImportOverwrite;
use serde::de::DeserializeOwned;

use crate::error::ApiError;
use crate::v2::error::V2Error;

/// Parse a JSON request body into `T`, rendering a `400` in the V2 envelope on
/// malformed JSON. An empty body yields `T::default()`, so a verb with only
/// optional fields accepts an absent body.
pub fn parse_json<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, V2Error> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| V2Error::from(ApiError::BadRequest(format!("invalid JSON body: {e}"))))
}

/// Map the idiomatic `overwrite` policy word to an [`ImportOverwrite`]. Absent
/// defaults to `fail` (reject a collision), matching classic's `overwrite_merge=0`.
pub fn resolve_overwrite(code: Option<&str>) -> Result<ImportOverwrite, V2Error> {
    match code
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("fail")
    {
        "fail" => Ok(ImportOverwrite::Fail),
        "replace" | "overwrite" => Ok(ImportOverwrite::Replace),
        "merge" => Ok(ImportOverwrite::Merge),
        other => Err(V2Error::from(ApiError::BadRequest(format!(
            "invalid overwrite policy: {other}"
        )))),
    }
}

/// Resolve a contribution serialization from an optional `format` hint,
/// defaulting to RDF/XML. GenBank and FASTA are accepted by the application
/// service, which validates and converts them to SBOL 3 before minting.
pub fn resolve_submission_format(hint: Option<&str>) -> Result<SerializationFormat, V2Error> {
    match hint.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(SerializationFormat::RdfXml),
        Some(hint) => match hint.to_ascii_lowercase().as_str() {
            "turtle" | "ttl" => Ok(SerializationFormat::Turtle),
            "jsonld" => Ok(SerializationFormat::JsonLd),
            "rdfxml" | "rdf" | "xml" => Ok(SerializationFormat::RdfXml),
            "ntriples" | "nt" => Ok(SerializationFormat::NTriples),
            "genbank" | "gb" | "gbk" => Ok(SerializationFormat::GenBank),
            "fasta" | "fa" | "fna" | "faa" => Ok(SerializationFormat::Fasta),
            other => Err(V2Error::from(ApiError::BadRequest(format!(
                "unsupported submission format: {other}"
            )))),
        },
    }
}

/// Percent-encode an IRI into a single path segment (RFC 3986 unreserved bytes
/// pass through, everything else is `%XX`) so a built `Location` rides the
/// `/objects/{iri}` route as one capture.
pub fn encode_iri_segment(iri: &str) -> String {
    iri.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A required, non-empty string field, or a `400` in the V2 envelope naming the
/// missing field.
pub fn required(value: Option<String>, field: &str) -> Result<String, V2Error> {
    value
        .filter(|s| !s.is_empty())
        .ok_or_else(|| V2Error::from(ApiError::BadRequest(format!("{field} is required"))))
}

/// A comma-separated citations string split into trimmed, non-empty PubMed ids.
pub fn parse_citations(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}
