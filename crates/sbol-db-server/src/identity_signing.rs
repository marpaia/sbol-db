//! Ed25519 signing material for SBOL Identity OpenID Connect tokens.
//!
//! The private key is intentionally opaque: configuration can construct it
//! from a base64-encoded PKCS#8 document, but neither `Debug` nor any public
//! accessor returns the secret bytes.

use std::fmt;
use std::sync::Arc;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct IdentitySigningKey {
    key_pair: Arc<Ed25519KeyPair>,
    persistent: bool,
}

impl IdentitySigningKey {
    /// Generate process-local signing material for tests and loopback
    /// development. Production instances should use
    /// [`from_pkcs8_base64`](Self::from_pkcs8_base64) so tokens remain
    /// verifiable across restarts and replicas.
    pub fn generate_ephemeral() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .expect("the operating system must provide secure randomness");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .expect("ring-generated Ed25519 PKCS#8 must parse");
        Self {
            key_pair: Arc::new(key_pair),
            persistent: false,
        }
    }

    /// Parse an Ed25519 PKCS#8 document encoded with standard or unpadded
    /// URL-safe base64. The returned error never includes key material.
    pub fn from_pkcs8_base64(encoded: &str) -> Result<Self, String> {
        let encoded = encoded.trim();
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .or_else(|_| STANDARD.decode(encoded))
            .map_err(|_| "SBOL_DB_IDENTITY_SIGNING_KEY must be base64-encoded PKCS#8".to_owned())?;
        let key_pair = Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| {
            "SBOL_DB_IDENTITY_SIGNING_KEY must contain an Ed25519 PKCS#8 private key".to_owned()
        })?;
        Ok(Self {
            key_pair: Arc::new(key_pair),
            persistent: true,
        })
    }

    pub(crate) fn is_persistent(&self) -> bool {
        self.persistent
    }

    pub(crate) fn key_id(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(self.key_pair.public_key().as_ref()))
    }

    pub(crate) fn jwk(&self) -> Value {
        json!({
            "kty": "OKP",
            "crv": "Ed25519",
            "use": "sig",
            "alg": "EdDSA",
            "kid": self.key_id(),
            "x": URL_SAFE_NO_PAD.encode(self.key_pair.public_key().as_ref())
        })
    }

    pub(crate) fn sign_claims(&self, claims: &Value) -> Result<String, String> {
        let header = json!({
            "alg": "EdDSA",
            "typ": "JWT",
            "kid": self.key_id()
        });
        let header = serde_json::to_vec(&header)
            .map_err(|_| "failed to encode the ID-token header".to_owned())?;
        let claims = serde_json::to_vec(claims)
            .map_err(|_| "failed to encode the ID-token claims".to_owned())?;
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header),
            URL_SAFE_NO_PAD.encode(claims)
        );
        let signature = self.key_pair.sign(signing_input.as_bytes());
        Ok(format!(
            "{}.{}",
            signing_input,
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        ))
    }
}

impl Default for IdentitySigningKey {
    fn default() -> Self {
        Self::generate_ephemeral()
    }
}

impl fmt::Debug for IdentitySigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentitySigningKey")
            .field("private_key", &"[REDACTED]")
            .field("persistent", &self.persistent)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey, ED25519};

    #[test]
    fn jwt_is_signed_and_private_material_is_redacted() {
        let key = IdentitySigningKey::generate_ephemeral();
        let token = key.sign_claims(&json!({"sub": "alice"})).unwrap();
        let pieces = token.split('.').collect::<Vec<_>>();
        assert_eq!(pieces.len(), 3);
        let signature = URL_SAFE_NO_PAD.decode(pieces[2]).unwrap();
        UnparsedPublicKey::new(&ED25519, key.key_pair.public_key().as_ref())
            .verify(
                format!("{}.{}", pieces[0], pieces[1]).as_bytes(),
                &signature,
            )
            .unwrap();
        let debug = format!("{key:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(&token));
    }
}
