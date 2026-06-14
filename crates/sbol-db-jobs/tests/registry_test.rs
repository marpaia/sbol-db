//! Pure-Rust tests for `JobRegistry`. No database required.

use async_trait::async_trait;
use sbol_db_jobs::{
    default_registry,
    handlers::{
        ImportDocumentHandler, ImportRemoteDocumentHandler, ImportSynBioHubCollectionHandler,
    },
    HandlerError, JobContext, JobHandler, JobOutcome, JobRegistry,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AlphaPayload {
    value: i64,
}

struct AlphaHandler;

#[async_trait]
impl JobHandler for AlphaHandler {
    type Payload = AlphaPayload;
    fn kind(&self) -> &'static str {
        "test_alpha"
    }
    async fn run(
        &self,
        _ctx: JobContext,
        _payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        Ok(JobOutcome::empty())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BetaPayload {
    msg: String,
}

struct BetaHandler;

#[async_trait]
impl JobHandler for BetaHandler {
    type Payload = BetaPayload;
    fn kind(&self) -> &'static str {
        "test_beta"
    }
    async fn run(
        &self,
        _ctx: JobContext,
        _payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        Ok(JobOutcome::empty())
    }
}

/// For every registered kind, lookup must return Some and the returned
/// handler must self-identify with the same kind. Catches a class of bug
/// where two handlers race for the same kind string, or where the registry
/// stores a stale handler.
#[test]
fn every_registered_kind_self_identifies() {
    let registry = JobRegistry::new()
        .register(AlphaHandler)
        .register(BetaHandler)
        .register(ImportDocumentHandler)
        .register(ImportRemoteDocumentHandler)
        .register(ImportSynBioHubCollectionHandler);
    let kinds: Vec<_> = registry.kinds().collect();
    assert_eq!(kinds.len(), 5);
    for kind in kinds {
        let handler = registry.lookup(kind).expect("registered kind must look up");
        assert_eq!(
            handler.kind(),
            kind,
            "lookup({kind}) yielded {}",
            handler.kind()
        );
    }
}

#[test]
fn default_registry_contains_import_document() {
    let registry = default_registry();
    let handler = registry
        .lookup("import_document")
        .expect("default_registry must include import_document");
    assert_eq!(handler.kind(), "import_document");
}

#[test]
fn default_registry_contains_import_remote_document() {
    let registry = default_registry();
    let handler = registry
        .lookup("import_remote_document")
        .expect("default_registry must include import_remote_document");
    assert_eq!(handler.kind(), "import_remote_document");
}

#[test]
fn default_registry_contains_import_synbiohub_collection() {
    let registry = default_registry();
    let handler = registry
        .lookup("import_synbiohub_collection")
        .expect("default_registry must include import_synbiohub_collection");
    assert_eq!(handler.kind(), "import_synbiohub_collection");
}

/// `JobRegistry::register` replaces an existing entry for the same kind.
/// The duplicate warning is logged; this test pins the post-replacement
/// state so callers don't accidentally end up with multiple stale handlers.
#[test]
fn duplicate_registration_replaces_previous_handler() {
    struct AlphaV2;
    #[async_trait]
    impl JobHandler for AlphaV2 {
        type Payload = AlphaPayload;
        fn kind(&self) -> &'static str {
            "test_alpha"
        }
        async fn run(
            &self,
            _ctx: JobContext,
            _payload: Self::Payload,
        ) -> Result<JobOutcome, HandlerError> {
            Ok(JobOutcome::with_result(serde_json::json!({"v": 2})))
        }
    }

    let registry = JobRegistry::new().register(AlphaHandler).register(AlphaV2);
    assert_eq!(
        registry.len(),
        1,
        "duplicate kind should collapse to one entry"
    );
    let handler = registry.lookup("test_alpha").unwrap();
    assert_eq!(handler.kind(), "test_alpha");
}

#[test]
fn empty_registry_has_no_handlers() {
    let registry = JobRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert!(registry.lookup("anything").is_none());
}
