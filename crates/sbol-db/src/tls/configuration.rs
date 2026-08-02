use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::extract::State;
use axum::http::Uri;
use axum::response::Redirect;
use axum::Router;
use rustls_acme::acme::{LETS_ENCRYPT_PRODUCTION_DIRECTORY, LETS_ENCRYPT_STAGING_DIRECTORY};
use rustls_acme::{AcmeConfig, AcmeState};
use url::{Host, Url};

use super::acceptor::TimeoutAcceptor;
use super::cache::{CertificateState, DurableAcmeCache};
use crate::cli::{RuntimeProfile, ServerArgs, TlsMode};
use crate::runtime::ServerRuntime;

const DEVELOPMENT_BIND: &str = "127.0.0.1:8888";
const PRODUCTION_BIND: &str = "0.0.0.0:443";
const PRODUCTION_HTTP_BIND: &str = "0.0.0.0:80";

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
    pub(super) hostname: String,
    pub(super) contact: String,
    pub(super) directory_url: String,
    pub(super) cache_root: PathBuf,
    pub(super) redirect_origin: String,
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
                        cache_root: runtime.data_root().join("acme"),
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

pub(super) fn redirect_target(origin: &str, uri: &Uri) -> String {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    format!("{origin}{path_and_query}")
}

pub(super) fn normalize_hostname(raw: &str) -> Result<String> {
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
