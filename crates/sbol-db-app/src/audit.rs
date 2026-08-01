//! Immutable, RDF-backed audit events for object-scoped collaboration.
//!
//! Events live in the same named graph as the object they describe. Mutating
//! services compose the event triples into the same SPARQL Update as the state
//! change, so success cannot leave collaboration state without its evidence.
//! Readers receive a typed projection; they never interpret RDF in an HTTP
//! adapter or in React.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use sbol_db_core::{DomainError, IriString};
use sbol_db_sparql::{GraphScope, ResultFormat, SparqlEngine, SparqlOptions};
use serde::Serialize;
use uuid::Uuid;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
const EVENT_TYPE: &str = "https://sbol-db.org/terms#AuditEvent";
const EVENT_OBJECT: &str = "https://sbol-db.org/terms#auditObject";
const EVENT_ACTION: &str = "https://sbol-db.org/terms#auditAction";
const EVENT_ACTOR: &str = "https://sbol-db.org/terms#auditActor";
const EVENT_SUBJECT: &str = "https://sbol-db.org/terms#auditSubject";
const EVENT_NOTE: &str = "https://sbol-db.org/terms#auditNote";
const EVENT_OCCURRED_AT: &str = "https://sbol-db.org/terms#auditOccurredAt";

/// The object-scoped state transitions whose evidence is exposed by V2.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    ShareGranted,
    ShareRevoked,
    OwnershipTransferred,
    ReviewRequested,
    ReviewApproved,
    ReviewChangesRequested,
}

impl AuditAction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ShareGranted => "share_granted",
            Self::ShareRevoked => "share_revoked",
            Self::OwnershipTransferred => "ownership_transferred",
            Self::ReviewRequested => "review_requested",
            Self::ReviewApproved => "review_approved",
            Self::ReviewChangesRequested => "review_changes_requested",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "share_granted" => Some(Self::ShareGranted),
            "share_revoked" => Some(Self::ShareRevoked),
            "ownership_transferred" => Some(Self::OwnershipTransferred),
            "review_requested" => Some(Self::ReviewRequested),
            "review_approved" => Some(Self::ReviewApproved),
            "review_changes_requested" => Some(Self::ReviewChangesRequested),
            _ => None,
        }
    }

    pub(crate) fn is_review(self) -> bool {
        matches!(
            self,
            Self::ReviewRequested | Self::ReviewApproved | Self::ReviewChangesRequested
        )
    }
}

/// One append-only audit event. `subject_graph` is the recipient/new owner or
/// assigned curator, depending on the action.
#[derive(Clone, Debug, Serialize)]
pub struct AuditEvent {
    pub iri: String,
    pub object_iri: String,
    pub action: AuditAction,
    pub actor_graph: String,
    pub subject_graph: Option<String>,
    pub note: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

/// Builds and reads object audit evidence.
#[derive(Clone)]
pub struct AuditService {
    sparql: Arc<SparqlEngine>,
}

impl AuditService {
    pub fn new(sparql: Arc<SparqlEngine>) -> Self {
        Self { sparql }
    }

    /// Read all events for one object. The caller must enforce object-level
    /// authorization before invoking this query.
    pub async fn for_object(&self, object_iri: &str) -> Result<Vec<AuditEvent>, DomainError> {
        IriString::new(object_iri.to_owned())?;
        let query = format!(
            "SELECT ?event ?object ?action ?actor ?subject ?note ?occurred WHERE {{\n\
             ?event <{RDF_TYPE}> <{EVENT_TYPE}> ;\n\
                    <{EVENT_OBJECT}> <{object_iri}> ;\n\
                    <{EVENT_OBJECT}> ?object ;\n\
                    <{EVENT_ACTION}> ?action ;\n\
                    <{EVENT_ACTOR}> ?actor ;\n\
                    <{EVENT_OCCURRED_AT}> ?occurred .\n\
             OPTIONAL {{ ?event <{EVENT_SUBJECT}> ?subject . }}\n\
             OPTIONAL {{ ?event <{EVENT_NOTE}> ?note . }}\n\
             }} ORDER BY ?occurred ?event"
        );
        self.query(&query).await
    }

    /// Read the complete event stream for an application service that will
    /// apply its own actor/assignee authorization filter.
    pub(crate) async fn all(&self) -> Result<Vec<AuditEvent>, DomainError> {
        let query = format!(
            "SELECT ?event ?object ?action ?actor ?subject ?note ?occurred WHERE {{\n\
             ?event <{RDF_TYPE}> <{EVENT_TYPE}> ;\n\
                    <{EVENT_OBJECT}> ?object ;\n\
                    <{EVENT_ACTION}> ?action ;\n\
                    <{EVENT_ACTOR}> ?actor ;\n\
                    <{EVENT_OCCURRED_AT}> ?occurred .\n\
             OPTIONAL {{ ?event <{EVENT_SUBJECT}> ?subject . }}\n\
             OPTIONAL {{ ?event <{EVENT_NOTE}> ?note . }}\n\
             }} ORDER BY ?occurred ?event"
        );
        self.query(&query).await
    }

    async fn query(&self, query: &str) -> Result<Vec<AuditEvent>, DomainError> {
        let options = SparqlOptions {
            authorized_graphs: GraphScope::Union,
            max_rows: 10_000,
            ..SparqlOptions::default()
        };
        let outcome = self
            .sparql
            .execute(query, Some(ResultFormat::Json), None, &options)
            .await
            .map_err(DomainError::from)?;
        if outcome.payload.truncated {
            return Err(DomainError::Unavailable(
                "audit result exceeded its 10000-row safety bound".to_owned(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&outcome.payload.body)?;
        let bindings = value
            .get("results")
            .and_then(|results| results.get("bindings"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                DomainError::Database("SPARQL audit response has no results.bindings".to_owned())
            })?;
        let mut events = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let Some(action) = binding_value(binding, "action").and_then(AuditAction::parse) else {
                continue;
            };
            let occurred = binding_value(binding, "occurred").ok_or_else(|| {
                DomainError::Database("audit event has no occurred timestamp".to_owned())
            })?;
            let occurred_at = DateTime::parse_from_rfc3339(occurred)
                .map_err(|error| {
                    DomainError::Database(format!("invalid audit timestamp {occurred}: {error}"))
                })?
                .with_timezone(&Utc);
            events.push(AuditEvent {
                iri: required_binding(binding, "event")?.to_owned(),
                object_iri: required_binding(binding, "object")?.to_owned(),
                action,
                actor_graph: required_binding(binding, "actor")?.to_owned(),
                subject_graph: binding_value(binding, "subject").map(str::to_owned),
                note: binding_value(binding, "note").map(str::to_owned),
                occurred_at,
            });
        }
        events.sort_by(|left, right| {
            left.occurred_at
                .cmp(&right.occurred_at)
                .then_with(|| left.iri.cmp(&right.iri))
        });
        Ok(events)
    }
}

/// Build one event's Turtle-compatible SPARQL triples. Callers place this body
/// in the same explicit `GRAPH` block as the protected mutation.
pub(crate) fn event_triples(
    object_iri: &str,
    action: AuditAction,
    actor_graph: &str,
    subject_graph: Option<&str>,
    note: Option<&str>,
) -> Result<String, DomainError> {
    IriString::new(object_iri.to_owned())?;
    IriString::new(actor_graph.to_owned())?;
    if let Some(subject) = subject_graph {
        IriString::new(subject.to_owned())?;
    }
    let event_iri = format!("urn:uuid:{}", Uuid::new_v4());
    let occurred = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut triples = format!(
        "<{event_iri}> <{RDF_TYPE}> <{EVENT_TYPE}> ;\n\
         <{EVENT_OBJECT}> <{object_iri}> ;\n\
         <{EVENT_ACTION}> \"{}\" ;\n\
         <{EVENT_ACTOR}> <{actor_graph}> ;\n\
         <{EVENT_OCCURRED_AT}> \"{occurred}\"^^<{XSD_DATE_TIME}>",
        action.as_str()
    );
    if let Some(subject) = subject_graph {
        triples.push_str(&format!(" ;\n<{EVENT_SUBJECT}> <{subject}>"));
    }
    if let Some(note) = note.filter(|note| !note.is_empty()) {
        triples.push_str(&format!(" ;\n<{EVENT_NOTE}> \"{}\"", escape_literal(note)));
    }
    triples.push_str(" .");
    Ok(triples)
}

fn binding_value<'a>(binding: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    binding
        .get(name)
        .and_then(|value| value.get("value"))
        .and_then(serde_json::Value::as_str)
}

fn required_binding<'a>(
    binding: &'a serde_json::Value,
    name: &str,
) -> Result<&'a str, DomainError> {
    binding_value(binding, name)
        .ok_or_else(|| DomainError::Database(format!("audit event has no {name} binding")))
}

fn escape_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
