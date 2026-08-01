//! Durable audit evidence for administrator mutations.
//!
//! Object collaboration events live beside their protected objects. Admin
//! events instead use one dedicated named graph because their targets can be
//! accounts, configuration keys, jobs, or a whole registry restore. The
//! service only appends triples; no update path is exposed for rewriting or
//! deleting earlier evidence.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use sbol_db_core::DomainError;
use sbol_db_sparql::{GraphScope, ResultFormat, SparqlEngine, SparqlOptions, SparqlUpdateEngine};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const AUDIT_GRAPH: &str = "urn:sbol-db:admin-audit";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
const EVENT_TYPE: &str = "https://sbol-db.org/terms#AdminAuditEvent";
const EVENT_ACTION: &str = "https://sbol-db.org/terms#adminAction";
const EVENT_ACTOR: &str = "https://sbol-db.org/terms#adminActor";
const EVENT_TARGET: &str = "https://sbol-db.org/terms#adminTarget";
const EVENT_OUTCOME: &str = "https://sbol-db.org/terms#adminOutcome";
const EVENT_DETAIL: &str = "https://sbol-db.org/terms#adminDetail";
const EVENT_OCCURRED_AT: &str = "https://sbol-db.org/terms#auditOccurredAt";

/// Lifecycle state of an administrator command. Destructive commands record
/// `attempted` before mutation and a terminal outcome afterward.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminAuditOutcome {
    Attempted,
    Succeeded,
    Failed,
}

impl AdminAuditOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Attempted => "attempted",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "attempted" => Some(Self::Attempted),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One immutable administrator event. `detail` must remain operational and
/// non-secret; adapters never put passwords, API keys, or remote credentials
/// into this record.
#[derive(Clone, Debug, Serialize)]
pub struct AdminAuditEvent {
    pub iri: String,
    pub action: String,
    pub actor: String,
    pub target: String,
    pub outcome: AdminAuditOutcome,
    pub detail: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

/// Append and read the administrator event stream.
#[derive(Clone)]
pub struct AdminAuditService {
    sparql: Arc<SparqlEngine>,
    sparql_update: Arc<SparqlUpdateEngine>,
}

impl AdminAuditService {
    pub fn new(sparql: Arc<SparqlEngine>, sparql_update: Arc<SparqlUpdateEngine>) -> Self {
        Self {
            sparql,
            sparql_update,
        }
    }

    /// Append one event to the dedicated graph. Administrator targets are not
    /// uniformly IRIs, so actor and target are escaped RDF string literals.
    pub async fn record(
        &self,
        action: &str,
        actor: &str,
        target: &str,
        outcome: AdminAuditOutcome,
        detail: Option<&str>,
    ) -> Result<AdminAuditEvent, DomainError> {
        let event = AdminAuditEvent {
            iri: format!("urn:uuid:{}", Uuid::new_v4()),
            action: required(action, "action")?,
            actor: required(actor, "actor")?,
            target: required(target, "target")?,
            outcome,
            detail: detail.filter(|value| !value.is_empty()).map(str::to_owned),
            occurred_at: Utc::now(),
        };
        let occurred = event
            .occurred_at
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut body = format!(
            "<{}> <{RDF_TYPE}> <{EVENT_TYPE}> ;\n\
             <{EVENT_ACTION}> \"{}\" ;\n\
             <{EVENT_ACTOR}> \"{}\" ;\n\
             <{EVENT_TARGET}> \"{}\" ;\n\
             <{EVENT_OUTCOME}> \"{}\" ;\n\
             <{EVENT_OCCURRED_AT}> \"{occurred}\"^^<{XSD_DATE_TIME}>",
            event.iri,
            escape_literal(&event.action),
            escape_literal(&event.actor),
            escape_literal(&event.target),
            event.outcome.as_str(),
        );
        if let Some(detail) = event.detail.as_deref() {
            body.push_str(&format!(
                " ;\n<{EVENT_DETAIL}> \"{}\"",
                escape_literal(detail)
            ));
        }
        body.push_str(" .");
        let update = format!("INSERT DATA {{ GRAPH <{AUDIT_GRAPH}> {{ {body} }} }}");
        self.sparql_update
            .execute(&update, None, &SparqlOptions::default())
            .await
            .map_err(DomainError::from)?;
        Ok(event)
    }

    /// Read the newest administrator events first, with a hard safety bound.
    pub async fn list(&self, limit: u32) -> Result<Vec<AdminAuditEvent>, DomainError> {
        let limit = limit.clamp(1, 1_000);
        let query = format!(
            "SELECT ?event ?action ?actor ?target ?outcome ?detail ?occurred WHERE {{\n\
             GRAPH <{AUDIT_GRAPH}> {{\n\
               ?event <{RDF_TYPE}> <{EVENT_TYPE}> ;\n\
                      <{EVENT_ACTION}> ?action ;\n\
                      <{EVENT_ACTOR}> ?actor ;\n\
                      <{EVENT_TARGET}> ?target ;\n\
                      <{EVENT_OUTCOME}> ?outcome ;\n\
                      <{EVENT_OCCURRED_AT}> ?occurred .\n\
               OPTIONAL {{ ?event <{EVENT_DETAIL}> ?detail . }}\n\
             }}\n\
             }} ORDER BY DESC(?occurred) DESC(?event) LIMIT {limit}"
        );
        let options = SparqlOptions {
            authorized_graphs: GraphScope::Union,
            max_rows: limit as usize,
            ..SparqlOptions::default()
        };
        let outcome = self
            .sparql
            .execute(&query, Some(ResultFormat::Json), None, &options)
            .await
            .map_err(DomainError::from)?;
        let value: serde_json::Value = serde_json::from_slice(&outcome.payload.body)?;
        let bindings = value
            .get("results")
            .and_then(|results| results.get("bindings"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                DomainError::Database("SPARQL admin audit response has no results.bindings".into())
            })?;
        bindings.iter().map(parse_binding).collect()
    }
}

fn parse_binding(binding: &serde_json::Value) -> Result<AdminAuditEvent, DomainError> {
    let outcome =
        AdminAuditOutcome::parse(required_binding(binding, "outcome")?).ok_or_else(|| {
            DomainError::Database("admin audit event has an invalid outcome".to_owned())
        })?;
    let occurred = required_binding(binding, "occurred")?;
    let occurred_at = DateTime::parse_from_rfc3339(occurred)
        .map_err(|error| {
            DomainError::Database(format!("invalid admin audit timestamp {occurred}: {error}"))
        })?
        .with_timezone(&Utc);
    Ok(AdminAuditEvent {
        iri: required_binding(binding, "event")?.to_owned(),
        action: required_binding(binding, "action")?.to_owned(),
        actor: required_binding(binding, "actor")?.to_owned(),
        target: required_binding(binding, "target")?.to_owned(),
        outcome,
        detail: binding_value(binding, "detail").map(str::to_owned),
        occurred_at,
    })
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
        .ok_or_else(|| DomainError::Database(format!("admin audit event has no {name} binding")))
}

fn required(value: &str, field: &str) -> Result<String, DomainError> {
    let value = value.trim();
    if value.is_empty() {
        Err(DomainError::InvalidInput(format!(
            "admin audit {field} must not be empty"
        )))
    } else {
        Ok(value.to_owned())
    }
}

fn escape_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
