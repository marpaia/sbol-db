//! Shared parsing helpers for CLI flag values: serialization formats and
//! traversal direction. Pulled out of `main.rs` so every subcommand that
//! needs them refers to one canonical implementation.

use anyhow::{anyhow, Result};
use sbol::RdfFormat;
use sbol_db_core::{Direction, SerializationFormat};

/// Resolve an `--format <s>` flag or, if `None`, infer from the path's
/// extension. Returns an error if neither identifies a known format.
pub fn resolve_format(
    explicit: Option<&str>,
    path: &std::path::Path,
) -> Result<SerializationFormat> {
    if let Some(f) = explicit {
        return parse_format(f).ok_or_else(|| anyhow!("unknown format: {f}"));
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("could not infer format from path {}", path.display()))?;
    SerializationFormat::from_extension(ext).ok_or_else(|| anyhow!("unknown extension: {ext}"))
}

pub fn parse_format(s: &str) -> Option<SerializationFormat> {
    match s.to_ascii_lowercase().as_str() {
        "turtle" | "ttl" => Some(SerializationFormat::Turtle),
        "jsonld" => Some(SerializationFormat::JsonLd),
        "rdfxml" | "rdf" | "xml" => Some(SerializationFormat::RdfXml),
        "ntriples" | "nt" => Some(SerializationFormat::NTriples),
        "nquads" | "nq" => Some(SerializationFormat::NQuads),
        "trig" => Some(SerializationFormat::TriG),
        "json" => Some(SerializationFormat::Json),
        _ => None,
    }
}

/// Map a `SerializationFormat` to the upstream `sbol::RdfFormat`. Mirrors
/// the conversion in `sbol-db-postgres::service::to_rdf_format`: the
/// upstream parser only handles Turtle/JsonLd/RdfXml/NTriples, so NQuads
/// and TriG (and the synthetic `Json` projection) reject here as inputs.
pub fn serialization_to_rdf_format(format: SerializationFormat) -> Result<RdfFormat> {
    match format {
        SerializationFormat::Turtle => Ok(RdfFormat::Turtle),
        SerializationFormat::JsonLd => Ok(RdfFormat::JsonLd),
        SerializationFormat::RdfXml => Ok(RdfFormat::RdfXml),
        SerializationFormat::NTriples => Ok(RdfFormat::NTriples),
        other => Err(anyhow!(
            "format {other:?} is not supported as an input format (upstream sbol parser limitation)"
        )),
    }
}

pub fn parse_direction(s: &str) -> Result<Direction> {
    match s.to_ascii_lowercase().as_str() {
        "forward" | "out" => Ok(Direction::Forward),
        "backward" | "back" | "in" => Ok(Direction::Backward),
        "both" | "either" => Ok(Direction::Both),
        other => Err(anyhow!("unknown direction: {other}")),
    }
}
