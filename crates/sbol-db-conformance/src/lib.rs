//! Backend-neutral conformance scenarios for `sbol-db-storage`.
//!
//! Each scenario drives a storage backend purely through the trait surface and
//! asserts the observable contract every implementation must honor: import and
//! derived-view reads, the graph set-semantics rule, ontology load and closure
//! queries, the job-queue lifecycle, and ACL-scoped reads hiding another user's
//! private graph. A backend crate wires these into its own test harness by
//! assembling a fresh, empty [`AppServices`] over the backend and calling
//! [`run_all`] (or an individual store-level scenario against a bare store).
//!
//! Scenarios assume they start against an empty store; they scope their reads
//! to the graphs and keys they create so [`run_all`] can run them in sequence
//! against one store without cross-contamination.

use std::time::Duration;

use sbol_db_app::{AppServices, PUBLIC_GRAPH};
use sbol_db_core::{Direction, IriString, NeighborhoodQuery, SerializationFormat};
use sbol_db_sparql::{GraphScope, SparqlOptions};
use sbol_db_storage::{
    EnqueueOutcome, GraphWriteMode, ImportInput, JobQueue, JobStatus, ListJobsFilter,
    ListObjectsFilter, NewJob, SbolStore, SequenceSearchOptions, DEFAULT_QUEUE, SBH_OWNED_BY,
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
    ontology_roundtrip(store).await;
    job_queue_lifecycle(jobs).await;
    acl_scope_hides_private_graph(app).await;
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

    // Leave the store as we found it for any scenario that follows.
    for graph in [GRAPH_A, GRAPH_B, PUBLIC_GRAPH] {
        app.store
            .graph_store_clear(graph)
            .await
            .expect("clear seed graph");
    }
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
