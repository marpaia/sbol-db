use std::path::Path;
use std::sync::Arc;

use age::x25519;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path as ObjectPath;
use object_store::{Error as ObjectStoreError, ObjectStore, ObjectStoreExt, WriteMultipart};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::archive::verify_encrypted_backup;
use crate::types::{CreatedBackup, PublishedBackup};

#[async_trait]
pub trait BackupRepository: Send + Sync + 'static {
    async fn publish_verified(
        &self,
        local: &CreatedBackup,
        verification_identity: &x25519::Identity,
        staging_parent: &Path,
    ) -> Result<PublishedBackup>;
}

/// S3/GCS repository backed by Apache Arrow's provider-neutral object-store
/// client. Credentials come only from the providers' standard environment or
/// workload identity; the repository URL contains only bucket and prefix.
pub struct ObjectStoreBackupRepository {
    store: Arc<dyn ObjectStore>,
    provider: String,
    bucket: String,
    prefix: ObjectPath,
}

impl ObjectStoreBackupRepository {
    pub fn from_url(repository_url: &str) -> Result<Self> {
        let url = Url::parse(repository_url).context("parse backup repository URL")?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port().is_some()
        {
            bail!("backup repository URL may contain only scheme, bucket, and object prefix");
        }
        let bucket = url
            .host_str()
            .filter(|bucket| !bucket.is_empty())
            .context("backup repository URL is missing its bucket")?
            .to_owned();
        let raw_prefix = url.path().trim_matches('/');
        if raw_prefix.is_empty() {
            bail!("backup repository URL must include a non-empty instance prefix");
        }
        let prefix = ObjectPath::from_url_path(raw_prefix)
            .context("backup repository URL has an invalid object prefix")?;
        if prefix.is_root() {
            bail!("backup repository URL must include a non-empty instance prefix");
        }
        let (provider, store): (&str, Arc<dyn ObjectStore>) = match url.scheme() {
            "s3" => {
                if environment_flag("AWS_ALLOW_HTTP") {
                    bail!("AWS_ALLOW_HTTP cannot be enabled for backup repositories");
                }
                let store = AmazonS3Builder::from_env()
                    .with_bucket_name(&bucket)
                    .build()
                    .context("configure S3 backup repository")?;
                ("s3", Arc::new(store))
            }
            "gs" => {
                let store = GoogleCloudStorageBuilder::from_env()
                    .with_bucket_name(&bucket)
                    .build()
                    .context("configure GCS backup repository")?;
                ("gcs", Arc::new(store))
            }
            scheme => {
                bail!("unsupported backup repository scheme `{scheme}`; expected s3:// or gs://")
            }
        };
        Ok(Self::new(store, provider, bucket, prefix))
    }

    pub fn new(
        store: Arc<dyn ObjectStore>,
        provider: impl Into<String>,
        bucket: impl Into<String>,
        prefix: ObjectPath,
    ) -> Self {
        Self {
            store,
            provider: provider.into(),
            bucket: bucket.into(),
            prefix,
        }
    }

    fn object_key(&self, backup: &CreatedBackup) -> Result<ObjectPath> {
        let file_name = backup
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .context("local backup artifact has no UTF-8 filename")?;
        Ok(self
            .prefix
            .clone()
            .join(backup.created_at.format("%Y").to_string())
            .join(backup.created_at.format("%m").to_string())
            .join(backup.created_at.format("%d").to_string())
            .join(file_name))
    }

    async fn verify_remote(
        &self,
        object_key: &ObjectPath,
        local: &CreatedBackup,
        verification_identity: &x25519::Identity,
        staging_parent: &Path,
        reused: bool,
    ) -> Result<PublishedBackup> {
        let result = self
            .store
            .get(object_key)
            .await
            .with_context(|| format!("read back remote backup `{object_key}`"))?;
        if result.meta.size != local.artifact_bytes {
            bail!(
                "remote backup size mismatch for `{object_key}`: local={}, remote={}",
                local.artifact_bytes,
                result.meta.size
            );
        }
        let e_tag = result.meta.e_tag.clone();
        let version = result.meta.version.clone();
        let temporary = tempfile::Builder::new()
            .prefix(".remote-backup-readback-")
            .suffix(".partial")
            .tempfile_in(staging_parent)
            .context("create remote backup readback file")?;
        let mut output = tokio::fs::File::from_std(
            temporary
                .reopen()
                .context("reopen remote backup readback file")?,
        );
        let mut stream = result.into_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("stream remote backup `{object_key}`"))?;
            output
                .write_all(&chunk)
                .await
                .context("write remote backup readback")?;
        }
        output
            .sync_all()
            .await
            .context("sync remote backup readback")?;
        drop(output);
        let verified =
            verify_encrypted_backup(temporary.path(), verification_identity, staging_parent)
                .context("semantic verification of remote backup readback")?;
        if verified.manifest.backup_id != local.backup_id
            || verified.artifact_sha256 != local.artifact_sha256
            || verified.artifact_bytes != local.artifact_bytes
        {
            bail!("remote backup readback does not match the local verified artifact");
        }
        Ok(PublishedBackup {
            provider: self.provider.clone(),
            bucket: self.bucket.clone(),
            object_key: object_key.to_string(),
            artifact_sha256: verified.artifact_sha256,
            artifact_bytes: verified.artifact_bytes,
            e_tag,
            version,
            verified_at: Utc::now(),
            reused,
        })
    }

    async fn upload(&self, object_key: &ObjectPath, local: &CreatedBackup) -> Result<()> {
        let mut input = tokio::fs::File::open(&local.path)
            .await
            .with_context(|| format!("open local backup {}", local.path.display()))?;
        let multipart = self
            .store
            .put_multipart(object_key)
            .await
            .with_context(|| format!("start remote backup upload `{object_key}`"))?;
        let mut upload = WriteMultipart::new(multipart);
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let read = match input.read(&mut buffer).await {
                Ok(read) => read,
                Err(error) => {
                    let _ = upload.abort().await;
                    return Err(error).context("read local backup for remote upload");
                }
            };
            if read == 0 {
                break;
            }
            if let Err(error) = upload.wait_for_capacity(4).await {
                let _ = upload.abort().await;
                return Err(error).context("wait for remote backup upload capacity");
            }
            upload.write(&buffer[..read]);
        }
        upload
            .finish()
            .await
            .with_context(|| format!("complete remote backup upload `{object_key}`"))?;
        Ok(())
    }
}

#[async_trait]
impl BackupRepository for ObjectStoreBackupRepository {
    async fn publish_verified(
        &self,
        local: &CreatedBackup,
        verification_identity: &x25519::Identity,
        staging_parent: &Path,
    ) -> Result<PublishedBackup> {
        let object_key = self.object_key(local)?;
        match self.store.head(&object_key).await {
            Ok(_) => {
                return self
                    .verify_remote(
                        &object_key,
                        local,
                        verification_identity,
                        staging_parent,
                        true,
                    )
                    .await;
            }
            Err(ObjectStoreError::NotFound { .. }) => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspect remote backup `{object_key}`"));
            }
        }
        self.upload(&object_key, local).await?;
        self.verify_remote(
            &object_key,
            local,
            verification_identity,
            staging_parent,
            false,
        )
        .await
    }
}

fn environment_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}
