//! Filesystem-backed content-addressed blob store.
//!
//! [`FsBlobStore`] lays blobs out exactly as classic SynBioHub does, so a
//! migration can copy the `uploads/` tree in place: each blob is gzip-compressed
//! at `<root>/uploads/<sha1[0:2]>/<sha1[2:]>.gz`, keyed by the SHA-1 of the
//! *uncompressed* bytes. Content-addressing makes writes idempotent: identical
//! content collapses onto one file.

use std::io::{Read, Write};
use std::path::PathBuf;

use async_trait::async_trait;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use sbol_db_core::{BlobRef, DomainError};
use sbol_db_storage::BlobStore;
use sha1::{Digest, Sha1};

/// The media-type IRI prefix SynBioHub records for attachment formats.
const MEDIATYPE_PREFIX: &str = "http://purl.org/NET/mediatypes/";

/// A content-addressed gzip blob store rooted at a directory. The directory
/// tree is created lazily on the first [`put`](FsBlobStore::put).
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Create a store rooted at `root`. Blobs live under `<root>/uploads/`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The `.gz` path a blob with content address `sha1` occupies. `sha1` is a
    /// 40-character hex digest; the first two characters shard the directory.
    fn blob_path(&self, sha1: &str) -> PathBuf {
        self.root
            .join("uploads")
            .join(&sha1[0..2])
            .join(format!("{}.gz", &sha1[2..]))
    }
}

/// The media-type IRI for `bytes`, sniffed from their byte signature. Content
/// with no recognized signature is recorded as `text/plain`, matching
/// SynBioHub's fallback for unclassifiable uploads.
fn sniff_mime(bytes: &[u8]) -> String {
    match infer::get(bytes) {
        Some(kind) => format!("{MEDIATYPE_PREFIX}{}", kind.mime_type()),
        None => format!("{MEDIATYPE_PREFIX}text/plain"),
    }
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put(&self, bytes: &[u8]) -> Result<BlobRef, DomainError> {
        let mut hasher = Sha1::new();
        hasher.update(bytes);
        let sha1 = hex::encode(hasher.finalize());
        let size = bytes.len() as u64;
        let mime = sniff_mime(bytes);

        let path = self.blob_path(&sha1);
        if !path.exists() {
            let dir = path
                .parent()
                .expect("a blob path always has a shard directory");
            std::fs::create_dir_all(dir)?;
            // Gzip into a uniquely named temp file in the same directory, then
            // rename into place. The rename is atomic on one filesystem, so a
            // reader never observes a half-written blob, and a lost race just
            // overwrites the target with byte-identical content.
            let temp = dir.join(format!("{sha1}.{}.tmp", uuid::Uuid::new_v4().simple()));
            {
                let file = std::fs::File::create(&temp)?;
                let mut encoder = GzEncoder::new(file, Compression::default());
                encoder.write_all(bytes)?;
                encoder.finish()?;
            }
            std::fs::rename(&temp, &path)?;
        }

        Ok(BlobRef { sha1, size, mime })
    }

    async fn get(&self, sha1: &str) -> Result<Option<Vec<u8>>, DomainError> {
        if sha1.len() < 3 {
            return Ok(None);
        }
        match std::fs::read(self.blob_path(sha1)) {
            Ok(gz) => {
                let mut out = Vec::new();
                GzDecoder::new(&gz[..]).read_to_end(&mut out)?;
                Ok(Some(out))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DomainError::Io(e.to_string())),
        }
    }

    async fn get_gz(&self, sha1: &str) -> Result<Option<Vec<u8>>, DomainError> {
        if sha1.len() < 3 {
            return Ok(None);
        }
        match std::fs::read(self.blob_path(sha1)) {
            Ok(gz) => Ok(Some(gz)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DomainError::Io(e.to_string())),
        }
    }

    async fn exists(&self, sha1: &str) -> Result<bool, DomainError> {
        if sha1.len() < 3 {
            return Ok(false);
        }
        Ok(self.blob_path(sha1).exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fs_blob_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());

        let payload = b"hello attachment";
        let blob = store.put(payload).await.expect("put");
        assert_eq!(blob.size, 16, "size is the uncompressed byte count");
        assert_eq!(blob.sha1.len(), 40, "sha1 is a 40-char hex digest");

        // The gzip file lands at the SynBioHub uploads layout.
        let path = dir
            .path()
            .join("uploads")
            .join(&blob.sha1[0..2])
            .join(format!("{}.gz", &blob.sha1[2..]));
        assert!(path.exists(), "blob must be stored at the uploads layout");

        // get returns byte-identical content.
        let got = store
            .get(&blob.sha1)
            .await
            .expect("get")
            .expect("blob present");
        assert_eq!(got, payload, "get must return the original bytes verbatim");

        assert!(store.exists(&blob.sha1).await.expect("exists"));
        assert!(
            !store
                .exists("ffffffffffffffffffffffffffffffffffffffff")
                .await
                .expect("exists absent"),
            "an unstored hash must not report as present"
        );

        // Putting the same bytes twice yields the same ref and one file.
        let again = store.put(payload).await.expect("second put");
        assert_eq!(
            again, blob,
            "identical content must yield an identical BlobRef"
        );
        let shard = path.parent().expect("shard directory");
        let count = std::fs::read_dir(shard).expect("read shard").count();
        assert_eq!(count, 1, "identical content must de-duplicate to one file");
    }
}
