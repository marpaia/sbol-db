//! Content-addressed blob reference: the identity, size, and media type of a
//! stored attachment payload. Holds no I/O; the storage layer produces it.

use serde::{Deserialize, Serialize};

/// A reference to a content-addressed blob. `sha1` is the hex SHA-1 of the
/// uncompressed bytes and doubles as the blob's identity, so identical content
/// shares one reference; `size` is the length of those uncompressed bytes;
/// `mime` is the media-type IRI SynBioHub records
/// (`http://purl.org/NET/mediatypes/<type>`), kept as a plain string rather
/// than an enum to carry any media type verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// Hex SHA-1 of the uncompressed bytes; the blob's content address.
    pub sha1: String,
    /// Length of the uncompressed bytes.
    pub size: u64,
    /// Media-type IRI describing the payload.
    pub mime: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_ref_serde_roundtrip() {
        let blob = BlobRef {
            sha1: "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33".to_string(),
            size: 42,
            mime: "http://purl.org/NET/mediatypes/text/plain".to_string(),
        };
        let json = serde_json::to_string(&blob).expect("serialize");
        let back: BlobRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(blob, back);
    }
}
