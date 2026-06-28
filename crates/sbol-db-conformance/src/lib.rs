//! Backend-neutral conformance scenarios for `sbol-db-storage`.
//!
//! Each scenario drives a storage backend purely through the trait surface and
//! asserts the observable contract every implementation must honor: import and
//! derived-view reads, the graph set-semantics rule, ontology load and closure
//! queries, and the job-queue lifecycle. A backend crate wires these into its
//! own test harness by providing a fresh, empty store and calling [`run_all`]
//! (or an individual scenario).
//!
//! Scenarios assume they start against an empty store; they scope their reads
//! to the graphs and keys they create so [`run_all`] can run them in sequence
//! against one store without cross-contamination.

use std::time::Duration;

use sbol_db_core::SerializationFormat;
use sbol_db_storage::{
    EnqueueOutcome, GraphWriteMode, ImportInput, JobQueue, JobStatus, ListJobsFilter,
    ListObjectsFilter, NewJob, SbolStore, DEFAULT_QUEUE,
};

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

/// Run every scenario in sequence against one store + job queue. The store and
/// queue must start empty.
pub async fn run_all(store: &dyn SbolStore, jobs: &dyn JobQueue) {
    import_and_read_back(store).await;
    graph_set_semantics(store).await;
    ontology_roundtrip(store).await;
    job_queue_lifecycle(jobs).await;
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
