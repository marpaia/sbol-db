use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;

use age::secrecy::ExposeSecret;
use age::x25519;
use anyhow::{bail, Context, Result};

use crate::filesystem::{
    prepare_private_directory, set_file_mode, sync_directory, verify_private_file_mode,
};

/// The configured recovery recipient plus a local identity used to decrypt and
/// verify local and remote readbacks. Artifacts are encrypted to both public
/// keys, so the external recovery identity never needs to live on the server.
#[derive(Clone)]
pub struct BackupEncryption {
    recovery_recipient: x25519::Recipient,
    verification_identity: x25519::Identity,
}

impl BackupEncryption {
    pub fn new(
        recovery_recipient: x25519::Recipient,
        verification_identity: x25519::Identity,
    ) -> Self {
        Self {
            recovery_recipient,
            verification_identity,
        }
    }

    pub fn parse(recovery_recipient: &str, verification_identity: &str) -> Result<Self> {
        let recipient = x25519::Recipient::from_str(recovery_recipient.trim())
            .map_err(|error| anyhow::anyhow!("invalid age recovery recipient: {error}"))?;
        let identity = parse_x25519_identity(verification_identity)?;
        Ok(Self::new(recipient, identity))
    }

    pub fn recovery_recipient(&self) -> &x25519::Recipient {
        &self.recovery_recipient
    }

    pub fn verification_identity(&self) -> &x25519::Identity {
        &self.verification_identity
    }
}

/// Load the server's local verification identity, creating it atomically on
/// first launch. The external recovery recipient is public configuration; its
/// corresponding secret key must be held outside the server.
pub fn load_or_create_encryption(
    recovery_recipient: &str,
    verification_identity_path: &Path,
) -> Result<BackupEncryption> {
    let recovery_recipient = x25519::Recipient::from_str(recovery_recipient.trim())
        .map_err(|error| anyhow::anyhow!("invalid age recovery recipient: {error}"))?;
    let parent = verification_identity_path
        .parent()
        .context("backup verification identity path has no parent")?;
    prepare_private_directory(parent)?;

    let verification_identity = match read_private_identity(verification_identity_path) {
        Ok(identity) => identity,
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|io| io.kind() == io::ErrorKind::NotFound) =>
        {
            create_private_identity(verification_identity_path)?
        }
        Err(error) => return Err(error),
    };
    Ok(BackupEncryption::new(
        recovery_recipient,
        verification_identity,
    ))
}

/// Create one owner-only recovery identity without replacing an existing key.
///
/// Returning only the public recipient keeps callers from accidentally
/// serializing the private identity into command output or logs.
pub fn generate_x25519_identity_file(path: &Path) -> Result<x25519::Recipient> {
    Ok(create_private_identity(path)?.to_public())
}

fn read_private_identity(path: &Path) -> Result<x25519::Identity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "backup verification identity must be a regular, non-symlink file: {}",
            path.display()
        );
    }
    verify_private_file_mode(path, &metadata)?;
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read backup verification identity {}", path.display()))?;
    parse_x25519_identity(&contents)
}

fn create_private_identity(path: &Path) -> Result<x25519::Identity> {
    let parent = path
        .parent()
        .context("backup verification identity path has no parent")?;
    let identity = x25519::Identity::generate();
    let encoded = identity.to_string();
    let body = format!(
        "# sbol-db local backup verification identity\n# public key: {}\n{}\n",
        identity.to_public(),
        encoded.expose_secret()
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".backup-verification-identity-")
        .tempfile_in(parent)
        .context("create temporary backup verification identity")?;
    set_file_mode(temporary.path(), 0o600)?;
    temporary
        .write_all(body.as_bytes())
        .context("write backup verification identity")?;
    temporary
        .as_file()
        .sync_all()
        .context("sync backup verification identity")?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {
            sync_directory(parent)?;
            Ok(identity)
        }
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            read_private_identity(path)
        }
        Err(error) => Err(error.error)
            .with_context(|| format!("publish backup verification identity at {}", path.display())),
    }
}

/// Parse an age secret-key file. Blank lines and age-keygen comments are
/// ignored, but exactly one X25519 identity must remain.
pub fn parse_x25519_identity(contents: &str) -> Result<x25519::Identity> {
    let mut identities = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let encoded = identities
        .next()
        .context("age identity file contains no key")?;
    if identities.next().is_some() {
        bail!("age identity file must contain exactly one key");
    }
    x25519::Identity::from_str(encoded)
        .map_err(|error| anyhow::anyhow!("invalid age X25519 identity: {error}"))
}
