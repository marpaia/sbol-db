//! Bootstrap and restart application of durable edge settings.

use anyhow::{Context, Result};
use rustls_acme::acme::LETS_ENCRYPT_PRODUCTION_DIRECTORY;
use sbol_db_server::{read_edge_settings, write_edge_settings, EdgeSettings, EDGE_SETTINGS_KEY};
use sbol_db_storage::ConfigStore;

use crate::cli::ServerArgs;

pub async fn resolve(store: &dyn ConfigStore, args: &mut ServerArgs) -> Result<EdgeSettings> {
    let settings = match read_edge_settings(store).await? {
        Some(settings) => {
            tracing::info!(key = EDGE_SETTINGS_KEY, "applying durable edge settings");
            settings
        }
        None => {
            let settings = bootstrap(args)?.validate()?;
            write_edge_settings(store, &settings).await?;
            tracing::info!(
                key = EDGE_SETTINGS_KEY,
                "seeded durable edge settings from bootstrap arguments"
            );
            settings
        }
    };
    apply(args, &settings);
    Ok(settings)
}

fn bootstrap(args: &ServerArgs) -> Result<EdgeSettings> {
    Ok(EdgeSettings {
        version: 1,
        hostname: args
            .hostname
            .clone()
            .context("production requires SBOL_DB_HOSTNAME on first launch")?,
        acme_contact: args
            .acme_contact
            .clone()
            .context("production requires SBOL_DB_ACME_CONTACT on first launch")?,
        acme_directory_url: args
            .acme_directory_url
            .clone()
            .unwrap_or_else(|| LETS_ENCRYPT_PRODUCTION_DIRECTORY.to_owned()),
        http_redirect_enabled: !args.no_http_redirect,
        tls_handshake_timeout_secs: args.tls_handshake_timeout_secs,
        backup_recovery_recipient: args
            .backup_recovery_recipient
            .clone()
            .context("production requires SBOL_DB_BACKUP_RECOVERY_RECIPIENT on first launch")?,
        backup_repository_url: args
            .backup_repository_url
            .clone()
            .context("production requires SBOL_DB_BACKUP_REPOSITORY_URL on first launch")?,
        backup_interval_secs: args.backup_interval_secs,
        backup_local_retention: args.backup_local_retention,
        minimum_free_bytes: args.minimum_free_bytes,
    })
}

fn apply(args: &mut ServerArgs, settings: &EdgeSettings) {
    args.hostname = Some(settings.hostname.clone());
    args.acme_contact = Some(settings.acme_contact.clone());
    args.acme_directory_url = Some(settings.acme_directory_url.clone());
    args.no_http_redirect = !settings.http_redirect_enabled;
    args.tls_handshake_timeout_secs = settings.tls_handshake_timeout_secs;
    args.backup_recovery_recipient = Some(settings.backup_recovery_recipient.clone());
    args.backup_repository_url = Some(settings.backup_repository_url.clone());
    args.backup_interval_secs = settings.backup_interval_secs;
    args.backup_local_retention = settings.backup_local_retention;
    args.minimum_free_bytes = settings.minimum_free_bytes;
}
