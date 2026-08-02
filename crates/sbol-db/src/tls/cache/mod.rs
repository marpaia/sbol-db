mod filesystem;

use std::fmt::Debug;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};
use async_trait::async_trait;
use rustls_acme::{AccountCache, CertCache};
use sha2::{Digest, Sha256};
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;

use self::filesystem::{
    prepare_private_directory, read_private_cache_file, write_private_cache_file,
};

pub(super) const MAX_CACHE_ENTRY_BYTES: u64 = 4 * 1024 * 1024;

/// Account and certificate cache with atomic replacement and private modes.
#[derive(Clone, Debug)]
pub(super) struct DurableAcmeCache {
    root: Arc<PathBuf>,
    certificate_state: CertificateState,
}

impl DurableAcmeCache {
    pub(super) fn open(root: &Path, certificate_state: CertificateState) -> Result<Self> {
        prepare_private_directory(root)?;
        Ok(Self {
            root: Arc::new(root.to_path_buf()),
            certificate_state,
        })
    }

    fn account_path(&self, contact: &[String], directory_url: &str) -> PathBuf {
        self.root
            .join(cache_name("account", contact, directory_url))
    }

    fn certificate_path(&self, domains: &[String], directory_url: &str) -> PathBuf {
        self.root
            .join(cache_name("certificate", domains, directory_url))
    }

    async fn load(path: PathBuf) -> io::Result<Option<Vec<u8>>> {
        tokio::task::spawn_blocking(move || read_private_cache_file(&path))
            .await
            .map_err(|error| io::Error::other(format!("ACME cache read task failed: {error}")))?
    }

    async fn store(path: PathBuf, contents: Vec<u8>) -> io::Result<()> {
        tokio::task::spawn_blocking(move || write_private_cache_file(&path, &contents))
            .await
            .map_err(|error| io::Error::other(format!("ACME cache write task failed: {error}")))?
    }
}

#[async_trait]
impl CertCache for DurableAcmeCache {
    type EC = io::Error;

    async fn load_cert(
        &self,
        domains: &[String],
        directory_url: &str,
    ) -> io::Result<Option<Vec<u8>>> {
        let contents = Self::load(self.certificate_path(domains, directory_url)).await?;
        if let Some(contents) = contents.as_deref() {
            let not_after = validate_cached_certificate(contents, domains)?;
            self.certificate_state.set_not_after_unix(not_after);
        }
        Ok(contents)
    }

    async fn store_cert(
        &self,
        domains: &[String],
        directory_url: &str,
        cert: &[u8],
    ) -> io::Result<()> {
        let not_after = validate_cached_certificate(cert, domains)?;
        Self::store(self.certificate_path(domains, directory_url), cert.to_vec()).await?;
        self.certificate_state.set_not_after_unix(not_after);
        Ok(())
    }
}

#[async_trait]
impl AccountCache for DurableAcmeCache {
    type EA = io::Error;

    async fn load_account(
        &self,
        contact: &[String],
        directory_url: &str,
    ) -> io::Result<Option<Vec<u8>>> {
        Self::load(self.account_path(contact, directory_url)).await
    }

    async fn store_account(
        &self,
        contact: &[String],
        directory_url: &str,
        account: &[u8],
    ) -> io::Result<()> {
        Self::store(self.account_path(contact, directory_url), account.to_vec()).await
    }
}

fn cache_name(kind: &str, values: &[String], directory_url: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0]);
    for value in values {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    digest.update(directory_url.as_bytes());
    format!("{kind}-{}", hex::encode(digest.finalize()))
}

fn validate_cached_certificate(contents: &[u8], domains: &[String]) -> io::Result<i64> {
    let blocks = pem::parse_many(contents).map_err(invalid_cache_entry)?;
    let leaf = blocks
        .iter()
        .find(|block| block.tag() == "CERTIFICATE")
        .ok_or_else(|| invalid_cache_entry("cached certificate contains no certificate PEM"))?;
    let (_, certificate) = parse_x509_certificate(leaf.contents()).map_err(invalid_cache_entry)?;
    if !certificate.validity().is_valid() {
        return Err(invalid_cache_entry(
            "cached certificate is expired or not yet valid",
        ));
    }
    let alternative_names = certificate
        .subject_alternative_name()
        .map_err(invalid_cache_entry)?
        .ok_or_else(|| invalid_cache_entry("cached certificate has no subjectAltName"))?;
    for domain in domains {
        let present = alternative_names
            .value
            .general_names
            .iter()
            .any(|name| matches!(name, GeneralName::DNSName(value) if value.eq_ignore_ascii_case(domain)));
        if !present {
            return Err(invalid_cache_entry(format!(
                "cached certificate does not cover configured domain {domain}"
            )));
        }
    }
    Ok(certificate.validity().not_after.timestamp())
}

fn invalid_cache_entry(error: impl Debug) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid ACME certificate cache entry: {error:?}"),
    )
}

/// Validity metadata shared between the durable cache and the ACME event loop.
/// A new certificate is not advertised as ready until its successful cache
/// event can read this value.
#[derive(Clone, Debug, Default)]
pub struct CertificateState {
    not_after_unix: Arc<AtomicI64>,
}

impl CertificateState {
    pub(super) fn set_not_after_unix(&self, value: i64) {
        self.not_after_unix.store(value, Ordering::Release);
    }

    pub(super) fn not_after_unix(&self) -> Result<i64> {
        let value = self.not_after_unix.load(Ordering::Acquire);
        if value <= 0 {
            bail!("ACME certificate validity metadata is unavailable after cache event");
        }
        Ok(value)
    }
}
