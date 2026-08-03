//! In-memory identity stores.
//!
//! [`InMemoryUserStore`] and [`InMemoryTokenStore`] implement the identity
//! storage traits against process-local maps. They back two callers: the
//! [`AppServices::new`](crate::AppServices::new) convenience constructor, which
//! provisions a non-persistent identity layer for callers assembling the facade
//! from individual handles, and the facade's own tests and adapter integration
//! tests, which drive [`AuthService`](crate::AuthService) without a database.
//! A real deployment gets its persistent user and token stores through
//! [`AppServices::from_backend`](crate::AppServices::from_backend).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use sbol_db_core::{ConfigEntry, DomainError, NewUser, User, UserId};
use sbol_db_storage::{
    ClusterId, ClusterStore, ConfigStore, PageRankStore, RankRow, Signature, SketchStore,
    TokenStore, UserStore,
};
use serde_json::Value;

/// A process-local [`UserStore`] keyed by [`UserId`], enforcing the same unique
/// `username` and `email` constraints as the persistent backends.
#[derive(Default)]
pub struct InMemoryUserStore {
    users: Mutex<HashMap<UserId, User>>,
}

impl InMemoryUserStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl UserStore for InMemoryUserStore {
    async fn create_user(&self, new_user: NewUser) -> Result<User, DomainError> {
        let mut users = self.users.lock().unwrap();
        if users
            .values()
            .any(|u| u.username == new_user.username || u.email == new_user.email)
        {
            return Err(DomainError::InvalidInput(
                "username or email already registered".into(),
            ));
        }
        let now = Utc::now();
        let user = User {
            id: UserId::new(),
            username: new_user.username,
            name: new_user.name,
            email: new_user.email,
            affiliation: new_user.affiliation,
            password_hash: new_user.password_hash,
            graph_uri: new_user.graph_uri,
            is_admin: new_user.is_admin,
            is_curator: new_user.is_curator,
            is_member: new_user.is_member,
            reset_password_link: None,
            created_at: now,
            updated_at: now,
        };
        users.insert(user.id, user.clone());
        Ok(user)
    }

    async fn find_by_email_or_username(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DomainError> {
        let users = self.users.lock().unwrap();
        if let Some(user) = users.values().find(|user| user.username == identifier) {
            return Ok(Some(user.clone()));
        }
        let mut matches = users.values().filter(|user| user.email == identifier);
        let first = matches.next().cloned();
        if first.is_some() && matches.next().is_some() {
            return Err(DomainError::Validation(
                "multiple accounts use this email; log in with your username".to_owned(),
            ));
        }
        Ok(first)
    }

    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError> {
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn list_users(&self) -> Result<Vec<User>, DomainError> {
        let mut users = self
            .users
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        users.sort_by(|left, right| left.username.cmp(&right.username));
        Ok(users)
    }

    async fn update_user(&self, user: &User) -> Result<User, DomainError> {
        let mut users = self.users.lock().unwrap();
        let Some(existing) = users.get_mut(&user.id) else {
            return Err(DomainError::NotFound(format!("user {}", user.id)));
        };
        existing.name = user.name.clone();
        existing.affiliation = user.affiliation.clone();
        existing.is_admin = user.is_admin;
        existing.is_curator = user.is_curator;
        existing.is_member = user.is_member;
        existing.updated_at = Utc::now();
        Ok(existing.clone())
    }

    async fn set_sole_admin(&self, id: UserId) -> Result<(), DomainError> {
        let mut users = self.users.lock().unwrap();
        if !users.contains_key(&id) {
            return Err(DomainError::NotFound(format!("user {id}")));
        }
        let now = Utc::now();
        for user in users.values_mut() {
            let should_be_admin = user.id == id;
            if user.is_admin != should_be_admin {
                user.is_admin = should_be_admin;
                user.updated_at = now;
            }
        }
        Ok(())
    }

    async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DomainError> {
        let mut users = self.users.lock().unwrap();
        let Some(user) = users.get_mut(&id) else {
            return Err(DomainError::NotFound(format!("user {id}")));
        };
        user.password_hash = password_hash.to_owned();
        user.updated_at = Utc::now();
        Ok(())
    }

    async fn set_reset_link(&self, id: UserId, link: Option<&str>) -> Result<(), DomainError> {
        let mut users = self.users.lock().unwrap();
        let Some(user) = users.get_mut(&id) else {
            return Err(DomainError::NotFound(format!("user {id}")));
        };
        user.reset_password_link = link.map(str::to_owned);
        Ok(())
    }

    async fn consume_reset_link(&self, link: &str) -> Result<Option<User>, DomainError> {
        let mut users = self.users.lock().unwrap();
        let id = users
            .values()
            .find(|u| u.reset_password_link.as_deref() == Some(link))
            .map(|u| u.id);
        let Some(id) = id else {
            return Ok(None);
        };
        let user = users.get_mut(&id).unwrap();
        user.reset_password_link = None;
        Ok(Some(user.clone()))
    }

    async fn delete_user(&self, id: UserId) -> Result<bool, DomainError> {
        Ok(self.users.lock().unwrap().remove(&id).is_some())
    }

    async fn any_admin(&self) -> Result<bool, DomainError> {
        Ok(self.users.lock().unwrap().values().any(|u| u.is_admin))
    }
}

/// A process-local [`TokenStore`] mapping a token hash to the account it
/// authenticates.
#[derive(Default)]
pub struct InMemoryTokenStore {
    tokens: Mutex<HashMap<String, UserId>>,
}

impl InMemoryTokenStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TokenStore for InMemoryTokenStore {
    async fn issue(&self, token_hash: &str, user_id: UserId) -> Result<(), DomainError> {
        self.tokens
            .lock()
            .unwrap()
            .insert(token_hash.to_owned(), user_id);
        Ok(())
    }

    async fn resolve(&self, token_hash: &str) -> Result<Option<UserId>, DomainError> {
        Ok(self.tokens.lock().unwrap().get(token_hash).copied())
    }

    async fn revoke(&self, token_hash: &str) -> Result<bool, DomainError> {
        Ok(self.tokens.lock().unwrap().remove(token_hash).is_some())
    }
}

/// A process-local [`PageRankStore`], the non-persistent default the
/// [`AppServices::new`](crate::AppServices::new) constructor wires so the
/// sequence facade has a rank source; a backend-built facade replaces it with
/// the durable store. Empty until a caller writes ranks.
#[derive(Default)]
pub struct InMemoryPageRankStore {
    ranks: Mutex<HashMap<String, f64>>,
}

impl InMemoryPageRankStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PageRankStore for InMemoryPageRankStore {
    async fn rank_of(&self, iri: &str) -> Result<Option<f64>, DomainError> {
        Ok(self.ranks.lock().unwrap().get(iri).copied())
    }

    async fn ranks_for(&self, iris: &[String]) -> Result<HashMap<String, f64>, DomainError> {
        let ranks = self.ranks.lock().unwrap();
        Ok(iris
            .iter()
            .filter_map(|iri| ranks.get(iri).map(|score| (iri.clone(), *score)))
            .collect())
    }

    async fn all_ranks(&self) -> Result<Vec<RankRow>, DomainError> {
        Ok(self
            .ranks
            .lock()
            .unwrap()
            .iter()
            .map(|(iri, score)| RankRow {
                iri: iri.clone(),
                score: *score,
            })
            .collect())
    }

    async fn replace_all_ranks(&self, ranks: Vec<RankRow>) -> Result<(), DomainError> {
        *self.ranks.lock().unwrap() = ranks.into_iter().map(|r| (r.iri, r.score)).collect();
        Ok(())
    }
}

/// A process-local [`ClusterStore`], the non-persistent default the
/// [`AppServices::new`](crate::AppServices::new) constructor wires so the
/// sequence facade can answer `/similar`; a backend-built facade replaces it
/// with the durable store. Empty until a caller writes assignments.
#[derive(Default)]
pub struct InMemoryClusterStore {
    /// Sequence IRI to the cluster it belongs to.
    assignments: Mutex<HashMap<String, ClusterId>>,
}

impl InMemoryClusterStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

/// A process-local [`SketchStore`], the non-persistent default the
/// [`AppServices::new`](crate::AppServices::new) constructor wires so the
/// sequence facade's align path has a similarity index; a backend-built facade
/// replaces it with the durable store. Empty until a caller writes a sketch.
#[derive(Default)]
pub struct InMemorySketchStore {
    /// Sequence IRI to its MinHash signature.
    sketches: Mutex<HashMap<String, Signature>>,
    /// Sequence IRI to the LSH band hashes it falls into.
    bands: Mutex<HashMap<String, Vec<u64>>>,
}

impl InMemorySketchStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SketchStore for InMemorySketchStore {
    async fn put_sketch(
        &self,
        iri: &str,
        signature: &Signature,
        bands: &[u64],
    ) -> Result<(), DomainError> {
        self.sketches
            .lock()
            .unwrap()
            .insert(iri.to_owned(), signature.clone());
        self.bands
            .lock()
            .unwrap()
            .insert(iri.to_owned(), bands.to_vec());
        Ok(())
    }

    async fn sketch_of(&self, iri: &str) -> Result<Option<Signature>, DomainError> {
        Ok(self.sketches.lock().unwrap().get(iri).cloned())
    }

    async fn candidates_by_bands(&self, bands: &[u64]) -> Result<Vec<String>, DomainError> {
        let query: std::collections::HashSet<u64> = bands.iter().copied().collect();
        let mut out = Vec::new();
        for (iri, seq_bands) in self.bands.lock().unwrap().iter() {
            if seq_bands.iter().any(|b| query.contains(b)) {
                out.push(iri.clone());
            }
        }
        Ok(out)
    }

    async fn all_sketches(&self) -> Result<Vec<(String, Signature)>, DomainError> {
        Ok(self
            .sketches
            .lock()
            .unwrap()
            .iter()
            .map(|(iri, sig)| (iri.clone(), sig.clone()))
            .collect())
    }

    async fn replace_all_sketches(
        &self,
        entries: Vec<(String, Signature, Vec<u64>)>,
    ) -> Result<(), DomainError> {
        let mut sketches = self.sketches.lock().unwrap();
        let mut bands = self.bands.lock().unwrap();
        sketches.clear();
        bands.clear();
        for (iri, signature, band_hashes) in entries {
            sketches.insert(iri.clone(), signature);
            bands.insert(iri, band_hashes);
        }
        Ok(())
    }
}

/// A process-local [`ConfigStore`], the non-persistent default the
/// [`AppServices::new`](crate::AppServices::new) constructor wires so the config
/// service has somewhere to read and write; a backend-built facade replaces it
/// with the durable store. Empty until a caller writes a key.
#[derive(Default)]
pub struct InMemoryConfigStore {
    entries: Mutex<HashMap<String, ConfigEntry>>,
}

impl InMemoryConfigStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ConfigStore for InMemoryConfigStore {
    async fn get(&self, key: &str) -> Result<Option<Value>, DomainError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(key)
            .map(|e| e.value.clone()))
    }

    async fn set(&self, key: &str, value: &Value) -> Result<(), DomainError> {
        self.entries.lock().unwrap().insert(
            key.to_owned(),
            ConfigEntry {
                key: key.to_owned(),
                value: value.clone(),
                updated_at: Utc::now(),
            },
        );
        Ok(())
    }

    async fn get_all(&self) -> Result<Vec<ConfigEntry>, DomainError> {
        Ok(self.entries.lock().unwrap().values().cloned().collect())
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        self.entries.lock().unwrap().remove(key);
        Ok(())
    }
}

#[async_trait]
impl ClusterStore for InMemoryClusterStore {
    async fn cluster_id_of(&self, iri: &str) -> Result<Option<ClusterId>, DomainError> {
        Ok(self.assignments.lock().unwrap().get(iri).copied())
    }

    async fn cluster_mates(&self, iri: &str) -> Result<Vec<String>, DomainError> {
        let assignments = self.assignments.lock().unwrap();
        let Some(cluster) = assignments.get(iri).copied() else {
            return Ok(Vec::new());
        };
        Ok(assignments
            .iter()
            .filter(|(mate, c)| **c == cluster && mate.as_str() != iri)
            .map(|(mate, _)| mate.clone())
            .collect())
    }

    async fn replace_clusters(&self, pairs: Vec<(String, ClusterId)>) -> Result<(), DomainError> {
        *self.assignments.lock().unwrap() = pairs.into_iter().collect();
        Ok(())
    }

    async fn all_assignments(&self) -> Result<Vec<(String, ClusterId)>, DomainError> {
        Ok(self
            .assignments
            .lock()
            .unwrap()
            .iter()
            .map(|(iri, cluster)| (iri.clone(), *cluster))
            .collect())
    }
}
