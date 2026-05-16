use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A string that we've checked looks IRI-shaped. Matches the Postgres
/// `sbol_iri` domain at the database boundary; deeper RFC 3987 conformance is delegated
/// to the upstream `sbol-rdf` parser.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IriString(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IriValidationError {
    #[error("iri is empty")]
    Empty,
    #[error("iri has no scheme: {0}")]
    MissingScheme(String),
}

impl IriString {
    pub fn new(value: impl Into<String>) -> Result<Self, IriValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IriValidationError::Empty);
        }
        // Match Postgres iri domain: `^[a-zA-Z][a-zA-Z0-9+.-]*:.+`
        let bytes = value.as_bytes();
        if !bytes[0].is_ascii_alphabetic() {
            return Err(IriValidationError::MissingScheme(value));
        }
        let mut colon = None;
        for (idx, &b) in bytes.iter().enumerate().skip(1) {
            if b == b':' {
                colon = Some(idx);
                break;
            }
            let ok = b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-');
            if !ok {
                return Err(IriValidationError::MissingScheme(value));
            }
        }
        match colon {
            Some(idx) if idx + 1 < value.len() => Ok(Self(value)),
            _ => Err(IriValidationError::MissingScheme(value)),
        }
    }

    /// Construct without validation. Use for IRIs already vetted upstream
    /// (e.g. produced by `sbol-rdf::Iri`).
    pub fn unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for IriString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for IriString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_iri() {
        assert!(IriString::new("https://example.org/foo").is_ok());
    }

    #[test]
    fn accepts_urn() {
        assert!(IriString::new("urn:sbol:component:1").is_ok());
    }

    #[test]
    fn rejects_blank_node() {
        assert!(IriString::new("_:b0").is_err());
    }

    #[test]
    fn rejects_no_scheme() {
        assert!(IriString::new("just-a-name").is_err());
    }

    #[test]
    fn rejects_empty_body() {
        assert!(IriString::new("https:").is_err());
    }
}
