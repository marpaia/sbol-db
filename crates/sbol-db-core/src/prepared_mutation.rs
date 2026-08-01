//! Durable, one-time prepared mutation records.
//!
//! Only a hash of the opaque plan token is persisted. The normalized payload
//! is retained so execution commits exactly what the user reviewed, rather
//! than reinterpreting a later request carrying a boolean confirmation flag.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::UserId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreparedMutation {
    pub token_hash: String,
    pub user_id: UserId,
    pub oauth_client_id: Option<String>,
    pub audience: Option<String>,
    pub required_scopes: Vec<String>,
    pub operation: String,
    pub target_iri: Option<String>,
    pub expected_content_etag: Option<String>,
    pub input_hash: String,
    pub effect: Value,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
