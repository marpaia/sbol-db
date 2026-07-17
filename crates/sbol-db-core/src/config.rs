//! The persisted application-configuration entry.
//!
//! sbol-db keeps instance configuration (registries, remotes, plugins, mail,
//! theme, and the like) as a flat key/JSON-value store, the durable equivalent
//! of classic SynBioHub's mutable `config.local.json`. Each section lives under
//! one stable key with an arbitrary JSON value, so the schema of a section is
//! owned by the application layer that reads it rather than by the storage
//! backend.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One row of the configuration store: a section key, its JSON value, and the
/// time it was last written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigEntry {
    /// The stable section key, e.g. `mail`, `theme`, or `plugins`.
    pub key: String,
    /// The section's value as arbitrary JSON.
    pub value: Value,
    /// When this entry was last written.
    pub updated_at: DateTime<Utc>,
}
