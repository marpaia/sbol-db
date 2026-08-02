//! Deployment-specific registry identity.
//!
//! SynBioHub graph and object IRIs are persistent data, not presentation URLs.
//! A migrated registry therefore carries one immutable database prefix and one
//! public graph IRI through every identity, ACL, minting, mutation, visibility,
//! and download service. Defaults preserve the historical local-development
//! behavior; production supplies the values from its migrated configuration.

use sbol_db_core::{DomainError, IriString};

pub const DEFAULT_DATABASE_PREFIX: &str = "http://synbiohub.org/";
pub const DEFAULT_PUBLIC_GRAPH: &str = "http://synbiohub.org/public";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryNamespace {
    database_prefix: String,
    public_graph: String,
}

impl Default for RegistryNamespace {
    fn default() -> Self {
        Self {
            database_prefix: DEFAULT_DATABASE_PREFIX.to_owned(),
            public_graph: DEFAULT_PUBLIC_GRAPH.to_owned(),
        }
    }
}

impl RegistryNamespace {
    /// Validate and normalize a deployment namespace. The database prefix is
    /// always stored with a trailing slash so concatenation cannot mint a split
    /// namespace accidentally.
    pub fn new(
        database_prefix: impl Into<String>,
        public_graph: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let mut database_prefix = database_prefix.into();
        if !database_prefix.ends_with('/') {
            database_prefix.push('/');
        }
        let public_graph = public_graph.into();
        IriString::new(database_prefix.clone())?;
        IriString::new(public_graph.clone())?;
        Ok(Self {
            database_prefix,
            public_graph,
        })
    }

    /// Build a namespace whose public graph is `<database-prefix>public`.
    pub fn from_database_prefix(database_prefix: impl Into<String>) -> Result<Self, DomainError> {
        let mut database_prefix = database_prefix.into();
        if !database_prefix.ends_with('/') {
            database_prefix.push('/');
        }
        let public_graph = format!("{database_prefix}public");
        Self::new(database_prefix, public_graph)
    }

    pub fn database_prefix(&self) -> &str {
        &self.database_prefix
    }

    pub fn public_graph(&self) -> &str {
        &self.public_graph
    }

    pub fn user_graph(&self, username: &str) -> String {
        format!("{}user/{username}", self.database_prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_prefix_and_keeps_explicit_public_graph() {
        let namespace = RegistryNamespace::new(
            "https://registry.example",
            "https://data.example/graphs/public",
        )
        .expect("namespace");
        assert_eq!(namespace.database_prefix(), "https://registry.example/");
        assert_eq!(
            namespace.public_graph(),
            "https://data.example/graphs/public"
        );
        assert_eq!(
            namespace.user_graph("alice"),
            "https://registry.example/user/alice"
        );
    }
}
