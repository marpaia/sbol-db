//! Backend-neutral conformance scenarios for `sbol-db-storage`.
//!
//! Each scenario drives a storage backend purely through the trait surface and
//! asserts the observable contract every implementation must honor: import and
//! derived-view reads, the graph set-semantics rule, ontology load and closure
//! queries, the job-queue lifecycle, ACL-scoped reads hiding another user's
//! private graph, the native ranked text search (its SBOLExplorer ranking
//! rules and its scope enforcement), and the download path (the recursive and
//! non-recursive object closure, and the byte-stream serializers that render a
//! closure to GenBank, FASTA, GFF3, and an OMEX archive). A backend crate wires
//! these into its own test harness by
//! assembling a fresh, empty [`AppServices`] over the backend and calling
//! [`run_all`] (or an individual store-level scenario against a bare store).
//!
//! Scenarios assume they start against an empty store; they scope their reads
//! to the graphs and keys they create so [`run_all`] can run them in sequence
//! against one store without cross-contamination.

use std::sync::Arc;
use std::time::Duration;

use std::io::Read;

use async_trait::async_trait;
use sbol_db_app::{
    AppServices, AttachmentService, AuthService, Downloader, EditService, FacetedSearch,
    FederationError, FederationService, FsBlobStore, JoinPayload, JoinResponse, MutationError,
    MutationService, PermissionService, Registration, SubmissionService, SubmitRequest,
    WebOfRegistriesClient, WorInstance, PUBLIC_GRAPH,
};
use sbol_db_core::{
    Direction, DomainError, GraphId, IriString, NeighborhoodQuery, NewUser, ObjectTerm,
    SerializationFormat, SubjectTerm, Triple,
};
use sbol_db_search::pagerank::pagerank;
use sbol_db_search::ranked_text::IndexedPart;
use sbol_db_search::{cluster_sequences, AlignOptions, ClusterId};
use sbol_db_server::{export_subject_rdf, serialize_closure, serialize_gff3, serialize_omex};
use sbol_db_sparql::{GraphScope, SparqlOptions};
use sbol_db_storage::{
    BlobStore, EnqueueOutcome, GraphWriteMode, ImportInput, ImportOverwrite, JobQueue, JobStatus,
    ListJobsFilter, ListObjectsFilter, NewJob, RankRow, SbolStore, SequenceSearchOptions,
    DEFAULT_QUEUE, SBH_OWNED_BY,
};
use sha1::{Digest, Sha1};

/// A self-contained SBOL3 document: one Component referencing one Sequence.
const SIMPLE_COMPONENT_TTL: &str = r#"
BASE <https://example.org/sbol-db/conformance/>
PREFIX :     <https://example.org/sbol-db/conformance/>
PREFIX SBO:  <https://identifiers.org/SBO:>
PREFIX SO:   <https://identifiers.org/SO:>
PREFIX EDAM: <https://identifiers.org/edam:>
PREFIX sbol: <http://sbols.org/v3#>

:promoter_j23119
    a                  sbol:Component ;
    sbol:displayId     "promoter_j23119" ;
    sbol:name          "J23119 promoter" ;
    sbol:hasNamespace  <https://example.org/sbol-db/conformance> ;
    sbol:type          SBO:0000251 ;
    sbol:role          SO:0000167 ;
    sbol:hasSequence   :promoter_j23119_seq .

:promoter_j23119_seq
    a                  sbol:Sequence ;
    sbol:displayId     "promoter_j23119_seq" ;
    sbol:hasNamespace  <https://example.org/sbol-db/conformance> ;
    sbol:elements      "ttgacagctagctcagtcctaggtataatgctagc" ;
    sbol:encoding      EDAM:format_1207 .
"#;

/// A small Sequence Ontology slice with an `is_a` chain ending at `promoter`.
const TINY_SO_OBO: &str = r#"format-version: 1.4
data-version: conformance
default-namespace: sequence

[Term]
id: SO:0000110
name: sequence_feature

[Term]
id: SO:0000001
name: region
is_a: SO:0000110 ! sequence_feature

[Term]
id: SO:0001055
name: transcriptional_cis_regulatory_region
is_a: SO:0000001 ! region

[Term]
id: SO:0000167
name: promoter
is_a: SO:0001055 ! transcriptional_cis_regulatory_region
"#;

const SO_REGION_IRI: &str = "http://purl.obolibrary.org/obo/SO_0000001";
const SO_PROMOTER_IRI: &str = "http://purl.obolibrary.org/obo/SO_0000167";

/// Run every scenario in sequence against one facade. The store and job queue
/// the facade bundles must start empty.
///
/// The facade carries the store and job queue the store-level scenarios drive
/// plus the [`AclService`](sbol_db_app::AclService) and SPARQL engine the
/// ACL-scope gate needs, so a backend wires up one [`AppServices`] and passes
/// it here.
pub async fn run_all(app: &AppServices) {
    let store = app.store.as_ref();
    let jobs = app.jobs.as_ref();
    import_and_read_back(store).await;
    graph_set_semantics(store).await;
    neighborhood_walk(store).await;
    sequence_search(store).await;
    sequence_align(app).await;
    similar_sequences(app).await;
    cluster_recall();
    ranked_dup_penalty_active(app).await;
    ontology_roundtrip(store).await;
    job_queue_lifecycle(jobs).await;
    acl_scope_hides_private_graph(app).await;
    ranked_text_search(app).await;
    acl_scoped_search(app).await;
    download_closure_recursion(app).await;
    download_formats_roundtrip(app).await;
    blob_roundtrip().await;
    attachment_read_both_vocabs(app).await;
    collection_mint_roundtrip(app).await;
    overwrite_merge_modes(app).await;
    write_authz_matrix(app).await;
    user_crud(app).await;
    token_issue_resolve_revoke(app).await;
    legacy_sha1_rehash_on_login(app).await;
    config_store_roundtrip(app).await;
    config_roundtrip(app).await;
    admin_user_crud(app).await;
    wor_map_persisted(app).await;
    v1_v2_data_parity(app).await;
}

/// The durable configuration store's contract: an unset key reads back absent,
/// a write reads back verbatim, a later write to the same key overwrites the
/// earlier value (upsert), every entry is enumerable, and a delete removes it.
///
/// This is the backend-neutral parity for the `ConfigStore` each backend
/// implements, driven through the facade's `config` handle so Postgres, SQLite,
/// and RocksDB prove identical set/get/get_all/delete/upsert behavior.
pub async fn config_store_roundtrip(app: &AppServices) {
    let config = app.config.as_ref();
    const KEY: &str = "conformance:config:mail";
    const OTHER: &str = "conformance:config:theme";

    // An unset key reads back absent.
    assert!(
        config.get(KEY).await.expect("get unset").is_none(),
        "an unset key reads back as absent"
    );

    // A write reads back verbatim.
    let mail = serde_json::json!({ "fromAddress": "admin@example.org", "sendgridApiKey": "sg-1" });
    config.set(KEY, &mail).await.expect("set");
    assert_eq!(
        config.get(KEY).await.expect("get"),
        Some(mail),
        "a written value reads back verbatim"
    );

    // A second write to the same key overwrites the value (upsert).
    let mail2 = serde_json::json!({ "fromAddress": "ops@example.org" });
    config.set(KEY, &mail2).await.expect("overwrite");
    assert_eq!(
        config.get(KEY).await.expect("get after overwrite"),
        Some(mail2.clone()),
        "a later write to the same key overwrites the earlier value"
    );

    // A second key coexists, and get_all enumerates both.
    let theme = serde_json::json!({ "name": "dark" });
    config.set(OTHER, &theme).await.expect("set other");
    let all: std::collections::HashMap<String, serde_json::Value> = config
        .get_all()
        .await
        .expect("get_all")
        .into_iter()
        .map(|e| (e.key, e.value))
        .collect();
    assert_eq!(all.get(KEY), Some(&mail2), "get_all carries the first key");
    assert_eq!(
        all.get(OTHER),
        Some(&theme),
        "get_all carries the second key"
    );

    // Delete removes a key; the other survives.
    config.delete(KEY).await.expect("delete");
    assert!(
        config.get(KEY).await.expect("get after delete").is_none(),
        "a deleted key reads back as absent"
    );
    assert_eq!(
        config.get(OTHER).await.expect("get survivor"),
        Some(theme),
        "an unrelated key survives a delete"
    );

    // Leave the store as we found it for any scenario that follows.
    config.delete(OTHER).await.expect("cleanup other");
}

/// The admin-gated [`ConfigService`](sbol_db_app::ConfigService) over each
/// backend's durable store: an unset section reads back absent, a non-admin
/// write or delete is refused and persists nothing, an admin write reads back
/// verbatim and a later admin write overwrites it, and an admin delete removes
/// it.
///
/// This is the parity for the service layer the `/admin` config routes call,
/// where `config_store_roundtrip` covers the raw store trait. Both run so each
/// backend proves the durable store honors the gate the same way.
pub async fn config_roundtrip(app: &AppServices) {
    let config = app.config_service();
    const KEY: &str = "conformance:svc:mail";

    // An unset section defaults to absent.
    assert!(
        config.get(KEY).await.expect("get unset").is_none(),
        "an unset config section reads back as absent"
    );

    // A non-admin write is refused and leaves the section absent.
    let value = serde_json::json!({ "fromAddress": "ops@example.org" });
    assert!(
        config.set(false, KEY, &value).await.is_err(),
        "a non-admin write is refused"
    );
    assert!(
        config
            .get(KEY)
            .await
            .expect("get after refused write")
            .is_none(),
        "a refused write persists nothing"
    );

    // An admin write persists and reads back verbatim.
    config.set(true, KEY, &value).await.expect("admin set");
    assert_eq!(
        config.get(KEY).await.expect("get after set"),
        Some(value),
        "an admin write reads back verbatim"
    );

    // A later admin write to the same section overwrites the value.
    let updated = serde_json::json!({ "fromAddress": "admin@example.org" });
    config
        .set(true, KEY, &updated)
        .await
        .expect("admin overwrite");
    assert_eq!(
        config.get(KEY).await.expect("get after overwrite"),
        Some(updated),
        "a later admin write overwrites the earlier value"
    );

    // A non-admin delete is refused; an admin delete removes the section.
    assert!(
        config.delete(false, KEY).await.is_err(),
        "a non-admin delete is refused"
    );
    config.delete(true, KEY).await.expect("admin delete");
    assert!(
        config.get(KEY).await.expect("get after delete").is_none(),
        "an admin delete removes the section"
    );
}

/// A stubbed Web of Registries sync persists its `uriPrefix -> instanceUrl` map
/// in the backend's durable [`ConfigStore`](sbol_db_storage::ConfigStore).
///
/// A [`FederationService`] over `app`'s durable config store, paired with a stub
/// client that returns a canned instance list (so no network is touched), joins
/// then syncs; a second, independently constructed service over the same store
/// reads the map back, proving the federation state lives in the store and not
/// in the service instance. This certifies federation persistence on each
/// backend.
pub async fn wor_map_persisted(app: &AppServices) {
    let instances = vec![
        WorInstance {
            uri_prefix: "https://reg-a.conformance.example.org/".to_owned(),
            instance_url: "https://reg-a.conformance.example.org/".to_owned(),
        },
        WorInstance {
            uri_prefix: "https://reg-b.conformance.example.org/".to_owned(),
            // A trailing slash the sync must strip.
            instance_url: "https://reg-b.conformance.example.org".to_owned(),
        },
    ];
    let client: Arc<dyn WebOfRegistriesClient> = Arc::new(StubWorClient {
        instances: instances.clone(),
    });
    let federation = FederationService::new(app.config.clone(), client);

    // Join, then pull the instance list into the durable map.
    federation
        .federate(
            true,
            "admin@conformance.example.org",
            "https://wor.conformance.example.org/",
        )
        .await
        .expect("federate");
    let applied = federation.retrieve().await.expect("retrieve");
    assert_eq!(applied, 2, "the sync applies both advertised instances");

    // A fresh service over the same store reads the persisted map, so the state
    // is in the store, not the service.
    let reader = FederationService::new(app.config.clone(), Arc::new(StubWorClient::default()));
    let map: std::collections::HashMap<String, String> = reader
        .registries()
        .await
        .expect("registries")
        .into_iter()
        .collect();
    assert_eq!(
        map.get("https://reg-a.conformance.example.org/"),
        Some(&"https://reg-a.conformance.example.org".to_owned()),
        "the first instance is persisted with its trailing slash stripped"
    );
    assert_eq!(
        map.get("https://reg-b.conformance.example.org/"),
        Some(&"https://reg-b.conformance.example.org".to_owned()),
        "the second instance is persisted"
    );

    // Leave the store as we found it: the keys the federate/sync path writes
    // (mirrors the constants in sbol-db-app's federation module).
    let config = app.config.as_ref();
    for key in [
        "webOfRegistries",
        "webOfRegistriesUrl",
        "webOfRegistriesId",
        "webOfRegistriesSecret",
        "administratorEmail",
    ] {
        config.delete(key).await.expect("cleanup federation key");
    }
}

/// V1 and V2 are two presentations of the one facade, so a single write is
/// visible identically through the read verbs each surface uses.
///
/// One object is written through the shared [`SubmissionService`] verb (the
/// facade verb both the V1 `POST /submit` and the V2 `POST /api/v2/collections`
/// routes call). It is then read back two ways, each the verb its surface uses:
///
/// * the V1-shaped read is the `GetTopLevelMetadata` SPARQL query run under the
///   caller's authorized [`GraphScope`] (what the V1 `.../metadata` route runs),
/// * the V2-shaped read is [`export_subject_rdf`] of the subject's closure (what
///   the V2 `GET /api/v2/objects/{iri}` route serves under `Accept: text/turtle`
///   for a verbatim submission, which carries no derived object record).
///
/// Both reads surface the same object identity (type, displayId, version,
/// title) from the one write, and both hide the object from a caller outside its
/// scope through the exact gate each surface uses: the V1 read returns no rows
/// under a foreign scope, and the V2 gate (`AclService::graph_of_subject`)
/// resolves a graph that foreign scope does not name. This certifies there is no
/// divergence between the two views on any backend.
pub async fn v1_v2_data_parity(app: &AppServices) {
    const OWNER: &str = "conformance_parity_owner";
    const OWNER_GRAPH: &str = "http://synbiohub.org/user/conformance_parity_owner";
    const INTRUDER_GRAPH: &str = "http://synbiohub.org/user/conformance_parity_intruder";
    const COMPONENT: &str = "http://synbiohub.org/user/conformance_parity_owner/paritysub/cd/1";

    // The one write, through the shared submission verb both surfaces call.
    let submissions = SubmissionService::new(app.store.clone());
    let outcome = submissions
        .submit(SubmitRequest {
            owner: OWNER.to_owned(),
            id: "paritysub".to_owned(),
            version: "1".to_owned(),
            name: Some("Parity Roundtrip".to_owned()),
            description: None,
            creator_name: None,
            citations: Vec::new(),
            body: SUBMISSION_FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("submit mints the parity object");

    // The caller's server-computed scope; the V1 read runs under it and the V2
    // scope gate is checked against it.
    let scope = app
        .acl_service
        .compute_scope(Some(OWNER_GRAPH))
        .await
        .expect("owner scope");

    // The V1-shaped read: the metadata SPARQL under the owner's scope.
    let v1 = v1_metadata_identity(app, COMPONENT, &scope).await;
    // The V2-shaped read: the subject's RDF closure, the verbatim SBOL2 view.
    let v2 = export_subject_rdf(
        app.store.as_ref(),
        COMPONENT,
        SerializationFormat::Turtle,
        false,
    )
    .await
    .expect("V2 subject RDF");

    // The write's identity, as the fixture minted it, surfaces on both surfaces.
    let expected = ObjectIdentity {
        display_id: "cd".to_owned(),
        version: "1".to_owned(),
        title: "My Component".to_owned(),
        type_local: "ComponentDefinition".to_owned(),
    };
    assert_eq!(
        v1, expected,
        "the V1 metadata read surfaces the written identity"
    );
    for signal in [&expected.display_id, &expected.version, &expected.title] {
        assert!(
            v2.contains(signal.as_str()),
            "the V2 RDF read carries the written signal {signal:?}: {v2}"
        );
    }
    assert!(
        v2.contains(&expected.type_local),
        "the V2 RDF read carries the written type: {v2}"
    );

    // Both surfaces hide the object from a caller outside its scope, each through
    // its own gate. The intruder's scope names neither the object's graph nor the
    // public graph.
    let intruder_scope = app
        .acl_service
        .compute_scope(Some(INTRUDER_GRAPH))
        .await
        .expect("intruder scope");
    let hidden = v1_metadata_rows(app, COMPONENT, &intruder_scope).await;
    assert_eq!(
        hidden, 0,
        "the V1 metadata read returns no rows under a foreign scope"
    );
    let graph = app
        .acl_service
        .graph_of_subject(COMPONENT)
        .await
        .expect("graph of subject")
        .expect("the minted object resolves to a graph");
    assert!(
        !scope_names(&intruder_scope, &graph),
        "the V2 scope gate excludes the object's graph from a foreign scope"
    );

    app.store
        .graph_store_clear(&outcome.graph_iri)
        .await
        .expect("clear the parity submission graph");
}

/// The identity of one object as the V1 `.../metadata` route surfaces it: the
/// fields a `GetTopLevelMetadata` query binds.
#[derive(Debug, PartialEq, Eq)]
struct ObjectIdentity {
    display_id: String,
    version: String,
    title: String,
    type_local: String,
}

/// Run the classic `GetTopLevelMetadata` SPARQL for `subject` under `scope` and
/// parse the single row's identity, the way the V1 metadata route reads.
async fn v1_metadata_identity(
    app: &AppServices,
    subject: &str,
    scope: &GraphScope,
) -> ObjectIdentity {
    let body = v1_metadata_body(app, subject, scope).await;
    let value: serde_json::Value = serde_json::from_slice(&body).expect("SPARQL-results JSON");
    let binding = value["results"]["bindings"]
        .as_array()
        .and_then(|rows| rows.first())
        .expect("one metadata row");
    let cell = |var: &str| {
        binding[var]["value"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let type_iri = cell("type");
    ObjectIdentity {
        display_id: cell("displayId"),
        version: cell("version"),
        title: cell("name"),
        type_local: type_iri
            .rsplit(['#', '/'])
            .next()
            .unwrap_or(&type_iri)
            .to_owned(),
    }
}

/// The number of `GetTopLevelMetadata` rows `subject` binds under `scope`; zero
/// means the V1 read cannot see it.
async fn v1_metadata_rows(app: &AppServices, subject: &str, scope: &GraphScope) -> usize {
    let body = v1_metadata_body(app, subject, scope).await;
    let value: serde_json::Value = serde_json::from_slice(&body).expect("SPARQL-results JSON");
    value["results"]["bindings"]
        .as_array()
        .map(|rows| rows.len())
        .unwrap_or(0)
}

/// Execute the classic `GetTopLevelMetadata` SPARQL for `subject` under `scope`,
/// returning the serialized SPARQL-results body.
async fn v1_metadata_body(app: &AppServices, subject: &str, scope: &GraphScope) -> Vec<u8> {
    let query = format!(
        "PREFIX sbol2: <http://sbols.org/v2#>
PREFIX dcterms: <http://purl.org/dc/terms/>
SELECT DISTINCT ?displayId ?version ?name ?type WHERE {{
    <{subject}> a ?type .
    OPTIONAL {{ <{subject}> sbol2:displayId ?displayId . }}
    OPTIONAL {{ <{subject}> sbol2:version ?version . }}
    OPTIONAL {{ <{subject}> dcterms:title ?name . }}
}}"
    );
    let options = SparqlOptions {
        authorized_graphs: scope.clone(),
        ..SparqlOptions::default()
    };
    app.sparql
        .execute(&query, None, None, &options)
        .await
        .expect("V1 metadata SPARQL")
        .payload
        .body
}

/// A stub [`WebOfRegistriesClient`] that returns a canned instance list and a
/// fixed join response, so the federation scenario runs with no network.
#[derive(Default)]
struct StubWorClient {
    instances: Vec<WorInstance>,
}

#[async_trait]
impl WebOfRegistriesClient for StubWorClient {
    async fn join(
        &self,
        _wor_url: &str,
        _payload: &JoinPayload,
    ) -> Result<JoinResponse, FederationError> {
        Ok(JoinResponse {
            id: "conformance-instance".to_owned(),
            update_secret: "conformance-secret".to_owned(),
        })
    }

    async fn fetch_instances(&self, _wor_url: &str) -> Result<Vec<WorInstance>, FederationError> {
        Ok(self.instances.clone())
    }

    async fn fetch_sbol(&self, _object_url: &str) -> Result<String, FederationError> {
        Ok(String::new())
    }
}

/// The password salt the legacy-rehash scenario seeds its digest with and
/// authenticates under, standing in for a migrated instance's
/// `SBOL_DB_PASSWORD_SALT`.
const CONFORMANCE_SALT: &str = "conformance-password-salt";

/// Account persistence round-trips: an account is created, resolves by both its
/// email and its username to the same id, fetches by id, and a profile update
/// is durable.
pub async fn user_crud(app: &AppServices) {
    let users = app.users.as_ref();
    let created = users
        .create_user(NewUser {
            username: "crud_user".into(),
            name: "Crud User".into(),
            email: "crud_user@example.org".into(),
            affiliation: Some("Lab".into()),
            password_hash: "$argon2id$placeholder".into(),
            graph_uri: AuthService::graph_uri("crud_user"),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .expect("create_user");

    // The login lookup resolves by either identifier to the same account.
    let by_email = users
        .find_by_email_or_username("crud_user@example.org")
        .await
        .expect("find by email")
        .expect("account matches by email");
    assert_eq!(by_email.id, created.id, "email resolves to the created id");
    let by_username = users
        .find_by_email_or_username("crud_user")
        .await
        .expect("find by username")
        .expect("account matches by username");
    assert_eq!(
        by_username.id, created.id,
        "username resolves to the created id"
    );

    // Fetch by id round-trips the stored fields.
    let by_id = users
        .get_by_id(created.id)
        .await
        .expect("get_by_id")
        .expect("account fetches by id");
    assert_eq!(by_id.email, "crud_user@example.org");
    assert!(!by_id.is_admin, "created without the admin flag");

    // A profile update is durable across a re-read.
    let mut updated = by_id;
    updated.name = "Renamed User".into();
    updated.is_admin = true;
    users.update_user(&updated).await.expect("update_user");
    let reread = users
        .get_by_id(created.id)
        .await
        .expect("get_by_id after update")
        .expect("account still present");
    assert_eq!(reread.name, "Renamed User", "name update is durable");
    assert!(reread.is_admin, "membership-flag update is durable");
}

/// The admin user-management path end to end: the `AuthService.register` verb
/// the `/admin/createUser` route calls mints an account, a membership-flag
/// update is durable, and `delete_user` removes it so a second delete reports
/// the account already gone.
///
/// Where `user_crud` drives the raw `UserStore.create_user`, this drives the
/// register-then-delete flow the admin routes use, certifying `delete_user`
/// (which no read path exercises) on each backend.
pub async fn admin_user_crud(app: &AppServices) {
    let created = app
        .auth
        .register(Registration {
            username: "admin_crud_user".to_owned(),
            name: "Admin Crud User".to_owned(),
            email: "admin_crud_user@example.org".to_owned(),
            affiliation: None,
            password: "s3cret".to_owned(),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .expect("register mints the account");
    assert!(
        created.password_hash.starts_with("$argon2"),
        "register argon2-hashes the plaintext password"
    );

    // The account resolves by username and starts without the curator flag.
    let fetched = app
        .users
        .find_by_email_or_username("admin_crud_user")
        .await
        .expect("find")
        .expect("the registered account resolves by username");
    assert_eq!(fetched.id, created.id);
    assert!(!fetched.is_curator, "created without the curator flag");

    // A membership-flag update is durable.
    let mut updated = fetched;
    updated.is_curator = true;
    app.users
        .update_user(&updated)
        .await
        .expect("update the membership flag");
    let reread = app
        .users
        .get_by_id(created.id)
        .await
        .expect("get_by_id after update")
        .expect("account still present");
    assert!(reread.is_curator, "the curator-flag update is durable");

    // Delete removes the account; a second delete reports it already gone.
    assert!(
        app.users
            .delete_user(created.id)
            .await
            .expect("delete_user"),
        "deleting a present account reports success"
    );
    assert!(
        app.users
            .get_by_id(created.id)
            .await
            .expect("get_by_id after delete")
            .is_none(),
        "a deleted account no longer fetches by id"
    );
    assert!(
        !app.users
            .delete_user(created.id)
            .await
            .expect("second delete_user"),
        "a second delete reports the account already gone"
    );
}

/// The API-token lifecycle end to end: a minted token resolves to the account
/// it authenticates, an unknown token resolves to nothing, and a revoked token
/// stops resolving.
pub async fn token_issue_resolve_revoke(app: &AppServices) {
    let owner = app
        .users
        .create_user(NewUser {
            username: "token_user".into(),
            name: "Token User".into(),
            email: "token_user@example.org".into(),
            affiliation: None,
            password_hash: "$argon2id$placeholder".into(),
            graph_uri: AuthService::graph_uri("token_user"),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .expect("create_user");

    let token = app.auth.issue_token(owner.id).await.expect("issue_token");

    // The plaintext token resolves to its owner's id, and that id fetches the
    // owning account.
    let resolved = app
        .auth
        .resolve_token(&token)
        .await
        .expect("resolve_token")
        .expect("a live token resolves");
    assert_eq!(resolved, owner.id, "the token resolves to its owner");
    let resolved_user = app
        .users
        .get_by_id(resolved)
        .await
        .expect("get_by_id")
        .expect("the resolved id fetches the account");
    assert_eq!(resolved_user.username, "token_user");

    // An unknown token resolves to nothing.
    assert!(
        app.auth
            .resolve_token("not-a-real-token")
            .await
            .expect("resolve bogus token")
            .is_none(),
        "an unknown token authenticates no one"
    );

    // Revocation is durable: the token stops resolving.
    assert!(
        app.auth.revoke_token(&token).await.expect("revoke_token"),
        "revoking a live token reports success"
    );
    assert!(
        app.auth
            .resolve_token(&token)
            .await
            .expect("resolve after revoke")
            .is_none(),
        "a revoked token no longer resolves"
    );
}

/// A login against a classic SynBioHub `sha1(salt + sha1(password))` digest
/// succeeds and transparently upgrades the stored hash to argon2, so a migrated
/// instance's credentials keep working while silently strengthening.
pub async fn legacy_sha1_rehash_on_login(app: &AppServices) {
    let created = app
        .users
        .create_user(NewUser {
            username: "legacy_user".into(),
            name: "Legacy User".into(),
            email: "legacy_user@example.org".into(),
            affiliation: None,
            password_hash: classic_digest(CONFORMANCE_SALT, "hunter2"),
            graph_uri: AuthService::graph_uri("legacy_user"),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .expect("create_user");
    assert!(
        !created.password_hash.starts_with("$argon2"),
        "the account is seeded with a legacy digest"
    );

    let authed = app
        .auth
        .authenticate("legacy_user", "hunter2", CONFORMANCE_SALT)
        .await
        .expect("legacy login succeeds");
    assert_eq!(authed.id, created.id);

    let stored = app
        .users
        .get_by_id(created.id)
        .await
        .expect("get_by_id")
        .expect("account present after login");
    assert!(
        stored.password_hash.starts_with("$argon2"),
        "a successful legacy login rehashes the stored credential to argon2"
    );
}

/// The classic SynBioHub password digest `sha1(salt + sha1(password))`, each
/// `sha1` rendered as lowercase hex (`lib/db.js`), for seeding a migrated
/// credential.
fn classic_digest(salt: &str, password: &str) -> String {
    let inner = hex::encode(Sha1::digest(password.as_bytes()));
    hex::encode(Sha1::digest(format!("{salt}{inner}").as_bytes()))
}

/// Importing a document creates a graph that owns its triples, projects the
/// derived object view, and deleting the graph removes its triples.
pub async fn import_and_read_back(store: &dyn SbolStore) {
    let report = store
        .import_document(ImportInput {
            body: SIMPLE_COMPONENT_TTL.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("conformance://simple_component".to_owned()),
            document_iri: None,
            created_by: None,
            name: Some("conformance import".to_owned()),
            description: None,
            overwrite: sbol_db_storage::ImportOverwrite::Fail,
        })
        .await
        .expect("import_document");

    assert_eq!(
        report.object_count, 2,
        "component + sequence project to 2 objects"
    );
    assert!(report.triple_count > 0, "document has triples");

    // The graph exists and owns exactly this import's objects.
    assert!(
        store
            .get_graph(report.graph_id)
            .await
            .expect("get_graph")
            .is_some(),
        "imported graph is registered"
    );
    let objects = store
        .list_objects(&ListObjectsFilter {
            sbol_class: None,
            role: None,
            graph_id: Some(report.graph_id),
            after_iri: None,
            limit: 100,
        })
        .await
        .expect("list_objects");
    assert_eq!(
        objects.len(),
        2,
        "both objects are listable, scoped to the graph"
    );

    // Each listed object round-trips by IRI and has stored triples.
    let iri = objects[0].iri.as_str().to_owned();
    assert!(
        store
            .get_object_by_iri(&iri)
            .await
            .expect("get_object_by_iri")
            .is_some(),
        "object resolves by IRI"
    );
    assert!(
        !store
            .triples_for_subject(&iri)
            .await
            .expect("triples_for_subject")
            .is_empty(),
        "object has triples"
    );

    // Deleting the graph cascades its triples away.
    assert!(store
        .delete_graph(report.graph_id)
        .await
        .expect("delete_graph"));
    assert!(
        store
            .get_graph(report.graph_id)
            .await
            .expect("get_graph")
            .is_none(),
        "graph is gone after delete"
    );
    assert!(
        store
            .triples_for_subject(&iri)
            .await
            .expect("triples_for_subject")
            .is_empty(),
        "the graph's triples are gone after delete"
    );
}

/// A graph is a set of triples: re-writing an already-present triple is a
/// no-op, and clearing the graph removes its contents.
pub async fn graph_set_semantics(store: &dyn SbolStore) {
    const GRAPH: &str = "urn:sbol-db:conformance:set-semantics";
    let body = "<urn:s:a> <urn:p:rel> <urn:o:b> .\n<urn:s:a> <urn:p:rel> <urn:o:c> .\n";

    let first = store
        .graph_store_write(
            GRAPH,
            body,
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("first write");
    assert_eq!(first, 2, "two distinct triples inserted");

    let second = store
        .graph_store_write(
            GRAPH,
            body,
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("second write");
    assert_eq!(
        second, 0,
        "re-writing the same triples is a no-op (set semantics)"
    );

    let triples = store
        .graph_store_read(GRAPH)
        .await
        .expect("graph_store_read");
    assert_eq!(
        triples.len(),
        2,
        "the graph holds exactly the two distinct triples"
    );

    let cleared = store
        .graph_store_clear(GRAPH)
        .await
        .expect("graph_store_clear");
    assert_eq!(cleared, 2, "clearing removes both triples");
    assert!(
        store
            .graph_store_read(GRAPH)
            .await
            .expect("graph_store_read")
            .is_empty(),
        "graph is empty after clear"
    );
}

/// A forward neighborhood walk from a component reaches the sequence it
/// references, with the connecting edge.
pub async fn neighborhood_walk(store: &dyn SbolStore) {
    let report = store
        .import_document(ImportInput {
            body: SIMPLE_COMPONENT_TTL.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("conformance://neighborhood".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
            overwrite: sbol_db_storage::ImportOverwrite::Fail,
        })
        .await
        .expect("import_document");

    let objects = store
        .list_objects(&ListObjectsFilter {
            sbol_class: None,
            role: None,
            graph_id: Some(report.graph_id),
            after_iri: None,
            limit: 100,
        })
        .await
        .expect("list_objects");
    let component = objects
        .iter()
        .find(|o| o.sbol_class.ends_with("#Component"))
        .expect("a component object");
    let sequence = objects
        .iter()
        .find(|o| o.sbol_class.ends_with("#Sequence"))
        .expect("a sequence object");

    let result = store
        .walk(&NeighborhoodQuery {
            root_iri: IriString::unchecked(component.iri.as_str()),
            depth: 1,
            direction: Direction::Forward,
            predicate_allowlist: Vec::new(),
            max_nodes: Some(100),
            include_literals: false,
        })
        .await
        .expect("walk");

    assert!(
        result.nodes.iter().any(|n| n.id == component.iri.as_str()),
        "the root component is a node in the walk"
    );
    assert!(
        result.nodes.iter().any(|n| n.id == sequence.iri.as_str()),
        "the referenced sequence is reached at depth 1"
    );
    assert!(
        result
            .edges
            .iter()
            .any(|e| e.subject == component.iri.as_str()),
        "there is an edge out of the component"
    );

    store
        .delete_graph(report.graph_id)
        .await
        .expect("delete_graph");
}

/// Nucleotide search finds a present motif (seeded by k-mer), is
/// reverse-complement aware, and reports nothing for an absent motif.
pub async fn sequence_search(store: &dyn SbolStore) {
    let report = store
        .import_document(ImportInput {
            body: SIMPLE_COMPONENT_TTL.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some("conformance://sequence".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
            overwrite: sbol_db_storage::ImportOverwrite::Fail,
        })
        .await
        .expect("import_document");

    let objects = store
        .list_objects(&ListObjectsFilter {
            sbol_class: None,
            role: None,
            graph_id: Some(report.graph_id),
            after_iri: None,
            limit: 100,
        })
        .await
        .expect("list_objects");
    let sequence_iri = objects
        .iter()
        .find(|o| o.sbol_class.ends_with("#Sequence"))
        .expect("a sequence object")
        .iri
        .as_str()
        .to_owned();

    let opts = || SequenceSearchOptions {
        max_hits: Some(100),
        forward_only: None,
    };

    // A motif present in the sequence's elements (k-mer seeded path).
    let motif = "GCTAGCTCAGTCC";
    let hits = store.search(motif, opts()).await.expect("search");
    assert!(
        hits.iter().any(|m| m.sequence_iri == sequence_iri),
        "k-mer-seeded forward search finds the sequence"
    );

    // The motif's reverse complement still matches (reverse strand).
    let rc = sbol_db_core::kmer::reverse_complement_string(motif);
    let rc_hits = store.search(&rc, opts()).await.expect("rc search");
    assert!(
        rc_hits.iter().any(|m| m.sequence_iri == sequence_iri),
        "reverse-complement search finds the sequence"
    );

    // A motif that does not occur returns nothing.
    let absent = store
        .search("AAAAAAAAAAAACCCCC", opts())
        .await
        .expect("search");
    assert!(absent.is_empty(), "an absent motif returns no matches");

    store
        .delete_graph(report.graph_id)
        .await
        .expect("delete_graph");
}

/// A 40 bp nucleotide sequence with varied composition, the centroid the
/// clustering scenarios build their near-identical members around.
const CLUSTER_BASE: &str = "ACGTACGTACGTTTGGCCAAGGTTCCAAGGATCGATCGAT";
/// A 40 bp sequence sharing no k-mer with [`CLUSTER_BASE`], so it never clusters
/// with the near-identical set.
const CLUSTER_UNRELATED: &str = "TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT";
/// The floating-point tolerance for identity comparisons: a native aligner's
/// `iddef=2` identity is equivalent to vsearch's but not bit-identical, so scores
/// are asserted within an epsilon rather than by string equality.
const ALIGN_EPS: f64 = 1e-9;

/// Flip the base at `at` to a different nucleotide, yielding a single-mismatch
/// variant that stays within the clustering identity threshold.
fn point_mutant(seq: &str, at: usize) -> String {
    let mut chars: Vec<char> = seq.chars().collect();
    chars[at] = if chars[at] == 'A' { 'C' } else { 'A' };
    chars.into_iter().collect()
}

/// The identity a CIGAR implies: `M` columns over all columns. `M` counts as an
/// aligned (match-or-mismatch) column and `I`/`D` as indel columns, so a gap-free
/// all-`M` alignment implies full identity. The known divergence from vsearch is
/// asserted through this implied identity rather than by CIGAR string equality.
fn cigar_implied_identity(cigar: &str) -> f64 {
    let mut run = 0usize;
    let mut aligned = 0usize;
    let mut total = 0usize;
    for ch in cigar.chars() {
        if let Some(d) = ch.to_digit(10) {
            run = run * 10 + d as usize;
        } else {
            if ch == 'M' {
                aligned += run;
            }
            total += run;
            run = 0;
        }
    }
    if total == 0 {
        0.0
    } else {
        aligned as f64 / total as f64
    }
}

/// The Sequence IRI a fresh import of [`SIMPLE_COMPONENT_TTL`] projects, seeded
/// into `app`'s store under `source_uri`.
async fn import_simple_component(app: &AppServices, source_uri: &str) -> (GraphId, String) {
    let report = app
        .store
        .import_document(ImportInput {
            body: SIMPLE_COMPONENT_TTL.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: Some(source_uri.to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("import_document");
    let objects = app
        .store
        .list_objects(&ListObjectsFilter {
            sbol_class: None,
            role: None,
            graph_id: Some(report.graph_id),
            after_iri: None,
            limit: 100,
        })
        .await
        .expect("list_objects");
    let sequence_iri = objects
        .iter()
        .find(|o| o.sbol_class.ends_with("#Sequence"))
        .expect("a sequence object")
        .iri
        .as_str()
        .to_owned();
    (report.graph_id, sequence_iri)
}

/// The banded aligner recovers a seeded sequence from a query identical to its
/// residues: the k-mer prefilter admits it, the aligner scores full identity on
/// the forward strand, and its CIGAR implies that same identity. The hit set is
/// exactly the one seeded sequence.
///
/// Alignment runs through the facade's [`SequenceService`](sbol_db_app::SequenceService),
/// which gathers candidates from the backend's k-mer index and verifies each with
/// the native aligner, so this certifies the whole sequence-search path on each
/// backend. The identity is asserted within [`ALIGN_EPS`] and the CIGAR by its
/// implied identity, never by string equality with vsearch.
pub async fn sequence_align(app: &AppServices) {
    let (graph_id, sequence_iri) =
        import_simple_component(app, "conformance://sequence-align").await;

    // The query is the seeded Sequence's own residues, uppercased.
    let query = "TTGACAGCTAGCTCAGTCCTAGGTATAATGCTAGC";
    let hits = app
        .sequence()
        .align(query, AlignOptions::default(), &GraphScope::Union)
        .await
        .expect("align");

    let seen: std::collections::HashSet<&str> =
        hits.iter().map(|h| h.sequence_iri.as_str()).collect();
    assert_eq!(
        seen,
        std::collections::HashSet::from([sequence_iri.as_str()]),
        "the hit set is exactly the seeded sequence: {seen:?}"
    );

    let hit = hits
        .iter()
        .find(|h| h.sequence_iri == sequence_iri)
        .expect("the seeded sequence is a hit");
    assert!(
        (hit.percent_match - 1.0).abs() < ALIGN_EPS,
        "an identical query is full identity within epsilon: {}",
        hit.percent_match
    );
    assert_eq!(
        hit.strand, '+',
        "the forward query aligns on the plus strand"
    );
    let implied = cigar_implied_identity(&hit.cigar);
    assert!(
        (implied - hit.percent_match).abs() < ALIGN_EPS,
        "the CIGAR's implied identity matches percentMatch: cigar={} implied={implied} percent_match={}",
        hit.cigar,
        hit.percent_match
    );

    app.store
        .delete_graph(graph_id)
        .await
        .expect("delete_graph");
}

/// `/similar` returns exactly a target's cluster mates, ordered by PageRank, and
/// never the target itself or an unrelated sequence.
///
/// A cluster of near-identical sequences plus one unrelated sequence is grouped
/// by [`cluster_sequences`] and persisted through the facade's `ClusterStore`;
/// with per-mate PageRank scores, [`SequenceService::similar`](sbol_db_app::SequenceService::similar)
/// returns the other cluster members in descending PageRank order, carrying no
/// alignment columns, and `similarCount` counts exactly those mates. The
/// unrelated sequence clusters alone, so it is never a mate.
pub async fn similar_sequences(app: &AppServices) {
    const TARGET: &str = "urn:sbol-db:conformance:similar:target";
    const MATE_LOW: &str = "urn:sbol-db:conformance:similar:mate_low";
    const MATE_HIGH: &str = "urn:sbol-db:conformance:similar:mate_high";
    const UNRELATED: &str = "urn:sbol-db:conformance:similar:unrelated";

    let assignments = cluster_sequences(
        vec![
            (TARGET.to_owned(), CLUSTER_BASE.to_owned()),
            (MATE_LOW.to_owned(), point_mutant(CLUSTER_BASE, 12)),
            (MATE_HIGH.to_owned(), point_mutant(CLUSTER_BASE, 27)),
            (UNRELATED.to_owned(), CLUSTER_UNRELATED.to_owned()),
        ],
        &AlignOptions::default(),
    );
    // The near-identical set shares the target's cluster; the unrelated one does
    // not, so it can never surface as a mate.
    let by_iri: std::collections::HashMap<String, ClusterId> =
        assignments.iter().cloned().collect();
    assert_eq!(
        by_iri[MATE_LOW], by_iri[TARGET],
        "the low-rank mate clusters with the target"
    );
    assert_eq!(
        by_iri[MATE_HIGH], by_iri[TARGET],
        "the high-rank mate clusters with the target"
    );
    assert_ne!(
        by_iri[UNRELATED], by_iri[TARGET],
        "the unrelated sequence clusters apart from the target"
    );

    app.cluster
        .replace_clusters(assignments)
        .await
        .expect("replace_clusters");
    app.pagerank
        .replace_all_ranks(vec![
            RankRow {
                iri: MATE_LOW.to_owned(),
                score: 1.0,
            },
            RankRow {
                iri: MATE_HIGH.to_owned(),
                score: 5.0,
            },
        ])
        .await
        .expect("replace_all_ranks");

    let hits = app
        .sequence()
        .similar(TARGET, &GraphScope::Union)
        .await
        .expect("similar");
    let order: Vec<&str> = hits.iter().map(|h| h.iri.as_str()).collect();
    assert_eq!(
        order,
        vec![MATE_HIGH, MATE_LOW],
        "the mates are the other cluster members, ranked by PageRank descending, never the target"
    );
    assert!(
        !order.contains(&UNRELATED),
        "the unrelated sequence is never a mate"
    );
    let count = app
        .sequence()
        .similar_count(TARGET, &GraphScope::Union)
        .await
        .expect("similar_count");
    assert_eq!(count, 2, "two cluster mates, excluding the target");

    // Leave the in-memory cluster and rank stores empty for later scenarios.
    app.cluster
        .replace_clusters(Vec::new())
        .await
        .expect("clear clusters");
    app.pagerank
        .replace_all_ranks(Vec::new())
        .await
        .expect("clear ranks");
}

/// Greedy centroid clustering groups a near-identical set into one cluster while
/// an unrelated sequence stands alone, reproducing `vsearch --cluster_fast --id
/// 0.8`. The algorithm is pure, so this certifies recall independent of any
/// backend.
pub fn cluster_recall() {
    let assignments = cluster_sequences(
        vec![
            ("urn:seq:recall:a".to_owned(), CLUSTER_BASE.to_owned()),
            (
                "urn:seq:recall:b".to_owned(),
                point_mutant(CLUSTER_BASE, 12),
            ),
            (
                "urn:seq:recall:c".to_owned(),
                point_mutant(CLUSTER_BASE, 27),
            ),
            (
                "urn:seq:recall:unrelated".to_owned(),
                CLUSTER_UNRELATED.to_owned(),
            ),
        ],
        &AlignOptions::default(),
    );
    let by_iri: std::collections::HashMap<String, ClusterId> = assignments.into_iter().collect();

    let cluster = by_iri["urn:seq:recall:a"];
    assert_eq!(
        by_iri["urn:seq:recall:b"], cluster,
        "the first near-identical member joins the centroid's cluster"
    );
    assert_eq!(
        by_iri["urn:seq:recall:c"], cluster,
        "the second near-identical member joins the centroid's cluster"
    );
    assert_ne!(
        by_iri["urn:seq:recall:unrelated"], cluster,
        "the unrelated sequence opens its own cluster"
    );
    let distinct: std::collections::HashSet<ClusterId> = by_iri.values().copied().collect();
    assert_eq!(
        distinct.len(),
        2,
        "the near-identical set and the unrelated sequence form exactly two clusters"
    );
}

/// With clustering persisted, the `/2` duplicate penalty bites in the served
/// ranked-text path: a non-centroid duplicate is demoted below its centroid, and
/// its score halves against the same search with no persisted clusters.
///
/// A centroid and a duplicate carry identical text, the centroid holding the
/// higher PageRank so it ranks first. [`AppServices::ranked_search`] with the
/// cluster store empty leaves the duplicate's score whole; persisting the two in
/// one cluster through [`ClusterStore::replace_clusters`] halves the duplicate on
/// the next served search, dropping it below the centroid. The served path reads
/// the assignments the store holds, so this fails if `ranked_search` ignores
/// persisted clusters.
pub async fn ranked_dup_penalty_active(app: &AppServices) {
    let centroid = conformance_iri("dup_centroid");
    let duplicate = conformance_iri("dup_member");
    app.text_search
        .rebuild(vec![
            indexed_part_ranked(&centroid, "dupwidget", "same text", COMPONENT_TYPE, 5.0),
            indexed_part_ranked(&duplicate, "dupwidget", "same text", COMPONENT_TYPE, 1.0),
        ])
        .expect("rebuild index for the duplicate-penalty rule");

    // With the cluster store empty the served search leaves the duplicate whole.
    app.cluster
        .replace_clusters(Vec::new())
        .await
        .expect("clear clusters");
    let baseline = ranked(app, "dupwidget").await;
    let baseline_dup = baseline
        .iter()
        .find(|h| h.subject == duplicate)
        .expect("the duplicate is present without penalty")
        .score;

    // Persisting the centroid and duplicate in one cluster activates the penalty
    // on the next served search.
    app.cluster
        .replace_clusters(vec![
            (centroid.clone(), ClusterId(0)),
            (duplicate.clone(), ClusterId(0)),
        ])
        .await
        .expect("persist the cluster assignments");
    let penalized = ranked(app, "dupwidget").await;
    let centroid_score = penalized
        .iter()
        .find(|h| h.subject == centroid)
        .expect("the centroid is present")
        .score;
    let dup_score = penalized
        .iter()
        .find(|h| h.subject == duplicate)
        .expect("the duplicate is present under penalty")
        .score;

    assert!(
        dup_score < centroid_score,
        "the duplicate is demoted below its centroid: duplicate={dup_score} centroid={centroid_score}"
    );
    assert!(
        (baseline_dup / dup_score - 2.0).abs() < 1e-6,
        "the served path halves the duplicate once its cluster is persisted: baseline={baseline_dup} penalized={dup_score}"
    );
    assert_eq!(
        penalized.last().map(|h| h.subject.as_str()),
        Some(duplicate.as_str()),
        "the penalized duplicate ranks below the centroid"
    );

    // Leave the cluster store empty for the scenarios that follow.
    app.cluster
        .replace_clusters(Vec::new())
        .await
        .expect("clear clusters");
}

/// Loading an ontology builds its transitive closure: descendants of an
/// ancestor include deeper subtypes, and terms resolve by canonical IRI.
pub async fn ontology_roundtrip(store: &dyn SbolStore) {
    let report = store
        .load_ontology_from_text("SO", "Sequence Ontology (conformance)", None, TINY_SO_OBO)
        .await
        .expect("load_ontology_from_text");
    assert_eq!(report.term_count, 4, "four terms loaded");

    assert!(
        !store
            .list_ontologies()
            .await
            .expect("list_ontologies")
            .is_empty(),
        "the loaded ontology is listed"
    );

    let descendants = store.descendants(SO_REGION_IRI).await.expect("descendants");
    assert!(
        descendants
            .iter()
            .any(|(iri, _depth)| iri == SO_PROMOTER_IRI),
        "promoter is a transitive descendant of region"
    );

    assert!(
        store
            .get_ontology_term(SO_PROMOTER_IRI)
            .await
            .expect("get_ontology_term")
            .is_some(),
        "the promoter term resolves by canonical IRI"
    );
}

/// The job queue's full lifecycle: enqueue, lease-based dequeue, lease renewal,
/// terminal success, idempotent enqueue, empty dequeue, and cancellation.
pub async fn job_queue_lifecycle(jobs: &dyn JobQueue) {
    let worker = "conformance-worker";
    let lease = Duration::from_secs(60);
    let queues = vec![DEFAULT_QUEUE.to_owned()];

    // Enqueue a job and find it through the read surface.
    let enqueued = jobs
        .enqueue(new_job("conformance.success", None))
        .await
        .expect("enqueue");
    let job = match enqueued {
        EnqueueOutcome::Inserted(job) => job,
        EnqueueOutcome::AlreadyExists(_) => panic!("first enqueue should insert"),
    };
    assert!(
        jobs.get(job.id).await.expect("get").is_some(),
        "job is gettable"
    );
    assert!(
        jobs.list(&ListJobsFilter {
            kind: Some("conformance.success".to_owned()),
            status: None,
            queue: None,
            correlation_id: None,
            since: None,
            limit: 50,
        })
        .await
        .expect("list")
        .iter()
        .any(|j| j.id == job.id),
        "job appears in a filtered listing"
    );

    // Lease it, renew the lease, and complete it.
    let leased = jobs
        .dequeue(&queues, worker, lease)
        .await
        .expect("dequeue")
        .expect("a job is available to dequeue");
    assert_eq!(leased.id, job.id, "dequeue returns the enqueued job");
    assert!(
        jobs.renew_lease(job.id, worker, lease)
            .await
            .expect("renew_lease"),
        "the lease holder can renew"
    );
    jobs.mark_succeeded(job.id, worker, None)
        .await
        .expect("mark_succeeded");
    assert_eq!(
        jobs.current_status(job.id).await.expect("current_status"),
        Some(JobStatus::Succeeded),
        "the job is terminal-succeeded"
    );

    // Idempotency keys deduplicate enqueues.
    let key = Some("conformance-idem-key".to_owned());
    let first = jobs
        .enqueue(new_job("conformance.idem", key.clone()))
        .await
        .expect("enqueue idem");
    assert!(
        matches!(first, EnqueueOutcome::Inserted(_)),
        "first idem enqueue inserts"
    );
    let second = jobs
        .enqueue(new_job("conformance.idem", key))
        .await
        .expect("enqueue idem again");
    assert!(
        matches!(second, EnqueueOutcome::AlreadyExists(_)),
        "a repeated idempotency key deduplicates"
    );

    // Cancellation is observable.
    let to_cancel = match jobs
        .enqueue(new_job("conformance.cancel", None))
        .await
        .expect("enqueue cancel")
    {
        EnqueueOutcome::Inserted(job) => job,
        EnqueueOutcome::AlreadyExists(_) => panic!("unexpected dedup"),
    };
    assert!(
        jobs.cancel(to_cancel.id).await.expect("cancel"),
        "cancel reports success"
    );
    assert_eq!(
        jobs.current_status(to_cancel.id)
            .await
            .expect("current_status"),
        Some(JobStatus::Cancelled),
        "the job is cancelled"
    );
}

/// ACL-scoped reads hide another user's private graph.
///
/// Two users each own a private graph carrying a private object, alongside a
/// public graph readable by everyone. The [`GraphScope`] the facade's
/// `AclService` computes for a user authorizes that user's own graph and the
/// public graph and nothing else; a SPARQL read run under it returns the
/// user's own and the public object but never the other user's private object.
///
/// This is the security-critical contract of the read path: visibility is the
/// server-computed scope, never a graph the client names.
pub async fn acl_scope_hides_private_graph(app: &AppServices) {
    const USER_A: &str = "http://synbiohub.org/user/alice";
    const USER_B: &str = "http://synbiohub.org/user/bob";
    const GRAPH_A: &str = "http://synbiohub.org/user/alice/private_A";
    const GRAPH_B: &str = "http://synbiohub.org/user/bob/private_B";
    const OBJ_A: &str = "http://synbiohub.org/user/alice/private_A/objA";
    const OBJ_B: &str = "http://synbiohub.org/user/bob/private_B/objB";
    const OBJ_PUBLIC: &str = "http://synbiohub.org/public/objPub";
    const DISPLAY_ID: &str = "http://sbols.org/v3#displayId";

    // Each private graph carries a real object plus the `sbh:ownedBy` fact the
    // AclService reads; the public graph carries a public object.
    let graph_a =
        format!("<{OBJ_A}> <{DISPLAY_ID}> \"objA\" .\n<{OBJ_A}> <{SBH_OWNED_BY}> <{USER_A}> .\n");
    let graph_b =
        format!("<{OBJ_B}> <{DISPLAY_ID}> \"objB\" .\n<{OBJ_B}> <{SBH_OWNED_BY}> <{USER_B}> .\n");
    let graph_public = format!("<{OBJ_PUBLIC}> <{DISPLAY_ID}> \"objPub\" .\n");

    for (graph, body) in [
        (GRAPH_A, &graph_a),
        (GRAPH_B, &graph_b),
        (PUBLIC_GRAPH, &graph_public),
    ] {
        app.store
            .graph_store_write(
                graph,
                body,
                SerializationFormat::NTriples,
                GraphWriteMode::Merge,
            )
            .await
            .expect("seed graph");
    }

    let scope_a = app
        .acl_service
        .compute_scope(Some(USER_A))
        .await
        .expect("compute scope for A");
    let scope_b = app
        .acl_service
        .compute_scope(Some(USER_B))
        .await
        .expect("compute scope for B");

    // Each computed scope names the public graph and the user's own graph, and
    // excludes the other user's private graph.
    assert!(
        scope_names(&scope_a, PUBLIC_GRAPH),
        "A's scope includes the public graph"
    );
    assert!(
        scope_names(&scope_a, GRAPH_A),
        "A's scope includes A's own graph"
    );
    assert!(
        !scope_names(&scope_a, GRAPH_B),
        "A's scope excludes B's private graph"
    );
    assert!(
        scope_names(&scope_b, PUBLIC_GRAPH),
        "B's scope includes the public graph"
    );
    assert!(
        scope_names(&scope_b, GRAPH_B),
        "B's scope includes B's own graph"
    );
    assert!(
        !scope_names(&scope_b, GRAPH_A),
        "B's scope excludes A's private graph"
    );

    // A read under each scope returns exactly the authorized objects.
    let seen_by_a = subjects_under_scope(app, &scope_a).await;
    let seen_by_b = subjects_under_scope(app, &scope_b).await;

    // The public object is visible to both users.
    assert!(
        seen_by_a.contains(OBJ_PUBLIC),
        "A reads the public object under its scope"
    );
    assert!(
        seen_by_b.contains(OBJ_PUBLIC),
        "B reads the public object under its scope"
    );

    // Each user reads their own private object.
    assert!(
        seen_by_a.contains(OBJ_A),
        "A reads A's own private object under its scope"
    );
    assert!(
        seen_by_b.contains(OBJ_B),
        "B reads B's own private object under its scope"
    );

    // The security-critical assertion: a non-owner cannot read another user's
    // private object.
    assert!(
        !seen_by_a.contains(OBJ_B),
        "A cannot read B's private object under its scope"
    );
    assert!(
        !seen_by_b.contains(OBJ_A),
        "B cannot read A's private object under its scope"
    );

    // An anonymous caller is scoped to the public graph alone: it reads the
    // public object but neither user's private object.
    let scope_anon = app
        .acl_service
        .compute_scope(None)
        .await
        .expect("compute scope for anonymous");
    assert!(
        scope_names(&scope_anon, PUBLIC_GRAPH),
        "anonymous scope includes the public graph"
    );
    assert!(
        !scope_names(&scope_anon, GRAPH_A) && !scope_names(&scope_anon, GRAPH_B),
        "anonymous scope excludes every private graph"
    );
    let seen_by_anon = subjects_under_scope(app, &scope_anon).await;
    assert!(
        seen_by_anon.contains(OBJ_PUBLIC),
        "anonymous reads the public object"
    );
    assert!(
        !seen_by_anon.contains(OBJ_A) && !seen_by_anon.contains(OBJ_B),
        "anonymous cannot read any private object"
    );

    // Leave the store as we found it for any scenario that follows.
    for graph in [GRAPH_A, GRAPH_B, PUBLIC_GRAPH] {
        app.store
            .graph_store_clear(graph)
            .await
            .expect("clear seed graph");
    }
}

/// The SBOL2 rdf:type IRIs a ranked object carries: a plain part versus the
/// Sequence type the ranker divides by 10.
const COMPONENT_TYPE: &str = "http://sbols.org/v2#ComponentDefinition";
const SEQUENCE_TYPE: &str = "http://sbols.org/v2#Sequence";

/// Native ranked text search reproduces SBOLExplorer's three ranking rules over
/// the facade's shared index: a displayId-exact hit outranks a description-only
/// one, a Sequence-typed hit is demoted, and PageRank (derived from the
/// reference graph) breaks a text-score tie.
///
/// Each rule is exercised on its own freshly rebuilt corpus, then queried
/// through [`AppServices::ranked_search`], the same facade verb the `/search`
/// adapter calls, under an unrestricted scope so only ranking is under test.
pub async fn ranked_text_search(app: &AppServices) {
    // A displayId-exact match ranks above an object that merely mentions the
    // term in its description.
    app.text_search
        .rebuild(vec![
            indexed_part(
                "rank_exact",
                "promoter",
                "a generic widget part",
                COMPONENT_TYPE,
                1.0,
            ),
            indexed_part(
                "rank_desc",
                "widget",
                "a strong promoter element",
                COMPONENT_TYPE,
                1.0,
            ),
        ])
        .expect("rebuild index for the displayId-exact rule");
    let hits = ranked(app, "promoter").await;
    assert_eq!(
        hits.first().map(|h| h.subject.as_str()),
        Some(conformance_iri("rank_exact").as_str()),
        "a displayId-exact hit outranks a description-only hit"
    );

    // A Sequence-typed hit is divided by 10, so an equally matching
    // ComponentDefinition ranks above it.
    app.text_search
        .rebuild(vec![
            indexed_part("rank_seq", "promoter", "same text", SEQUENCE_TYPE, 1.0),
            indexed_part("rank_cd", "promoter", "same text", COMPONENT_TYPE, 1.0),
        ])
        .expect("rebuild index for the Sequence-penalty rule");
    let hits = ranked(app, "promoter").await;
    assert_eq!(
        hits.first().map(|h| h.subject.as_str()),
        Some(conformance_iri("rank_cd").as_str()),
        "a Sequence-typed hit is demoted below an equally matching non-Sequence hit"
    );

    // With identical text, the object the reference graph endorses (higher
    // PageRank) wins the tie. Two leaves reference the hub, so the hub outranks
    // the leaf exactly as SBOLExplorer's link graph would score them.
    let hub = conformance_iri("rank_hub");
    let leaf = conformance_iri("rank_leaf");
    let feeder = conformance_iri("rank_feeder");
    let uris = vec![hub.clone(), leaf.clone(), feeder.clone()];
    let edges = vec![(leaf.clone(), hub.clone()), (feeder.clone(), hub.clone())];
    let ranks = pagerank(&edges, &uris);
    assert!(
        ranks[&hub] > ranks[&leaf],
        "the reference graph must give the hub a higher PageRank"
    );
    app.text_search
        .rebuild(vec![
            indexed_part_ranked(&hub, "widget", "same text", COMPONENT_TYPE, ranks[&hub]),
            indexed_part_ranked(&leaf, "widget", "same text", COMPONENT_TYPE, ranks[&leaf]),
        ])
        .expect("rebuild index for the PageRank tie-break rule");
    let hits = ranked(app, "widget").await;
    assert_eq!(
        hits.first().map(|h| h.subject.as_str()),
        Some(hub.as_str()),
        "on a text tie the higher-PageRank object wins"
    );
}

/// ACL-scoped ranked search hides another user's private object while surfacing
/// the owner's own and the public one, enforcing the caller's scope inside the
/// index exactly as the SPARQL read path enforces it.
///
/// The owner's private graph carries an `sbh:ownedBy` fact so the facade's
/// `AclService` admits it to the owner's scope; the index holds the owner's
/// private object, a public object, and a non-owner's private object. Searching
/// under each computed scope proves visibility follows the server-computed
/// scope, never the index's full corpus.
pub async fn acl_scoped_search(app: &AppServices) {
    const OWNER: &str = "http://synbiohub.org/user/search_owner";
    const OWNER_GRAPH: &str = "http://synbiohub.org/user/search_owner/private";
    const OTHER: &str = "http://synbiohub.org/user/search_other";
    const OWNER_OBJ: &str = "http://synbiohub.org/user/search_owner/private/ownerWidget";
    const PUBLIC_OBJ: &str = "http://synbiohub.org/public/publicWidget";
    const OTHER_OBJ: &str = "http://synbiohub.org/user/search_other/private/otherWidget";

    // The owner's private graph records the ownership fact the AclService reads.
    let owned = format!("<{OWNER_OBJ}> <{SBH_OWNED_BY}> <{OWNER}> .\n");
    app.store
        .graph_store_write(
            OWNER_GRAPH,
            &owned,
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("seed the owner's private graph");

    // All three objects share a distinctive term so a single query matches each;
    // graph placement, not text, decides visibility.
    app.text_search
        .rebuild(vec![
            indexed_part_in(OWNER_OBJ, OWNER_GRAPH, "scopedwidget", COMPONENT_TYPE),
            indexed_part_in(PUBLIC_OBJ, PUBLIC_GRAPH, "scopedwidget", COMPONENT_TYPE),
            indexed_part_in(
                OTHER_OBJ,
                "http://synbiohub.org/user/search_other/private",
                "scopedwidget",
                COMPONENT_TYPE,
            ),
        ])
        .expect("rebuild index for the ACL-scoped search");

    let owner_scope = app
        .acl_service
        .compute_scope(Some(OWNER))
        .await
        .expect("compute the owner's scope");
    let other_scope = app
        .acl_service
        .compute_scope(Some(OTHER))
        .await
        .expect("compute the non-owner's scope");

    let seen_by_owner = ranked_subjects(app, "scopedwidget", owner_scope).await;
    let seen_by_other = ranked_subjects(app, "scopedwidget", other_scope).await;

    assert!(
        seen_by_owner.contains(&OWNER_OBJ.to_owned()),
        "the owner sees their own private object"
    );
    assert!(
        seen_by_owner.contains(&PUBLIC_OBJ.to_owned()),
        "the owner sees the public object"
    );
    assert!(
        seen_by_other.contains(&PUBLIC_OBJ.to_owned()),
        "a non-owner sees the public object"
    );
    // The security-critical assertion: the non-owner's scope hides the owner's
    // private object.
    assert!(
        !seen_by_other.contains(&OWNER_OBJ.to_owned()),
        "a non-owner never surfaces another user's private object"
    );

    app.store
        .graph_store_clear(OWNER_GRAPH)
        .await
        .expect("clear the owner's private graph");
}

/// The recursive object closure follows transitive references to a fixpoint,
/// while the non-recursive closure narrows to the object's own URI prefix and
/// drops `sbol2:member` edges.
///
/// A ComponentDefinition `root` references an annotation that references a
/// transitively-reachable `subcomp`, and also `member`s a `sibling` reachable
/// only through that membership edge. The recursive crawl reaches both the deep
/// child and the sibling; the non-recursive crawl reaches the deep child (under
/// the object's prefix) but never the member-only sibling. The crawl runs over
/// the backend's SPARQL engine, so this certifies the download closure loop end
/// to end on each backend.
pub async fn download_closure_recursion(app: &AppServices) {
    const GRAPH: &str = "urn:sbol-db:conformance:download-closure";
    const PREFIX: &str = "http://example.org/dl/";
    const ROOT: &str = "http://example.org/dl/root";
    const SUBCOMP: &str = "http://example.org/dl/subcomp";
    const SIBLING: &str = "http://example.org/dl/sibling";
    const TITLE: &str = "http://purl.org/dc/terms/title";
    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";

    let body = format!(
        "<{ROOT}> <{RDF_TYPE}> <http://sbols.org/v2#ComponentDefinition> .\n\
         <{ROOT}> <http://sbols.org/v2#sequenceAnnotation> <http://example.org/dl/root/anno> .\n\
         <{ROOT}> <{SBOL2_MEMBER}> <{SIBLING}> .\n\
         <http://example.org/dl/root/anno> <{RDF_TYPE}> <http://sbols.org/v2#SequenceAnnotation> .\n\
         <http://example.org/dl/root/anno> <http://sbols.org/v2#component> <{SUBCOMP}> .\n\
         <{SUBCOMP}> <{RDF_TYPE}> <http://sbols.org/v2#Component> .\n\
         <{SUBCOMP}> <{TITLE}> \"deep child\" .\n\
         <{SIBLING}> <{RDF_TYPE}> <http://sbols.org/v2#ComponentDefinition> .\n\
         <{SIBLING}> <{TITLE}> \"sibling only via member\" .\n"
    );
    app.store
        .graph_store_write(
            GRAPH,
            &body,
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("seed the download-closure fixture");

    let downloader = Downloader::new(app.sparql.clone()).with_database_prefix(PREFIX);

    // The recursive closure follows the two-hop reference to the deep child and
    // the membership edge to the sibling.
    let recursive = downloader
        .fetch_recursive(ROOT, GraphScope::Union)
        .await
        .expect("recursive closure");
    assert!(
        closure_has_literal(&recursive, SUBCOMP, TITLE, "deep child"),
        "recursive closure includes the transitively-referenced child"
    );
    assert!(
        closure_has_literal(&recursive, SIBLING, TITLE, "sibling only via member"),
        "recursive closure follows the member edge to the sibling"
    );

    // The non-recursive closure drops the member edge, so the member-only
    // sibling is never reached, while the deep child under the object's own
    // prefix is still included.
    let non_recursive = downloader
        .fetch_non_recursive(ROOT, GraphScope::Union)
        .await
        .expect("non-recursive closure");
    assert!(
        closure_has_literal(&non_recursive, SUBCOMP, TITLE, "deep child"),
        "non-recursive closure still includes children under the object's prefix"
    );
    assert!(
        !closure_has_literal(&non_recursive, SIBLING, TITLE, "sibling only via member"),
        "non-recursive closure excludes a sibling reachable only via member"
    );
    assert!(
        !non_recursive
            .iter()
            .any(|t| t.predicate.as_str() == SBOL2_MEMBER),
        "non-recursive closure carries no member edges"
    );

    app.store
        .graph_store_clear(GRAPH)
        .await
        .expect("clear the download-closure fixture");
}

/// A seeded object crawled to its closure serializes to every download format,
/// and each format carries the object's content back: GenBank and FASTA
/// re-import to the same residues, GFF3 projects the feature at its range, and
/// the OMEX archive holds `manifest.xml` plus an `sbol.rdf` that re-parses as an
/// SBOL document.
///
/// The fixture is one `Component` carrying a resolvable `Sequence` and a
/// promoter `SequenceFeature` at 10..40. It is written to the store as
/// N-Triples, crawled through the backend's SPARQL engine, then serialized, so
/// this certifies the whole download path (closure crawl plus every serializer)
/// on each backend.
pub async fn download_formats_roundtrip(app: &AppServices) {
    use sbol::v3::constants::{EDAM_IUPAC_DNA, ORIENTATION_INLINE, SBO_DNA, SO_PROMOTER};
    use sbol::v3::{Component, Document, Range, SbolObject, Sequence, SequenceFeature};

    const NS: &str = "https://example.org/sbol-db/conformance/download";
    const GRAPH: &str = "urn:sbol-db:conformance:download-formats";
    /// 60 bases, long enough to carry the promoter range's 10..40 bounds.
    const ELEMENTS: &str = "atgcatgcatgcatgcatgcatgcatgcatgcatgcatgcatgcatgcatgcatgcatgc";

    let sequence = Sequence::builder(NS, "cassette_sequence")
        .expect("sequence builder")
        .elements(ELEMENTS)
        .encoding(EDAM_IUPAC_DNA)
        .build()
        .expect("build sequence");

    // The range and feature are minted under the component's identity.
    let parent = Component::builder(NS, "cassette")
        .expect("component seed")
        .types([SBO_DNA])
        .build()
        .expect("build seed")
        .identity
        .clone();
    let prom_range = Range::builder(&parent, "prom_range")
        .expect("range builder")
        .start(10)
        .end(40)
        .orientation(ORIENTATION_INLINE)
        .sequence(sequence.identity.clone())
        .build()
        .expect("build promoter range");
    let promoter = SequenceFeature::builder(&parent, "prom")
        .expect("feature builder")
        .roles([SO_PROMOTER])
        .name("Promoter")
        .add_location(prom_range.identity.clone())
        .build()
        .expect("build promoter");
    let component = Component::builder(NS, "cassette")
        .expect("component builder")
        .types([SBO_DNA])
        .name("Test cassette")
        .add_sequence(sequence.identity.clone())
        .add_feature(promoter.identity.clone())
        .build()
        .expect("build component");

    let root_iri = component
        .identity
        .as_iri()
        .expect("the component has an IRI identity")
        .as_str()
        .to_owned();
    let document = Document::from_objects(vec![
        SbolObject::Component(component),
        SbolObject::Sequence(sequence),
        SbolObject::SequenceFeature(promoter),
        SbolObject::Range(prom_range),
    ])
    .expect("assemble the fixture document");
    let ntriples = document
        .write(sbol::RdfFormat::NTriples)
        .expect("write the fixture as N-Triples");

    app.store
        .graph_store_write(
            GRAPH,
            &ntriples,
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("seed the download-formats fixture");

    let downloader = Downloader::new(app.sparql.clone()).with_database_prefix(NS);
    let closure = downloader
        .fetch_recursive(&root_iri, GraphScope::Union)
        .await
        .expect("crawl the object closure");
    assert!(!closure.is_empty(), "the crawl reaches the seeded object");

    // FASTA re-imports to the same residues.
    let fasta = serialize_closure(&closure, SerializationFormat::Fasta, false).expect("fasta");
    let fasta_text = String::from_utf8(fasta.bytes).expect("utf8 fasta");
    let (fasta_doc, _) = sbol_fasta::FastaImporter::new(NS)
        .expect("fasta importer")
        .read_str(&fasta_text)
        .expect("re-import the fasta");
    assert_eq!(
        fasta_doc
            .sequences()
            .next()
            .and_then(|s| s.elements.as_deref())
            .map(str::to_ascii_lowercase),
        Some(ELEMENTS.to_ascii_lowercase()),
        "fasta round-trips the residues"
    );

    // GenBank re-imports to the same residues.
    let genbank =
        serialize_closure(&closure, SerializationFormat::GenBank, false).expect("genbank");
    let genbank_text = String::from_utf8(genbank.bytes).expect("utf8 genbank");
    let (genbank_doc, _) = sbol_genbank::GenbankImporter::new(NS)
        .expect("genbank importer")
        .read_str(&genbank_text)
        .expect("re-import the genbank");
    assert_eq!(
        genbank_doc
            .sequences()
            .next()
            .and_then(|s| s.elements.as_deref())
            .map(str::to_ascii_lowercase),
        Some(ELEMENTS.to_ascii_lowercase()),
        "genbank round-trips the residues"
    );

    // GFF3 opens with the version pragma and projects the promoter at its range.
    let gff3 = serialize_gff3(&closure).expect("gff3");
    let gff3_text = String::from_utf8(gff3.bytes).expect("utf8 gff3");
    assert!(
        gff3_text.starts_with("##gff-version 3\n"),
        "gff3 opens with the version pragma: {gff3_text}"
    );
    assert!(
        gff3_text.contains("cassette\t.\tpromoter\t10\t40\t.\t+\t0\tID=prom;Name=Promoter"),
        "gff3 projects the promoter feature at its range: {gff3_text}"
    );

    // OMEX is a zip carrying manifest.xml and an sbol.rdf that re-parses.
    let omex = serialize_omex(&closure, false, None).expect("omex");
    assert_eq!(omex.content_type, "application/zip");
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(omex.bytes)).expect("open the omex archive");
    let mut entries = Vec::new();
    for i in 0..archive.len() {
        entries.push(
            archive
                .by_index(i)
                .expect("archive entry")
                .name()
                .to_owned(),
        );
    }
    assert!(
        entries.iter().any(|n| n == "manifest.xml"),
        "the omex archive carries manifest.xml: {entries:?}"
    );
    assert!(
        entries.iter().any(|n| n == "sbol.rdf"),
        "the omex archive carries sbol.rdf: {entries:?}"
    );
    let mut sbol_rdf = String::new();
    archive
        .by_name("sbol.rdf")
        .expect("sbol.rdf entry")
        .read_to_string(&mut sbol_rdf)
        .expect("read sbol.rdf");
    let archived = Document::read(&sbol_rdf, sbol::RdfFormat::RdfXml).expect("parse sbol.rdf");
    assert!(
        archived.sequences().next().is_some(),
        "the archived sbol.rdf round-trips the sequence"
    );

    app.store
        .graph_store_clear(GRAPH)
        .await
        .expect("clear the download-formats fixture");
}

/// The content-addressed blob store round-trips bytes byte-for-byte and
/// de-duplicates identical content onto a single file.
///
/// Two `put`s of the same payload return an identical [`BlobRef`] and leave
/// exactly one `.gz` file in the content shard, so the classic SynBioHub
/// uploads layout stays content-addressed and a re-upload never doubles the
/// store. Exercised directly against the filesystem [`FsBlobStore`] because the
/// dedup claim is a filesystem property, not a trait-surface one.
pub async fn blob_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = FsBlobStore::new(dir.path());

    let payload = b"conformance blob payload";
    let blob = store.put(payload).await.expect("put");
    assert_eq!(
        blob.size,
        payload.len() as u64,
        "the ref records the uncompressed byte count"
    );
    assert_eq!(
        blob.sha1.len(),
        40,
        "the content address is a sha1 hex digest"
    );

    // The stored bytes read back verbatim through the trait surface.
    let got = store
        .get(&blob.sha1)
        .await
        .expect("get")
        .expect("the blob is present");
    assert_eq!(got, payload, "get returns the original bytes verbatim");

    // The blob lands at the classic uploads layout, sharded by the first two
    // hex characters of its content address.
    let shard = dir.path().join("uploads").join(&blob.sha1[0..2]);
    assert!(
        shard.join(format!("{}.gz", &blob.sha1[2..])).exists(),
        "the blob is stored at uploads/<sha1[0:2]>/<sha1[2:]>.gz"
    );

    // Re-putting identical content is idempotent: the same ref, one file.
    let again = store.put(payload).await.expect("second put");
    assert_eq!(again, blob, "identical content yields an identical ref");
    let file_count = std::fs::read_dir(&shard)
        .expect("read the content shard")
        .count();
    assert_eq!(
        file_count, 1,
        "identical content de-duplicates onto a single file"
    );
}

/// The attachment reader unions the legacy and current vocabularies: a parent
/// carrying both a canonical `sbol:attachment` edge and a legacy
/// `sbh:attachment` edge surfaces both attachments, each resolved from whichever
/// vocabulary annotates it.
///
/// A migrated SBOL2 corpus records attachments under the legacy `sbh:attachment*`
/// terms, while new attachments this app mints use the canonical `sbol:*` terms;
/// `get_attachments` must read the union so an object touched by both eras shows
/// every attachment. The fixture is seeded verbatim into the store and read back
/// through [`AttachmentService`], so this certifies the dual-vocabulary read over
/// each backend's own triples.
pub async fn attachment_read_both_vocabs(app: &AppServices) {
    const GRAPH: &str = "urn:sbol-db:conformance:attachments";
    const PARENT: &str = "http://example.org/att/parent/1";
    const CANON_ATT: &str = "http://example.org/att/canon/1";
    const LEGACY_ATT: &str = "http://example.org/att/legacy/1";

    // The parent references one attachment under each vocabulary; the canonical
    // attachment carries `sbol:*` terms and the legacy one the `sbh:attachment*`
    // annotations a migrated corpus keeps.
    let body = format!(
        "<{PARENT}> <http://sbols.org/v2#attachment> <{CANON_ATT}> .\n\
         <{PARENT}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachment> <{LEGACY_ATT}> .\n\
         <{CANON_ATT}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#Attachment> .\n\
         <{CANON_ATT}> <http://sbols.org/v2#hash> \"canonhash\" .\n\
         <{CANON_ATT}> <http://sbols.org/v2#size> \"11\" .\n\
         <{CANON_ATT}> <http://sbols.org/v2#format> <http://purl.org/NET/mediatypes/text/plain> .\n\
         <{CANON_ATT}> <http://sbols.org/v2#source> <{CANON_ATT}/download> .\n\
         <{LEGACY_ATT}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentHash> \"legacyhash\" .\n\
         <{LEGACY_ATT}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentSize> \"22\" .\n\
         <{LEGACY_ATT}> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentType> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#imageAttachment> .\n"
    );
    app.store
        .graph_store_write(
            GRAPH,
            &body,
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("seed the attachment fixture");

    let attachments = AttachmentService::new(
        app.store.clone(),
        app.sparql_update.clone(),
        app.acl_service.clone(),
        app.blobs.clone(),
    );
    let found = attachments
        .get_attachments(PARENT)
        .await
        .expect("read the parent's attachments");

    assert_eq!(
        found.len(),
        2,
        "both the canonical and legacy attachments are read: {found:?}"
    );
    let canonical = found
        .iter()
        .find(|a| a.uri == CANON_ATT)
        .expect("the canonical attachment is surfaced");
    assert_eq!(
        canonical.hash.as_deref(),
        Some("canonhash"),
        "the canonical hash is read from the sbol:hash term"
    );
    assert_eq!(canonical.size, Some(11), "the canonical size is read");
    let legacy = found
        .iter()
        .find(|a| a.uri == LEGACY_ATT)
        .expect("the legacy attachment is surfaced");
    assert_eq!(
        legacy.hash.as_deref(),
        Some("legacyhash"),
        "the legacy hash is read from the sbh:attachmentHash annotation"
    );
    assert_eq!(legacy.size, Some(22), "the legacy size is read");

    app.store
        .graph_store_clear(GRAPH)
        .await
        .expect("clear the attachment fixture");
}

/// A compliant SBOL2 submission (Turtle): a ComponentDefinition with a nested
/// SequenceAnnotation child, plus a standalone Sequence, all versioned `1`. The
/// child's URI is a path-suffix of the ComponentDefinition's persistent identity,
/// exercising the by-prefix re-home of children through the mint.
const SUBMISSION_FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "My Component" ;
    sbol:sequenceAnnotation <http://example.org/cd/anno/1> .

<http://example.org/cd/anno/1>
    a sbol:SequenceAnnotation ;
    sbol:displayId "anno" ;
    sbol:persistentIdentity <http://example.org/cd/anno> ;
    sbol:version "1" .

<http://example.org/seq/1>
    a sbol:Sequence ;
    sbol:displayId "seq" ;
    sbol:persistentIdentity <http://example.org/seq> ;
    sbol:version "1" ;
    sbol:elements "atgc" .
"#;

/// A single-top-level SBOL2 Sequence submission whose minted member carries the
/// `seqa` displayId, for the overwrite/merge scenario.
const OVERWRITE_FIXTURE_A: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .

<http://example.org/seqa/1>
    a sbol:Sequence ;
    sbol:displayId "seqa" ;
    sbol:persistentIdentity <http://example.org/seqa> ;
    sbol:version "1" ;
    sbol:elements "atgc" .
"#;

/// The counterpart to [`OVERWRITE_FIXTURE_A`] whose minted member carries the
/// `seqb` displayId, so a merge is observable as two distinct members and a
/// replace as the loss of the original.
const OVERWRITE_FIXTURE_B: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .

<http://example.org/seqb/1>
    a sbol:Sequence ;
    sbol:displayId "seqb" ;
    sbol:persistentIdentity <http://example.org/seqb> ;
    sbol:version "1" ;
    sbol:elements "gcta" .
"#;

/// `sbh:topLevel`: the self-link every minted top-level subject carries.
const SBH_TOP_LEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";
/// `sbol:member`: the collection-to-object membership edge.
const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";
/// `sbh:mutableDescription`: a mutable text field an owner may edit in place.
const SBH_MUTABLE_DESCRIPTION: &str =
    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableDescription";

/// A submission mints SynBioHub-compliant URIs and denormalization triples that
/// read back correctly. The root Collection lands at its expected URI, every
/// top-level object is a `sbol:member` minted under the submission namespace, and
/// the collection and each member carry their `sbh:topLevel` self-link and an
/// `sbh:ownedBy` stamp naming the submitter's user graph. The submission goes
/// through the facade's [`SubmissionService`] and is read back through the store,
/// certifying the mint plus the verbatim graph write end to end on each backend.
pub async fn collection_mint_roundtrip(app: &AppServices) {
    let submissions = SubmissionService::new(app.store.clone());
    let outcome = submissions
        .submit(SubmitRequest {
            owner: "conformance_submitter".to_owned(),
            id: "mintsub".to_owned(),
            version: "1".to_owned(),
            name: Some("Mint Roundtrip".to_owned()),
            description: Some("A minted submission".to_owned()),
            creator_name: Some("Conformance Submitter".to_owned()),
            citations: vec!["12345678".to_owned()],
            body: SUBMISSION_FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("submit mints the collection");

    const COLLECTION: &str =
        "http://synbiohub.org/user/conformance_submitter/mintsub/mintsub_collection/1";
    const COMPONENT: &str = "http://synbiohub.org/user/conformance_submitter/mintsub/cd/1";
    const SEQUENCE: &str = "http://synbiohub.org/user/conformance_submitter/mintsub/seq/1";
    const USER_GRAPH: &str = "http://synbiohub.org/user/conformance_submitter";

    // The minted identity in the outcome follows the SynBioHub scheme.
    assert_eq!(
        outcome.collection_uri.as_str(),
        COLLECTION,
        "root collection minted at the expected URI"
    );
    let members: Vec<&str> = outcome.members.iter().map(|m| m.as_str()).collect();
    assert!(
        members.contains(&COMPONENT),
        "the component is a minted member: {members:?}"
    );
    assert!(
        members.contains(&SEQUENCE),
        "the sequence is a minted member: {members:?}"
    );
    assert_eq!(
        members.len(),
        2,
        "exactly the two top levels are members: {members:?}"
    );

    // The collection reads back with its membership, self-link, and ownership.
    let collection_triples = app
        .store
        .triples_for_subject(COLLECTION)
        .await
        .expect("collection triples");
    assert!(
        triple_has_iri(&collection_triples, COLLECTION, SBOL2_MEMBER, COMPONENT),
        "collection members the component on read-back"
    );
    assert!(
        triple_has_iri(&collection_triples, COLLECTION, SBOL2_MEMBER, SEQUENCE),
        "collection members the sequence on read-back"
    );
    assert!(
        triple_has_iri(&collection_triples, COLLECTION, SBH_TOP_LEVEL, COLLECTION),
        "collection carries its topLevel self-link on read-back"
    );
    assert!(
        triple_has_iri(&collection_triples, COLLECTION, SBH_OWNED_BY, USER_GRAPH),
        "collection is owned by the submitter's user graph on read-back"
    );

    // Each member reads back with its own self-link and ownership stamp.
    for member in [COMPONENT, SEQUENCE] {
        let member_triples = app
            .store
            .triples_for_subject(member)
            .await
            .expect("member triples");
        assert!(
            triple_has_iri(&member_triples, member, SBH_TOP_LEVEL, member),
            "{member} carries its topLevel self-link"
        );
        assert!(
            triple_has_iri(&member_triples, member, SBH_OWNED_BY, USER_GRAPH),
            "{member} is owned by the submitter's user graph"
        );
    }

    app.store
        .graph_store_clear(&outcome.graph_iri)
        .await
        .expect("clear the minted submission graph");
}

/// The three `overwrite_merge` policies behave as SynBioHub specifies. Code 0
/// (Fail) rejects a submission whose id/version is already taken; code 2 (Merge)
/// unions the new members into the existing collection; code 1 (Replace) clears
/// the collection first, so only the replacing submission's members remain.
/// Driven through the facade's [`SubmissionService`] and observed by the
/// collection's `sbol:member` edges on each backend.
pub async fn overwrite_merge_modes(app: &AppServices) {
    const COLLECTION: &str =
        "http://synbiohub.org/user/conformance_overwrite_owner/overwritesub/overwritesub_collection/1";
    const MEMBER_A: &str =
        "http://synbiohub.org/user/conformance_overwrite_owner/overwritesub/seqa/1";
    const MEMBER_B: &str =
        "http://synbiohub.org/user/conformance_overwrite_owner/overwritesub/seqb/1";

    let submissions = SubmissionService::new(app.store.clone());
    let request = |body: &str, overwrite: ImportOverwrite| SubmitRequest {
        owner: "conformance_overwrite_owner".to_owned(),
        id: "overwritesub".to_owned(),
        version: "1".to_owned(),
        name: Some("Overwrite Modes".to_owned()),
        description: None,
        creator_name: None,
        citations: Vec::new(),
        body: body.to_owned(),
        format: SerializationFormat::Turtle,
        overwrite,
    };

    // Code 0 (Fail): the first submission into a free id/version succeeds.
    submissions
        .submit(request(OVERWRITE_FIXTURE_A, ImportOverwrite::Fail))
        .await
        .expect("first submit into a free id succeeds");
    assert!(
        collection_members(app, COLLECTION)
            .await
            .contains(&MEMBER_A.to_owned()),
        "the first submission's member is present"
    );

    // Code 0 again: the id/version is now taken, so a Fail submission is rejected.
    let err = submissions
        .submit(request(OVERWRITE_FIXTURE_A, ImportOverwrite::Fail))
        .await
        .expect_err("a colliding Fail submission is rejected");
    assert!(
        matches!(err, DomainError::InvalidInput(_)),
        "the collision is an invalid-input error: {err:?}"
    );

    // Code 2 (Merge): the new member is unioned in alongside the existing one.
    submissions
        .submit(request(OVERWRITE_FIXTURE_B, ImportOverwrite::Merge))
        .await
        .expect("merge submit succeeds");
    let merged = collection_members(app, COLLECTION).await;
    assert!(
        merged.contains(&MEMBER_A.to_owned()),
        "merge keeps the original member: {merged:?}"
    );
    assert!(
        merged.contains(&MEMBER_B.to_owned()),
        "merge adds the new member: {merged:?}"
    );

    // Code 1 (Replace): the collection is cleared first, so only the replacing
    // submission's member remains.
    submissions
        .submit(request(OVERWRITE_FIXTURE_B, ImportOverwrite::Replace))
        .await
        .expect("replace submit succeeds");
    let replaced = collection_members(app, COLLECTION).await;
    assert!(
        replaced.contains(&MEMBER_B.to_owned()),
        "replace keeps the replacing member: {replaced:?}"
    );
    assert!(
        !replaced.contains(&MEMBER_A.to_owned()),
        "replace drops the original member: {replaced:?}"
    );

    app.store
        .graph_store_clear(COLLECTION)
        .await
        .expect("clear the overwrite submission graph");
}

/// The write-authorization matrix holds across the mutation surface. An owner may
/// edit and remove an object its user graph owns; a non-owner and an anonymous
/// caller are denied every mutation with [`MutationError::NotAuthorized`]; and
/// granting a second user ownership (addOwner) widens that user's read scope to
/// the object's graph. The gate reads the `sbh:ownedBy` stamp through the facade's
/// `AclService`, so the storage core stays authorization-free; this certifies the
/// gate on each backend.
pub async fn write_authz_matrix(app: &AppServices) {
    const COLLECTION: &str =
        "http://synbiohub.org/user/conformance_authz_owner/authzsub/authzsub_collection/1";
    const OWNER_GRAPH: &str = "http://synbiohub.org/user/conformance_authz_owner";
    const INTRUDER_GRAPH: &str = "http://synbiohub.org/user/conformance_authz_intruder";
    const GRANTEE_GRAPH: &str = "http://synbiohub.org/user/conformance_authz_grantee";
    // An anonymous caller carries no owning user graph.
    const ANON_GRAPH: &str = "";

    let submissions = SubmissionService::new(app.store.clone());
    let edits = EditService::new(
        app.store.clone(),
        app.sparql_update.clone(),
        app.acl_service.clone(),
    );
    let mutations = MutationService::new(
        app.store.clone(),
        app.sparql_update.clone(),
        app.acl_service.clone(),
    );
    let permissions = PermissionService::new(app.sparql_update.clone(), app.acl_service.clone());

    let outcome = submissions
        .submit(SubmitRequest {
            owner: "conformance_authz_owner".to_owned(),
            id: "authzsub".to_owned(),
            version: "1".to_owned(),
            name: Some("Authz Matrix".to_owned()),
            description: None,
            creator_name: None,
            citations: Vec::new(),
            body: SUBMISSION_FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("owner submits the object");
    let graph = outcome.graph_iri.clone();

    // The owner may edit its own object, and the edit is durable.
    edits
        .update_mutable_description(OWNER_GRAPH, false, COLLECTION, "conformance description")
        .await
        .expect("owner edits its own object");
    let edited = app
        .store
        .triples_for_subject(COLLECTION)
        .await
        .expect("collection triples after edit");
    assert!(
        triple_has_literal(
            &edited,
            COLLECTION,
            SBH_MUTABLE_DESCRIPTION,
            "conformance description"
        ),
        "the owner's edit is durable"
    );

    // A non-owner and an anonymous caller are denied edits.
    let denied = edits
        .update_mutable_description(INTRUDER_GRAPH, false, COLLECTION, "intruder edit")
        .await
        .expect_err("a non-owner may not edit");
    assert!(
        matches!(denied, MutationError::NotAuthorized(_)),
        "a non-owner edit is not authorized: {denied:?}"
    );
    let denied = edits
        .update_mutable_description(ANON_GRAPH, false, COLLECTION, "anon edit")
        .await
        .expect_err("an anonymous caller may not edit");
    assert!(
        matches!(denied, MutationError::NotAuthorized(_)),
        "an anonymous edit is not authorized: {denied:?}"
    );

    // A non-owner and an anonymous caller are denied removes.
    let denied = mutations
        .remove(INTRUDER_GRAPH, false, COLLECTION)
        .await
        .expect_err("a non-owner may not remove");
    assert!(
        matches!(denied, MutationError::NotAuthorized(_)),
        "a non-owner remove is not authorized: {denied:?}"
    );
    let denied = mutations
        .remove(ANON_GRAPH, false, COLLECTION)
        .await
        .expect_err("an anonymous caller may not remove");
    assert!(
        matches!(denied, MutationError::NotAuthorized(_)),
        "an anonymous remove is not authorized: {denied:?}"
    );

    // The object survives every rejected mutation.
    assert!(
        !app.store
            .triples_for_subject(COLLECTION)
            .await
            .expect("collection triples after denials")
            .is_empty(),
        "the object is intact after the rejected mutations"
    );

    // addOwner widens the grantee's read scope to the object's graph: excluded
    // before the grant, admitted after.
    let before = app
        .acl_service
        .compute_scope(Some(GRANTEE_GRAPH))
        .await
        .expect("grantee scope before grant");
    assert!(
        !scope_names(&before, &graph),
        "the grantee cannot see the object's graph before the grant"
    );
    permissions
        .add_owner(OWNER_GRAPH, false, COLLECTION, GRANTEE_GRAPH)
        .await
        .expect("owner grants the grantee ownership");
    let after = app
        .acl_service
        .compute_scope(Some(GRANTEE_GRAPH))
        .await
        .expect("grantee scope after grant");
    assert!(
        scope_names(&after, &graph),
        "addOwner widens the grantee's scope to the object's graph"
    );

    // The owner may remove its own object; the object's triples are then gone.
    mutations
        .remove(OWNER_GRAPH, false, COLLECTION)
        .await
        .expect("owner removes its own object");
    assert!(
        app.store
            .triples_for_subject(COLLECTION)
            .await
            .expect("collection triples after remove")
            .is_empty(),
        "the owner's remove clears the object"
    );

    app.store
        .graph_store_clear(&graph)
        .await
        .expect("clear the authz submission graph");
}

/// The `sbol:member` object URIs a collection currently holds, read from the
/// store.
async fn collection_members(app: &AppServices, collection: &str) -> Vec<String> {
    app.store
        .triples_for_subject(collection)
        .await
        .expect("collection triples")
        .into_iter()
        .filter(|t| t.predicate.as_str() == SBOL2_MEMBER)
        .filter_map(|t| match t.object {
            ObjectTerm::Iri(iri) => Some(iri.as_str().to_owned()),
            _ => None,
        })
        .collect()
}

/// Whether `triples` holds `(subject, predicate, object)` with an IRI object.
fn triple_has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.iter().any(|t| {
        matches!(&t.subject, SubjectTerm::Iri(s) if s.as_str() == subject)
            && t.predicate.as_str() == predicate
            && matches!(&t.object, ObjectTerm::Iri(o) if o.as_str() == object)
    })
}

/// Whether `triples` holds `(subject, predicate, value)` with a literal object.
fn triple_has_literal(triples: &[Triple], subject: &str, predicate: &str, value: &str) -> bool {
    triples.iter().any(|t| {
        matches!(&t.subject, SubjectTerm::Iri(s) if s.as_str() == subject)
            && t.predicate.as_str() == predicate
            && matches!(&t.object, ObjectTerm::Literal { value: v, .. } if v == value)
    })
}

/// Whether the closure holds a triple with the given IRI subject, predicate,
/// and literal object value.
fn closure_has_literal(triples: &[Triple], subject: &str, predicate: &str, value: &str) -> bool {
    triples.iter().any(|t| {
        matches!(&t.subject, SubjectTerm::Iri(iri) if iri.as_str() == subject)
            && t.predicate.as_str() == predicate
            && matches!(&t.object, ObjectTerm::Literal { value: v, .. } if v == value)
    })
}

/// The full conformance IRI for a locally-named object.
fn conformance_iri(local: &str) -> String {
    format!("https://example.org/sbol-db/conformance/{local}")
}

/// An [`IndexedPart`] in the public graph, named by its local id, with a unit
/// PageRank.
fn indexed_part(
    local: &str,
    display_id: &str,
    description: &str,
    type_iri: &str,
    pagerank: f64,
) -> IndexedPart {
    indexed_part_ranked(
        &conformance_iri(local),
        display_id,
        description,
        type_iri,
        pagerank,
    )
}

/// An [`IndexedPart`] in the public graph addressed by its full IRI, carrying
/// an explicit PageRank.
fn indexed_part_ranked(
    subject: &str,
    display_id: &str,
    description: &str,
    type_iri: &str,
    pagerank: f64,
) -> IndexedPart {
    IndexedPart {
        subject: subject.to_owned(),
        graph: PUBLIC_GRAPH.to_owned(),
        display_id: Some(display_id.to_owned()),
        name: None,
        description: Some(description.to_owned()),
        version: Some("1".to_owned()),
        type_iris: vec![type_iri.to_owned()],
        keywords: display_id.to_owned(),
        pagerank,
    }
}

/// An [`IndexedPart`] placed in a named graph, for scope-enforcement scenarios.
fn indexed_part_in(subject: &str, graph: &str, display_id: &str, type_iri: &str) -> IndexedPart {
    IndexedPart {
        subject: subject.to_owned(),
        graph: graph.to_owned(),
        display_id: Some(display_id.to_owned()),
        name: None,
        description: None,
        version: Some("1".to_owned()),
        type_iris: vec![type_iri.to_owned()],
        keywords: display_id.to_owned(),
        pagerank: 1.0,
    }
}

/// The hits for a free-text query under an unrestricted scope, in ranked order.
async fn ranked(app: &AppServices, term: &str) -> Vec<sbol_db_app::Hit> {
    let query = FacetedSearch {
        free_text: Some(term.to_owned()),
        ..FacetedSearch::default()
    };
    app.ranked_search(&query, GraphScope::Union)
        .await
        .expect("ranked search")
        .0
}

/// The subject IRIs a free-text query surfaces under `scope`.
async fn ranked_subjects(app: &AppServices, term: &str, scope: GraphScope) -> Vec<String> {
    let query = FacetedSearch {
        free_text: Some(term.to_owned()),
        ..FacetedSearch::default()
    };
    app.ranked_search(&query, scope)
        .await
        .expect("scoped ranked search")
        .0
        .into_iter()
        .map(|hit| hit.subject)
        .collect()
}

/// Whether `scope` authorizes reads of the named `graph`.
fn scope_names(scope: &GraphScope, graph: &str) -> bool {
    match scope {
        GraphScope::Union => true,
        GraphScope::Only(graphs) => graphs.iter().any(|g| g == graph),
    }
}

/// The subject IRIs carrying an `sbol:displayId`, read under `scope`, as the
/// serialized SPARQL result text. Membership is tested with the quoted IRI so a
/// hit is an exact result binding, not an incidental substring.
async fn subjects_under_scope(app: &AppServices, scope: &GraphScope) -> ScopedSubjects {
    let options = SparqlOptions {
        authorized_graphs: scope.clone(),
        ..SparqlOptions::default()
    };
    let outcome = app
        .sparql
        .execute(
            "SELECT ?s WHERE { ?s <http://sbols.org/v3#displayId> ?id }",
            None,
            None,
            &options,
        )
        .await
        .expect("scoped SPARQL read");
    ScopedSubjects(String::from_utf8(outcome.payload.body).expect("utf8 SPARQL results"))
}

/// Serialized SPARQL SELECT results, queried for whether a given IRI is bound.
struct ScopedSubjects(String);

impl ScopedSubjects {
    fn contains(&self, iri: &str) -> bool {
        self.0.contains(&format!("\"{iri}\""))
    }
}

fn new_job(kind: &str, idempotency_key: Option<String>) -> NewJob {
    NewJob {
        kind: kind.to_owned(),
        payload: serde_json::json!({ "conformance": true }),
        queue: None,
        priority: None,
        max_attempts: None,
        idempotency_key,
        available_at: None,
        parent_job_id: None,
        correlation_id: None,
    }
}
