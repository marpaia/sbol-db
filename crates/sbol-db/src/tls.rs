//! Native public-edge TLS and ACME lifecycle support.
//!
//! Production serves HTTPS itself. ACME uses TLS-ALPN-01 on the public TLS
//! socket, so no reverse proxy or challenge-only HTTP service is required.
//! Account keys and certificate private keys are cached with private modes and
//! atomic, fsynced replacement below the server data directory.

use std::fmt::Debug;
use std::future::Future;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use axum::extract::State;
use axum::http::Uri;
use axum::response::Redirect;
use axum::Router;
use axum_server::accept::Accept;
use futures::StreamExt;
use rustls_acme::acme::{LETS_ENCRYPT_PRODUCTION_DIRECTORY, LETS_ENCRYPT_STAGING_DIRECTORY};
use rustls_acme::{AccountCache, AcmeConfig, AcmeState, CertCache, EventError, EventOk};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};
use uuid::Uuid;
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;

use crate::cli::{RuntimeProfile, ServerArgs, TlsMode};
use crate::runtime::ServerRuntime;

const DEVELOPMENT_BIND: &str = "127.0.0.1:8888";
const PRODUCTION_BIND: &str = "0.0.0.0:443";
const PRODUCTION_HTTP_BIND: &str = "0.0.0.0:80";
const MAX_CACHE_ENTRY_BYTES: u64 = 4 * 1024 * 1024;

/// Fully validated public listener and TLS policy.
#[derive(Clone, Debug)]
pub struct EdgeHttpConfig {
    pub public_bind: SocketAddr,
    pub redirect_bind: Option<SocketAddr>,
    pub tls: Option<AcmeTlsConfig>,
    pub tls_handshake_timeout: Duration,
}

/// Configuration required to construct the ACME state stream and rustls
/// acceptor. The contact is kept private and is never logged.
#[derive(Clone, Debug)]
pub struct AcmeTlsConfig {
    hostname: String,
    contact: String,
    directory_url: String,
    cache_root: PathBuf,
    redirect_origin: String,
}

impl EdgeHttpConfig {
    pub fn resolve(runtime: &ServerRuntime, args: &ServerArgs) -> Result<Self> {
        let production = runtime.profile() == RuntimeProfile::Production;
        let public_bind = args.bind.unwrap_or_else(|| {
            if production {
                PRODUCTION_BIND.parse().expect("valid production bind")
            } else {
                DEVELOPMENT_BIND.parse().expect("valid development bind")
            }
        });
        let mode = args.tls_mode.unwrap_or(if production {
            TlsMode::Acme
        } else {
            TlsMode::Disabled
        });
        if !(1..=60).contains(&args.tls_handshake_timeout_secs) {
            bail!("--tls-handshake-timeout-secs must be between 1 and 60");
        }
        if args.no_http_redirect && args.http_bind.is_some() {
            bail!("--http-bind conflicts with --no-http-redirect");
        }

        let (tls, redirect_bind) = match mode {
            TlsMode::Disabled => {
                if production {
                    bail!("production profile requires --tls-mode acme");
                }
                if args.hostname.is_some()
                    || args.acme_contact.is_some()
                    || args.acme_directory_url.is_some()
                    || args.http_bind.is_some()
                {
                    bail!("ACME and HTTP redirect options require --tls-mode acme");
                }
                (None, None)
            }
            TlsMode::Acme => {
                let hostname = normalize_hostname(
                    args.hostname
                        .as_deref()
                        .context("--hostname (or SBOL_DB_HOSTNAME) is required for ACME")?,
                )?;
                let contact =
                    normalize_contact(args.acme_contact.as_deref().context(
                        "--acme-contact (or SBOL_DB_ACME_CONTACT) is required for ACME",
                    )?)?;
                let directory_url = validate_directory_url(
                    args.acme_directory_url.as_deref().unwrap_or(if production {
                        LETS_ENCRYPT_PRODUCTION_DIRECTORY
                    } else {
                        LETS_ENCRYPT_STAGING_DIRECTORY
                    }),
                    production,
                )?;
                let redirect_bind = if args.no_http_redirect {
                    None
                } else if let Some(bind) = args.http_bind {
                    Some(bind)
                } else if production {
                    Some(
                        PRODUCTION_HTTP_BIND
                            .parse()
                            .expect("valid production HTTP bind"),
                    )
                } else {
                    None
                };
                if redirect_bind == Some(public_bind) {
                    bail!("public TLS and HTTP redirect listeners cannot use the same address");
                }
                let redirect_origin = if production || public_bind.port() == 443 {
                    format!("https://{hostname}")
                } else {
                    format!("https://{hostname}:{}", public_bind.port())
                };
                (
                    Some(AcmeTlsConfig {
                        hostname,
                        contact,
                        directory_url,
                        cache_root: runtime
                            .layout()
                            .map(|layout| layout.acme_root().to_path_buf())
                            .unwrap_or_else(|| runtime.data_root().join("acme")),
                        redirect_origin,
                    }),
                    redirect_bind,
                )
            }
        };

        Ok(Self {
            public_bind,
            redirect_bind,
            tls,
            tls_handshake_timeout: Duration::from_secs(args.tls_handshake_timeout_secs),
        })
    }
}

impl AcmeTlsConfig {
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    pub fn directory_url(&self) -> &str {
        &self.directory_url
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn redirect_router(&self) -> Router {
        let state = RedirectState {
            origin: Arc::from(self.redirect_origin.as_str()),
        };
        Router::new().fallback(redirect_to_https).with_state(state)
    }

    pub fn build(
        &self,
        handshake_timeout: Duration,
    ) -> Result<(
        TimeoutAcceptor<rustls_acme::axum::AxumAcceptor>,
        AcmeState<io::Error>,
        CertificateState,
    )> {
        let certificate_state = CertificateState::default();
        let cache = DurableAcmeCache::open(&self.cache_root, certificate_state.clone())?;
        let state = AcmeConfig::new([self.hostname.as_str()])
            .contact([self.contact.as_str()])
            .cache(cache)
            .directory(&self.directory_url)
            .state();
        let acceptor = state.axum_acceptor(state.default_rustls_config());
        Ok((
            TimeoutAcceptor::new(acceptor, handshake_timeout),
            state,
            certificate_state,
        ))
    }
}

#[derive(Clone)]
struct RedirectState {
    origin: Arc<str>,
}

async fn redirect_to_https(State(state): State<RedirectState>, uri: Uri) -> Redirect {
    Redirect::permanent(&redirect_target(&state.origin, &uri))
}

fn redirect_target(origin: &str, uri: &Uri) -> String {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    format!("{origin}{path_and_query}")
}

/// Poll ACME forever. rustls-acme performs issuance and renewal only while its
/// state stream is being polled, so this task is a required part of the server
/// lifecycle rather than detached best-effort work.
pub async fn run_acme(
    mut state: AcmeState<io::Error>,
    certificate_state: CertificateState,
    metrics: Arc<sbol_db_server::Metrics>,
    cancel: CancellationToken,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            event = state.next() => {
                let Some(event) = event else {
                    bail!("ACME lifecycle stream ended unexpectedly");
                };
                match event {
                    Ok(event) => {
                        let kind = event_ok_label(&event);
                        metrics::counter!("sbol_db_acme_events_total", "event" => kind, "result" => "success")
                            .increment(1);
                        match event {
                            EventOk::DeployedCachedCert => {
                                metrics.mark_tls_ready(certificate_state.not_after_unix()?);
                                tracing::info!(event = kind, "ACME certificate deployed");
                            }
                            EventOk::DeployedNewCert => {
                                tracing::info!(
                                    event = kind,
                                    "ACME certificate deployed; waiting for durable cache"
                                );
                            }
                            EventOk::CertCacheStore => {
                                metrics.mark_tls_ready(certificate_state.not_after_unix()?);
                                tracing::info!(event = kind, "ACME certificate persisted and ready");
                            }
                            EventOk::AccountCacheStore => {
                                tracing::info!(event = kind, "ACME state persisted");
                            }
                        }
                    }
                    Err(error) => {
                        let kind = event_error_label(&error);
                        metrics::counter!("sbol_db_acme_events_total", "event" => kind, "result" => "error")
                            .increment(1);
                        tracing::error!(event = kind, error = %error, "ACME lifecycle event failed");
                    }
                }
            }
        }
    }
}

fn event_ok_label(event: &EventOk) -> &'static str {
    match event {
        EventOk::DeployedCachedCert => "certificate_deployed_cached",
        EventOk::DeployedNewCert => "certificate_deployed_new",
        EventOk::CertCacheStore => "certificate_cache_store",
        EventOk::AccountCacheStore => "account_cache_store",
    }
}

fn event_error_label(error: &EventError<io::Error, io::Error>) -> &'static str {
    match error {
        EventError::CertCacheLoad(_) => "certificate_cache_load",
        EventError::AccountCacheLoad(_) => "account_cache_load",
        EventError::CertCacheStore(_) => "certificate_cache_store",
        EventError::AccountCacheStore(_) => "account_cache_store",
        EventError::CachedCertParse(_) => "certificate_cached_parse",
        EventError::Order(_) => "certificate_order",
        EventError::NewCertParse(_) => "certificate_new_parse",
    }
}

fn normalize_hostname(raw: &str) -> Result<String> {
    let raw = raw.trim().trim_end_matches('.');
    if raw.is_empty() || raw.contains('*') {
        bail!("--hostname must be one concrete DNS name, not empty or a wildcard");
    }
    match Host::parse(raw).context("parse --hostname")? {
        Host::Domain(domain) if !domain.is_empty() => Ok(domain.to_ascii_lowercase()),
        Host::Domain(_) => bail!("--hostname cannot be empty"),
        Host::Ipv4(_) | Host::Ipv6(_) => bail!("--hostname must be a DNS name, not an IP address"),
    }
}

fn normalize_contact(raw: &str) -> Result<String> {
    let email = raw.trim().strip_prefix("mailto:").unwrap_or(raw.trim());
    if email
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        bail!("--acme-contact must be one email address without whitespace");
    }
    let Some((local, domain)) = email.split_once('@') else {
        bail!("--acme-contact must be an email address");
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        bail!("--acme-contact must be an email address");
    }
    normalize_hostname(domain).context("invalid --acme-contact domain")?;
    Ok(format!("mailto:{email}"))
}

fn validate_directory_url(raw: &str, production: bool) -> Result<String> {
    let url = Url::parse(raw).context("parse --acme-directory-url")?;
    if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
        bail!("--acme-directory-url cannot contain credentials or a fragment");
    }
    match url.scheme() {
        "https" => {}
        "http" if !production && is_loopback_url(&url) => {}
        "http" => bail!("plain HTTP ACME directories are allowed only on loopback in development"),
        scheme => bail!("unsupported ACME directory URL scheme `{scheme}`; use https"),
    }
    Ok(url.into())
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    }
}

/// Timeout wrapper for ACME's axum acceptor. This caps resources consumed by
/// clients that connect but never complete a TLS handshake.
#[derive(Clone, Debug)]
pub struct TimeoutAcceptor<A> {
    inner: A,
    timeout: Duration,
}

impl<A> TimeoutAcceptor<A> {
    fn new(inner: A, timeout: Duration) -> Self {
        Self { inner, timeout }
    }
}

impl<I, S, A> Accept<I, S> for TimeoutAcceptor<A>
where
    I: Send + 'static,
    S: Send + 'static,
    A: Accept<I, S> + Clone + Send + Sync + 'static,
    A::Future: Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
{
    type Stream = A::Stream;
    type Service = A::Service;
    type Future =
        Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send + 'static>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let future = self.inner.accept(stream, service);
        let timeout = self.timeout;
        Box::pin(async move {
            tokio::time::timeout(timeout, future)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"))?
        })
    }
}

/// Account and certificate cache with atomic replacement and private modes.
#[derive(Clone, Debug)]
struct DurableAcmeCache {
    root: Arc<PathBuf>,
    certificate_state: CertificateState,
}

impl DurableAcmeCache {
    fn open(root: &Path, certificate_state: CertificateState) -> Result<Self> {
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
    fn set_not_after_unix(&self, value: i64) {
        self.not_after_unix.store(value, Ordering::Release);
    }

    fn not_after_unix(&self) -> Result<i64> {
        let value = self.not_after_unix.load(Ordering::Acquire);
        if value <= 0 {
            bail!("ACME certificate validity metadata is unavailable after cache event");
        }
        Ok(value)
    }
}

fn prepare_private_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "ACME cache directory cannot be a symbolic link: {}",
                path.display()
            )
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("ACME cache path is not a directory: {}", path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)
                .with_context(|| format!("create ACME cache directory {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect ACME cache directory {}", path.display()));
        }
    }
    set_private_directory_permissions(path)?;
    Ok(())
}

fn read_private_cache_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ACME cache entry is not a regular file: {}", path.display()),
        ));
    }
    verify_private_file_permissions(path, &metadata)?;
    if metadata.len() > MAX_CACHE_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ACME cache entry exceeds {MAX_CACHE_ENTRY_BYTES} bytes"),
        ));
    }
    let file = std::fs::File::open(path)?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CACHE_ENTRY_BYTES + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_CACHE_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ACME cache entry exceeds {MAX_CACHE_ENTRY_BYTES} bytes"),
        ));
    }
    Ok(Some(contents))
}

fn write_private_cache_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if contents.len() as u64 > MAX_CACHE_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ACME cache entry exceeds {MAX_CACHE_ENTRY_BYTES} bytes"),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "ACME cache path has no parent")
    })?;
    let temp = parent.join(format!(".acme-cache-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        set_private_open_options(&mut options);
        let mut file = options.open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "refusing to replace unsafe ACME cache entry: {}",
                        path.display()
                    ),
                ));
            }
        }
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(unix)]
fn set_private_open_options(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_open_options(_options: &mut std::fs::OpenOptions) {}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("set private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn verify_private_file_permissions(path: &Path, metadata: &std::fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "ACME cache entry is accessible by group or others: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_file_permissions(_path: &Path, _metadata: &std::fs::Metadata) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::BackendKind;
    use axum::routing::get;
    use rcgen::generate_simple_self_signed;

    fn args() -> ServerArgs {
        ServerArgs {
            profile: RuntimeProfile::Development,
            data_dir: None,
            blob_root: None,
            bind: None,
            tls_mode: None,
            hostname: None,
            acme_contact: None,
            acme_directory_url: None,
            http_bind: None,
            no_http_redirect: false,
            tls_handshake_timeout_secs: 10,
            operations_bind: "127.0.0.1:9090".parse().unwrap(),
            explorer_bind: None,
            no_worker: false,
            worker_concurrency: None,
            worker_queues: None,
            worker_id: None,
            search_config: None,
            backup_recovery_recipient: None,
            backup_repository_url: None,
        }
    }

    #[test]
    fn production_defaults_to_https_and_redirect() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = ServerRuntime::resolve(
            RuntimeProfile::Production,
            Some(temp.path()),
            None,
            Some(BackendKind::Rocksdb),
            None,
        )
        .unwrap();
        let mut args = args();
        args.profile = RuntimeProfile::Production;
        args.hostname = Some("Registry.Example.org.".to_owned());
        args.acme_contact = Some("ops@example.org".to_owned());

        let config = EdgeHttpConfig::resolve(&runtime, &args).unwrap();

        assert_eq!(config.public_bind, "0.0.0.0:443".parse().unwrap());
        assert_eq!(config.redirect_bind, Some("0.0.0.0:80".parse().unwrap()));
        let tls = config.tls.unwrap();
        assert_eq!(tls.hostname(), "registry.example.org");
        assert_eq!(tls.directory_url(), LETS_ENCRYPT_PRODUCTION_DIRECTORY);
        assert_eq!(
            tls.cache_root(),
            runtime.layout().expect("managed layout").acme_root()
        );
    }

    #[test]
    fn production_cannot_disable_tls() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = ServerRuntime::resolve(
            RuntimeProfile::Production,
            Some(temp.path()),
            None,
            None,
            None,
        )
        .unwrap();
        let mut args = args();
        args.profile = RuntimeProfile::Production;
        args.tls_mode = Some(TlsMode::Disabled);
        let error = EdgeHttpConfig::resolve(&runtime, &args).unwrap_err();
        assert!(error.to_string().contains("requires --tls-mode acme"));
    }

    #[test]
    fn development_allows_only_loopback_plain_http_acme_directory() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = ServerRuntime::resolve(
            RuntimeProfile::Development,
            Some(temp.path()),
            None,
            None,
            Some("sqlite::memory:"),
        )
        .unwrap();
        let mut args = args();
        args.tls_mode = Some(TlsMode::Acme);
        args.hostname = Some("registry.example.org".to_owned());
        args.acme_contact = Some("ops@example.org".to_owned());
        args.acme_directory_url = Some("http://pebble.internal:14000/dir".to_owned());
        assert!(EdgeHttpConfig::resolve(&runtime, &args).is_err());

        args.acme_directory_url = Some("http://127.0.0.1:14000/dir".to_owned());
        assert!(EdgeHttpConfig::resolve(&runtime, &args).is_ok());
    }

    #[tokio::test]
    async fn durable_cache_round_trips_and_deploys_cached_certificate() {
        let temp = tempfile::tempdir().unwrap();
        let cache = DurableAcmeCache::open(temp.path(), CertificateState::default()).unwrap();
        let domains = vec!["registry.example.org".to_owned()];
        let contact = vec!["mailto:ops@example.org".to_owned()];
        let directory = LETS_ENCRYPT_STAGING_DIRECTORY;
        let certified = generate_simple_self_signed(domains.clone()).unwrap();
        let pem = format!(
            "{}\n{}",
            certified.key_pair.serialize_pem(),
            certified.cert.pem()
        )
        .into_bytes();

        cache.store_cert(&domains, directory, &pem).await.unwrap();
        cache
            .store_account(&contact, directory, b"private-account-key")
            .await
            .unwrap();
        assert_eq!(
            cache.load_cert(&domains, directory).await.unwrap(),
            Some(pem)
        );
        assert_eq!(
            cache.load_account(&contact, directory).await.unwrap(),
            Some(b"private-account-key".to_vec())
        );

        let mut state = AcmeConfig::new(domains)
            .contact(contact)
            .cache(cache)
            .directory(directory)
            .state();
        assert!(matches!(
            state.next().await,
            Some(Ok(EventOk::DeployedCachedCert))
        ));

        #[cfg(unix)]
        for entry in std::fs::read_dir(temp.path()).unwrap() {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.unwrap().metadata().unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[tokio::test]
    async fn cached_certificate_terminates_a_real_https_request() {
        let temp = tempfile::tempdir().unwrap();
        let hostname = "registry.example.org";
        let contact = "mailto:ops@example.org";
        let domains = vec![hostname.to_owned()];
        let certified = generate_simple_self_signed(domains.clone()).unwrap();
        let certificate_pem = certified.cert.pem();
        let combined_pem =
            format!("{}\n{certificate_pem}", certified.key_pair.serialize_pem()).into_bytes();
        let cache = DurableAcmeCache::open(temp.path(), CertificateState::default()).unwrap();
        cache
            .store_cert(&domains, LETS_ENCRYPT_STAGING_DIRECTORY, &combined_pem)
            .await
            .unwrap();

        let config = AcmeTlsConfig {
            hostname: hostname.to_owned(),
            contact: contact.to_owned(),
            directory_url: LETS_ENCRYPT_STAGING_DIRECTORY.to_owned(),
            cache_root: temp.path().to_path_buf(),
            redirect_origin: format!("https://{hostname}"),
        };
        let (acceptor, mut state, _certificate_state) =
            config.build(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            state.next().await,
            Some(Ok(EventOk::DeployedCachedCert))
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let std_listener = listener.into_std().unwrap();
        let handle = axum_server::Handle::new();
        let server_handle = handle.clone();
        let app = Router::new().route("/hello", get(|| async { "native tls" }));
        let server = tokio::spawn(async move {
            axum_server::from_tcp(std_listener)
                .unwrap()
                .acceptor(acceptor)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
                .unwrap();
        });

        let certificate = reqwest::Certificate::from_pem(certificate_pem.as_bytes()).unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(certificate)
            .resolve(hostname, address)
            .build()
            .unwrap();
        let body = client
            .get(format!("https://{hostname}:{}/hello", address.port()))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "native tls");

        handle.graceful_shutdown(Some(Duration::from_secs(1)));
        server.await.unwrap();
    }

    #[test]
    fn hostname_rejects_ip_and_wildcard() {
        assert!(normalize_hostname("127.0.0.1").is_err());
        assert!(normalize_hostname("*.example.org").is_err());
        assert_eq!(normalize_hostname("EXAMPLE.ORG.").unwrap(), "example.org");
    }

    #[test]
    fn redirect_uses_canonical_origin_and_preserves_path_and_query() {
        let uri: Uri = "/submit?collection=private".parse().unwrap();
        assert_eq!(
            redirect_target("https://registry.example.org", &uri),
            "https://registry.example.org/submit?collection=private"
        );
    }
}
