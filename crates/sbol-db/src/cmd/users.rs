//! `sbol-db users` — safe account inventory and offline administrator recovery.

use std::sync::Arc;

use anyhow::{bail, Result};
use sbol_db_core::{User, UserId};
use sbol_db_storage::UserStore;
use serde::Serialize;

use crate::cli::UsersAction;
use crate::output::print_json;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct SafeUser {
    id: UserId,
    username: String,
    name: String,
    email: String,
    affiliation: Option<String>,
    graph_uri: String,
    is_admin: bool,
    is_curator: bool,
    is_member: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&User> for SafeUser {
    fn from(user: &User) -> Self {
        Self {
            id: user.id,
            username: user.username.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            affiliation: user.affiliation.clone(),
            graph_uri: user.graph_uri.clone(),
            is_admin: user.is_admin,
            is_curator: user.is_curator,
            is_member: user.is_member,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct SoleAdminReport {
    changed: bool,
    target: SafeUser,
    previous_admins: Vec<SafeUser>,
    current_admins: Vec<SafeUser>,
    total_users: usize,
}

pub async fn run(users: Arc<dyn UserStore>, action: UsersAction) -> Result<()> {
    match action {
        UsersAction::List { admins_only } => {
            let rows = users.list_users().await?;
            let safe = rows
                .iter()
                .filter(|user| !admins_only || user.is_admin)
                .map(SafeUser::from)
                .collect::<Vec<_>>();
            print_json(&safe)
        }
        UsersAction::SetSoleAdmin {
            username,
            email,
            confirmation,
        } => {
            let report = set_sole_admin(users.as_ref(), &username, &email, &confirmation).await?;
            print_json(&report)
        }
    }
}

async fn set_sole_admin(
    users: &dyn UserStore,
    username: &str,
    email: &str,
    confirmation: &str,
) -> Result<SoleAdminReport> {
    let expected_confirmation = format!("set-sole-admin:{username}:{email}");
    if confirmation != expected_confirmation {
        bail!("refusing administrator change: --confirmation must equal `{expected_confirmation}`");
    }

    let before = users.list_users().await?;
    let Some(target) = before.iter().find(|user| user.username == username) else {
        bail!("no account has username `{username}`");
    };
    if target.email != email {
        bail!(
            "account `{username}` does not have the supplied email address; no changes were made"
        );
    }
    let target_id = target.id;
    let previous_admins = before
        .iter()
        .filter(|user| user.is_admin)
        .map(SafeUser::from)
        .collect::<Vec<_>>();
    let changed = previous_admins.len() != 1 || previous_admins[0].id != target_id;

    users.set_sole_admin(target_id).await?;

    let after = users.list_users().await?;
    let current_admins = after
        .iter()
        .filter(|user| user.is_admin)
        .map(SafeUser::from)
        .collect::<Vec<_>>();
    if current_admins.len() != 1 || current_admins[0].id != target_id {
        bail!("administrator update did not reconcile to the requested sole account");
    }
    let target = after
        .iter()
        .find(|user| user.id == target_id)
        .expect("the atomic store operation preserves its target account");

    Ok(SoleAdminReport {
        changed,
        target: SafeUser::from(target),
        previous_admins,
        current_admins,
        total_users: after.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbol_db_app::{memory::InMemoryUserStore, AuthService};
    use sbol_db_core::NewUser;

    async fn store_with_two_users() -> (InMemoryUserStore, UserId, UserId) {
        let store = InMemoryUserStore::new();
        let old_admin = store
            .create_user(NewUser {
                username: "old-admin".into(),
                name: "Old Admin".into(),
                email: "old@example.org".into(),
                affiliation: None,
                password_hash: "password-hash-must-not-leak".into(),
                graph_uri: AuthService::graph_uri("old-admin"),
                is_admin: true,
                is_curator: true,
                is_member: true,
            })
            .await
            .unwrap();
        let target = store
            .create_user(NewUser {
                username: "marpaia".into(),
                name: "Mike Arpaia".into(),
                email: "mike@arpaia.co".into(),
                affiliation: None,
                password_hash: "target-password-hash-must-not-leak".into(),
                graph_uri: AuthService::graph_uri("marpaia"),
                is_admin: false,
                is_curator: false,
                is_member: false,
            })
            .await
            .unwrap();
        store
            .set_reset_link(target.id, Some("reset-link-must-not-leak"))
            .await
            .unwrap();
        (store, old_admin.id, target.id)
    }

    #[tokio::test]
    async fn confirmation_and_email_must_match_before_roles_change() {
        let (store, old_admin_id, target_id) = store_with_two_users().await;

        set_sole_admin(&store, "marpaia", "mike@arpaia.co", "wrong")
            .await
            .expect_err("wrong confirmation is rejected");
        set_sole_admin(
            &store,
            "marpaia",
            "wrong@example.org",
            "set-sole-admin:marpaia:wrong@example.org",
        )
        .await
        .expect_err("wrong email is rejected");

        assert!(
            store
                .get_by_id(old_admin_id)
                .await
                .unwrap()
                .unwrap()
                .is_admin
        );
        assert!(!store.get_by_id(target_id).await.unwrap().unwrap().is_admin);
    }

    #[tokio::test]
    async fn successful_report_is_idempotent_and_excludes_credentials() {
        let (store, old_admin_id, target_id) = store_with_two_users().await;
        let confirmation = "set-sole-admin:marpaia:mike@arpaia.co";

        let report = set_sole_admin(&store, "marpaia", "mike@arpaia.co", confirmation)
            .await
            .unwrap();
        assert!(report.changed);
        assert_eq!(report.current_admins.len(), 1);
        assert_eq!(report.current_admins[0].id, target_id);
        assert!(
            !store
                .get_by_id(old_admin_id)
                .await
                .unwrap()
                .unwrap()
                .is_admin
        );

        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains("password-hash-must-not-leak"));
        assert!(!encoded.contains("reset-link-must-not-leak"));

        let repeated = set_sole_admin(&store, "marpaia", "mike@arpaia.co", confirmation)
            .await
            .unwrap();
        assert!(!repeated.changed);
        assert_eq!(repeated.current_admins[0].id, target_id);
    }
}
