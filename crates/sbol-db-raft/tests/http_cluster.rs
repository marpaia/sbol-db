use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use openraft::{BasicNode, Config};
use sbol_db_core::NewUser;
use sbol_db_raft::{NodeIdentity, ReplicatedConfigStore, RocksRaftNode, RocksRaftNodeConfig};
use sbol_db_storage::{ConfigStore, TokenStore, UserStore};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

async fn wait_for_linearizable_leader(
    nodes: &BTreeMap<u64, openraft::Raft<sbol_db_raft::TypeConfig>>,
) -> u64 {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            for (id, node) in nodes {
                if node.ensure_linearizable().await.is_ok() {
                    return *id;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("HTTP cluster should elect a quorum-confirmed leader")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn three_nodes_replicate_and_fail_over_across_http_transport() {
    let token = "test-only-shared-raft-secret";
    let cluster_id = Uuid::from_u128(700);
    let config = Arc::new(
        Config {
            cluster_name: "sbol-db-http-ha-test".to_owned(),
            heartbeat_interval: 50,
            election_timeout_min: 150,
            election_timeout_max: 300,
            ..Default::default()
        }
        .validate()
        .unwrap(),
    );

    let mut listeners = BTreeMap::new();
    let mut addresses = BTreeMap::new();
    for id in 1..=3 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        addresses.insert(id, format!("http://{}", listener.local_addr().unwrap()));
        listeners.insert(id, listener);
    }

    let mut directories: Vec<TempDir> = Vec::new();
    let mut runtimes = BTreeMap::new();
    let mut nodes = BTreeMap::new();
    let mut states = BTreeMap::new();
    for id in 1..=3 {
        let directory = tempfile::tempdir().unwrap();
        let runtime = RocksRaftNode::open(RocksRaftNodeConfig {
            identity: NodeIdentity {
                cluster_id,
                node_id: id,
            },
            storage_root: directory.path().join("node"),
            bearer_token: token.to_owned(),
            raft: config.clone(),
            peer_routes: BTreeMap::new(),
        })
        .await
        .unwrap();
        let raft = runtime.raft().clone();
        let state = runtime.state_machine().clone();
        directories.push(directory);
        states.insert(id, state);
        nodes.insert(id, raft);
        runtimes.insert(id, runtime);
    }

    let mut servers = BTreeMap::new();
    for id in 1..=3 {
        let listener = listeners.remove(&id).unwrap();
        let router = runtimes[&id].rpc_router();
        servers.insert(
            id,
            tokio::spawn(async move { axum::serve(listener, router).await }),
        );
    }

    let unauthenticated = reqwest::Client::new()
        .post(format!("{}/raft/vote", addresses[&1]))
        .bearer_auth("wrong-secret")
        .body("this is intentionally not JSON")
        .send()
        .await
        .unwrap();
    assert_eq!(
        unauthenticated.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "authentication must run before request body parsing"
    );
    let larger_than_axum_default = reqwest::Client::new()
        .post(format!("{}/raft/vote", addresses[&1]))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(vec![b'x'; 3 * 1024 * 1024])
        .send()
        .await
        .unwrap();
    assert_ne!(
        larger_than_axum_default.status(),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        "normal OpenRaft snapshot chunks exceed Axum's default body limit"
    );

    let members = addresses
        .iter()
        .map(|(id, address)| (*id, BasicNode::new(address)))
        .collect::<BTreeMap<_, _>>();
    nodes[&1].initialize(members).await.unwrap();
    let first_leader = wait_for_linearizable_leader(&nodes).await;
    let config_store = ReplicatedConfigStore::new(
        nodes[&first_leader].clone(),
        states[&first_leader].clone(),
        Uuid::from_u128(701),
    );
    let config_request = Uuid::from_u128(707);
    config_store
        .set_with_request_id(config_request, "theme", &json!({"dark": true}))
        .await
        .unwrap();
    let new_user = NewUser {
        username: "ada".to_owned(),
        name: "Ada Lovelace".to_owned(),
        email: "ada@example.test".to_owned(),
        affiliation: Some("Analytical Engine".to_owned()),
        password_hash: "argon2-test-hash".to_owned(),
        graph_uri: "https://example.test/user/ada".to_owned(),
        is_admin: true,
        is_curator: false,
        is_member: true,
    };
    let user_request = Uuid::from_u128(708);
    let created_user = runtimes[&first_leader]
        .user_store(Uuid::from_u128(709))
        .create_user_with_request_id(user_request, new_user.clone())
        .await
        .unwrap();
    runtimes[&first_leader]
        .token_store(Uuid::from_u128(704))
        .issue("sha3-test-token", created_user.id)
        .await
        .unwrap();

    servers.remove(&first_leader).unwrap().abort();
    nodes[&first_leader].shutdown().await.unwrap();

    let replacement = wait_for_linearizable_leader(&nodes).await;
    assert_ne!(replacement, first_leader);
    let replacement_config = ReplicatedConfigStore::new(
        nodes[&replacement].clone(),
        states[&replacement].clone(),
        Uuid::from_u128(702),
    );
    ReplicatedConfigStore::new(
        nodes[&replacement].clone(),
        states[&replacement].clone(),
        Uuid::from_u128(701),
    )
    .set_with_request_id(config_request, "theme", &json!({"dark": true}))
    .await
    .unwrap();
    assert_eq!(
        replacement_config.get("theme").await.unwrap(),
        Some(json!({"dark": true}))
    );
    replacement_config.set("mail", &json!(true)).await.unwrap();
    let replacement_tokens = runtimes[&replacement].token_store(Uuid::from_u128(705));
    assert_eq!(
        replacement_tokens.resolve("sha3-test-token").await.unwrap(),
        Some(created_user.id)
    );
    let revoke_request = Uuid::from_u128(706);
    assert!(replacement_tokens
        .revoke_with_request_id(revoke_request, "sha3-test-token")
        .await
        .unwrap());
    assert!(
        replacement_tokens
            .revoke_with_request_id(revoke_request, "sha3-test-token")
            .await
            .unwrap(),
        "an exact retry must return the first result, not re-evaluate state"
    );
    assert_eq!(
        replacement_tokens.resolve("sha3-test-token").await.unwrap(),
        None
    );
    let replacement_users = runtimes[&replacement].user_store(Uuid::from_u128(709));
    assert_eq!(
        replacement_users
            .create_user_with_request_id(user_request, new_user)
            .await
            .unwrap(),
        created_user,
        "a create retry after failover must return the original generated id and timestamps"
    );
    let duplicate = replacement_users
        .create_user(NewUser {
            username: "ada".to_owned(),
            name: "Different Account".to_owned(),
            email: "different@example.test".to_owned(),
            affiliation: None,
            password_hash: "different-hash".to_owned(),
            graph_uri: "https://example.test/user/different".to_owned(),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await;
    assert!(duplicate.is_err(), "username uniqueness must be replicated");
    assert_eq!(
        replacement_users
            .find_by_email_or_username("ada@example.test")
            .await
            .unwrap(),
        Some(created_user.clone())
    );
    assert!(replacement_users.any_admin().await.unwrap());
    replacement_users
        .set_password_hash(created_user.id, "argon2-rehashed")
        .await
        .unwrap();
    replacement_users
        .set_reset_link(created_user.id, Some("one-time-link"))
        .await
        .unwrap();
    let consume_request = Uuid::from_u128(710);
    let consumed = replacement_users
        .consume_reset_link_with_request_id(consume_request, "one-time-link")
        .await
        .unwrap();
    assert_eq!(
        replacement_users
            .consume_reset_link_with_request_id(consume_request, "one-time-link")
            .await
            .unwrap(),
        consumed,
        "a destructive transition retry must return the first response"
    );
    let mut updated = consumed.unwrap();
    updated.name = "Augusta Ada King".to_owned();
    let updated = replacement_users.update_user(&updated).await.unwrap();
    assert_eq!(updated.name, "Augusta Ada King");
    let delete_request = Uuid::from_u128(711);
    assert!(replacement_users
        .delete_user_with_request_id(delete_request, created_user.id)
        .await
        .unwrap());
    assert!(
        replacement_users
            .delete_user_with_request_id(delete_request, created_user.id)
            .await
            .unwrap(),
        "a delete retry must return the first response"
    );
    assert!(!replacement_users.any_admin().await.unwrap());

    for (id, node) in &nodes {
        if *id != first_leader {
            node.shutdown().await.unwrap();
        }
    }
    for (_, server) in servers {
        server.abort();
    }
    drop(directories);
}
