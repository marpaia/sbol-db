//! Principal-bound durable prepared changes.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use sbol_db_core::{DomainError, PreparedMutation, UserId};
use sbol_db_storage::PreparedMutationStore;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sha3::Sha3_256;
use uuid::Uuid;

const PREPARED_MUTATION_TTL: Duration = Duration::minutes(10);

/// Request identity fields a prepared change is bound to. A first-party
/// caller uses `None` for client and audience; delegated OAuth supplies both.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedMutationBinding {
    pub user_id: UserId,
    pub oauth_client_id: Option<String>,
    pub audience: Option<String>,
    pub scopes: Vec<String>,
}

/// Safe projection returned to a user or agent. The commit payload and stored
/// token hash are deliberately absent.
#[derive(Clone, Debug, Serialize)]
pub struct PreparedMutationReceipt {
    pub plan_token: String,
    pub operation: String,
    pub target_iri: Option<String>,
    pub expected_content_etag: Option<String>,
    pub input_hash: String,
    pub effect: Value,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum PreparedMutationError {
    #[error("the prepared change token is unknown or has already been used")]
    InvalidToken,
    #[error("the prepared change expired; prepare it again")]
    Expired,
    #[error("the prepared change belongs to a different user, client, or audience")]
    PrincipalMismatch,
    #[error(
        "the current authorization no longer grants every scope required by the prepared change"
    )]
    InsufficientScope,
    #[error(transparent)]
    Domain(#[from] DomainError),
}

#[derive(Clone)]
pub struct PreparedMutationService {
    store: Arc<dyn PreparedMutationStore>,
}

impl PreparedMutationService {
    pub fn new(store: Arc<dyn PreparedMutationStore>) -> Self {
        Self { store }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prepare(
        &self,
        binding: &PreparedMutationBinding,
        operation: &str,
        target_iri: Option<String>,
        expected_content_etag: Option<String>,
        required_scopes: Vec<String>,
        effect: Value,
        payload: Value,
    ) -> Result<PreparedMutationReceipt, PreparedMutationError> {
        let operation = operation.trim();
        if operation.is_empty() {
            return Err(DomainError::InvalidInput(
                "prepared mutation operation cannot be empty".to_owned(),
            )
            .into());
        }
        let mut required_scopes = required_scopes;
        required_scopes.sort();
        required_scopes.dedup();
        ensure_scopes(binding, &required_scopes)?;

        let payload_bytes = serde_json::to_vec(&payload).map_err(DomainError::from)?;
        let input_hash = hex::encode(Sha256::digest(&payload_bytes));
        let plan_token = format!(
            "sbol_plan_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let token_hash = token_hash(&plan_token);
        let created_at = Utc::now();
        let expires_at = created_at + PREPARED_MUTATION_TTL;
        self.store
            .put_prepared_mutation(PreparedMutation {
                token_hash,
                user_id: binding.user_id,
                oauth_client_id: binding.oauth_client_id.clone(),
                audience: binding.audience.clone(),
                required_scopes,
                operation: operation.to_owned(),
                target_iri: target_iri.clone(),
                expected_content_etag: expected_content_etag.clone(),
                input_hash: input_hash.clone(),
                effect: effect.clone(),
                payload,
                created_at,
                expires_at,
            })
            .await?;
        Ok(PreparedMutationReceipt {
            plan_token,
            operation: operation.to_owned(),
            target_iri,
            expected_content_etag,
            input_hash,
            effect,
            expires_at,
        })
    }

    /// Verify the current principal before atomically consuming the plan. A
    /// successful return is the only point at which an adapter may dispatch
    /// the stored payload to its mutation service.
    pub async fn consume(
        &self,
        plan_token: &str,
        binding: &PreparedMutationBinding,
    ) -> Result<PreparedMutation, PreparedMutationError> {
        let hash = token_hash(plan_token);
        let plan = self
            .store
            .get_prepared_mutation(&hash)
            .await?
            .ok_or(PreparedMutationError::InvalidToken)?;
        verify_binding(&plan, binding)?;
        if plan.expires_at <= Utc::now() {
            let _ = self.store.consume_prepared_mutation(&hash).await?;
            return Err(PreparedMutationError::Expired);
        }
        self.store
            .consume_prepared_mutation(&hash)
            .await?
            .ok_or(PreparedMutationError::InvalidToken)
    }
}

fn verify_binding(
    plan: &PreparedMutation,
    binding: &PreparedMutationBinding,
) -> Result<(), PreparedMutationError> {
    if plan.user_id != binding.user_id
        || plan.oauth_client_id != binding.oauth_client_id
        || plan.audience != binding.audience
    {
        return Err(PreparedMutationError::PrincipalMismatch);
    }
    ensure_scopes(binding, &plan.required_scopes)
}

fn ensure_scopes(
    binding: &PreparedMutationBinding,
    required_scopes: &[String],
) -> Result<(), PreparedMutationError> {
    if required_scopes
        .iter()
        .all(|required| binding.scopes.iter().any(|scope| scope == required))
    {
        Ok(())
    } else {
        Err(PreparedMutationError::InsufficientScope)
    }
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha3_256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::InMemoryPreparedMutationStore;

    fn binding() -> PreparedMutationBinding {
        PreparedMutationBinding {
            user_id: UserId::new(),
            oauth_client_id: Some("client".to_owned()),
            audience: Some("https://sbol.io/mcp".to_owned()),
            scopes: vec!["sbol:read".to_owned(), "sbol:write".to_owned()],
        }
    }

    #[tokio::test]
    async fn plan_is_bound_and_single_use() {
        let service = PreparedMutationService::new(Arc::new(InMemoryPreparedMutationStore::new()));
        let binding = binding();
        let receipt = service
            .prepare(
                &binding,
                "collection.replace",
                Some("urn:collection".to_owned()),
                Some("\"etag\"".to_owned()),
                vec!["sbol:write".to_owned()],
                serde_json::json!({"summary": "replace one collection"}),
                serde_json::json!({"content": "data"}),
            )
            .await
            .unwrap();
        let plan = service
            .consume(&receipt.plan_token, &binding)
            .await
            .unwrap();
        assert_eq!(plan.operation, "collection.replace");
        assert!(matches!(
            service.consume(&receipt.plan_token, &binding).await,
            Err(PreparedMutationError::InvalidToken)
        ));
    }
}
