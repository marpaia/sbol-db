use super::*;

use std::time::Duration;

use axum::http::Uri;
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
use rcgen::generate_simple_self_signed;
use rustls_acme::acme::{LETS_ENCRYPT_PRODUCTION_DIRECTORY, LETS_ENCRYPT_STAGING_DIRECTORY};
use rustls_acme::{AccountCache, AcmeConfig, CertCache, EventOk};

use super::cache::DurableAcmeCache;
use super::configuration::{normalize_hostname, redirect_target};
use crate::cli::{BackendKind, RuntimeProfile, ServerArgs, TlsMode};
use crate::runtime::ServerRuntime;

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
    let (acceptor, mut state, _certificate_state) = config.build(Duration::from_secs(2)).unwrap();
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
