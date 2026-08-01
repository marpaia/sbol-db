//! Curator review workflow over immutable object audit events.
//!
//! A request atomically grants the assigned curator read-only access and
//! appends `review_requested` evidence. Decisions append a new event rather
//! than rewriting history. The current case is a deterministic projection of
//! that event stream, keeping lifecycle semantics in the application layer.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, IriString};
use sbol_db_sparql::{SparqlOptions, SparqlUpdateEngine};
use serde::Serialize;

use crate::acl::AclService;
use crate::audit::{event_triples, AuditAction, AuditEvent, AuditService};
use crate::mutation::MutationError;

const SBH_CAN_VIEW: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#canView";
const MAX_REVIEW_NOTE_BYTES: usize = 4_000;

/// A curator's decision on the currently pending review.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
}

/// The derived state of the latest review cycle for an object.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Pending,
    Approved,
    ChangesRequested,
}

/// The latest review cycle for one object. `events` begins with its immutable
/// request and includes every subsequent decision in that cycle.
#[derive(Clone, Debug, Serialize)]
pub struct ReviewCase {
    pub object_iri: String,
    pub curator_graph: String,
    pub requested_by_graph: String,
    pub status: ReviewStatus,
    pub updated_at: DateTime<Utc>,
    pub note: Option<String>,
    pub events: Vec<AuditEvent>,
}

#[derive(Clone)]
pub struct ReviewService {
    sparql_update: Arc<SparqlUpdateEngine>,
    acl_service: AclService,
    audit: AuditService,
}

impl ReviewService {
    pub fn new(
        sparql: Arc<sbol_db_sparql::SparqlEngine>,
        sparql_update: Arc<SparqlUpdateEngine>,
        acl_service: AclService,
    ) -> Self {
        Self {
            sparql_update,
            acl_service,
            audit: AuditService::new(sparql),
        }
    }

    /// Ask one curator to review an object. The caller must own the object (or
    /// be an administrator), and the target role is validated in this layer so
    /// alternate adapters cannot bypass it.
    pub async fn request(
        &self,
        caller_graph: &str,
        is_admin: bool,
        object_iri: &str,
        curator_graph: &str,
        target_is_curator: bool,
        note: Option<&str>,
    ) -> Result<ReviewCase, MutationError> {
        validate_iri(caller_graph)?;
        validate_iri(object_iri)?;
        validate_iri(curator_graph)?;
        if !target_is_curator {
            return Err(DomainError::InvalidInput(
                "review requests require an active curator".to_owned(),
            )
            .into());
        }
        if caller_graph == curator_graph {
            return Err(DomainError::InvalidInput(
                "a submitter cannot be their own assigned curator".to_owned(),
            )
            .into());
        }
        let note = normalize_note(note)?;
        let graph = self
            .authorize_write(caller_graph, is_admin, object_iri)
            .await?;
        if self
            .latest_case(object_iri)
            .await?
            .is_some_and(|case| case.status == ReviewStatus::Pending)
        {
            return Err(DomainError::InvalidInput(
                "this object already has a pending review".to_owned(),
            )
            .into());
        }

        let event = event_triples(
            object_iri,
            AuditAction::ReviewRequested,
            caller_graph,
            Some(curator_graph),
            note.as_deref(),
        )?;
        let update = format!(
            "INSERT DATA {{\n\
             GRAPH <{curator_graph}> {{ <{curator_graph}> <{SBH_CAN_VIEW}> <{object_iri}> . }}\n\
             GRAPH <{graph}> {{ {event} }}\n\
             }}"
        );
        self.run_global(&update).await?;
        self.latest_case(object_iri).await?.ok_or_else(|| {
            DomainError::Database("review request was not visible after commit".to_owned()).into()
        })
    }

    /// Append a curator decision to the latest pending review cycle.
    pub async fn decide(
        &self,
        caller_graph: &str,
        is_curator: bool,
        is_admin: bool,
        object_iri: &str,
        decision: ReviewDecision,
        note: Option<&str>,
    ) -> Result<ReviewCase, MutationError> {
        validate_iri(caller_graph)?;
        validate_iri(object_iri)?;
        if !is_curator && !is_admin {
            return Err(MutationError::NotAuthorized(object_iri.to_owned()));
        }
        let note = normalize_note(note)?;
        let current = self.latest_case(object_iri).await?.ok_or_else(|| {
            DomainError::InvalidInput("this object has no review request".to_owned())
        })?;
        if current.status != ReviewStatus::Pending {
            return Err(DomainError::InvalidInput(
                "this object's latest review is not pending".to_owned(),
            )
            .into());
        }
        if !is_admin && current.curator_graph != caller_graph {
            return Err(MutationError::NotAuthorized(object_iri.to_owned()));
        }
        let graph = self
            .acl_service
            .graph_of_subject(object_iri)
            .await?
            .ok_or_else(|| MutationError::NotFound(object_iri.to_owned()))?;
        let action = match decision {
            ReviewDecision::Approve => AuditAction::ReviewApproved,
            ReviewDecision::RequestChanges => AuditAction::ReviewChangesRequested,
        };
        let event = event_triples(
            object_iri,
            action,
            caller_graph,
            Some(&current.curator_graph),
            note.as_deref(),
        )?;
        let update = format!("INSERT DATA {{ GRAPH <{graph}> {{ {event} }} }}");
        self.run_global(&update).await?;
        self.latest_case(object_iri).await?.ok_or_else(|| {
            DomainError::Database("review decision was not visible after commit".to_owned()).into()
        })
    }

    /// Latest cases relevant to the caller. Administrators see every case;
    /// other accounts see cases they requested or were assigned.
    pub async fn list_for(
        &self,
        caller_graph: &str,
        is_admin: bool,
    ) -> Result<Vec<ReviewCase>, DomainError> {
        validate_iri(caller_graph)?;
        let mut cases = latest_cases(self.audit.all().await?);
        if !is_admin {
            cases.retain(|case| {
                case.curator_graph == caller_graph || case.requested_by_graph == caller_graph
            });
        }
        Ok(cases)
    }

    /// Latest review state for an object. Authorization remains at the adapter
    /// boundary because owners, assigned curators, and administrators have
    /// different allowed projections.
    pub async fn latest_case(&self, object_iri: &str) -> Result<Option<ReviewCase>, DomainError> {
        validate_iri(object_iri)?;
        Ok(latest_case(
            object_iri,
            self.audit.for_object(object_iri).await?,
        ))
    }

    async fn authorize_write(
        &self,
        caller_graph: &str,
        is_admin: bool,
        object_iri: &str,
    ) -> Result<String, MutationError> {
        let graph = self
            .acl_service
            .graph_of_subject(object_iri)
            .await?
            .ok_or_else(|| MutationError::NotFound(object_iri.to_owned()))?;
        if !self
            .acl_service
            .can_write(caller_graph, is_admin, object_iri, &graph)
            .await?
        {
            return Err(MutationError::NotAuthorized(object_iri.to_owned()));
        }
        Ok(graph)
    }

    async fn run_global(&self, update: &str) -> Result<(), MutationError> {
        self.sparql_update
            .execute(update, None, &SparqlOptions::default())
            .await?;
        Ok(())
    }
}

fn latest_cases(events: Vec<AuditEvent>) -> Vec<ReviewCase> {
    let mut by_object: BTreeMap<String, Vec<AuditEvent>> = BTreeMap::new();
    for event in events.into_iter().filter(|event| event.action.is_review()) {
        by_object
            .entry(event.object_iri.clone())
            .or_default()
            .push(event);
    }
    by_object
        .into_iter()
        .filter_map(|(object, events)| latest_case(&object, events))
        .collect()
}

fn latest_case(object_iri: &str, mut events: Vec<AuditEvent>) -> Option<ReviewCase> {
    events.retain(|event| event.action.is_review() && event.object_iri == object_iri);
    events.sort_by(|left, right| {
        left.occurred_at
            .cmp(&right.occurred_at)
            .then_with(|| left.iri.cmp(&right.iri))
    });
    let request_index = events
        .iter()
        .rposition(|event| event.action == AuditAction::ReviewRequested)?;
    let cycle = events.split_off(request_index);
    let request = cycle.first()?;
    let curator_graph = request.subject_graph.clone()?;
    let latest = cycle.last()?;
    let status = match latest.action {
        AuditAction::ReviewRequested => ReviewStatus::Pending,
        AuditAction::ReviewApproved => ReviewStatus::Approved,
        AuditAction::ReviewChangesRequested => ReviewStatus::ChangesRequested,
        _ => return None,
    };
    Some(ReviewCase {
        object_iri: object_iri.to_owned(),
        curator_graph,
        requested_by_graph: request.actor_graph.clone(),
        status,
        updated_at: latest.occurred_at,
        note: latest.note.clone(),
        events: cycle,
    })
}

fn normalize_note(note: Option<&str>) -> Result<Option<String>, DomainError> {
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    if note.is_some_and(|note| note.len() > MAX_REVIEW_NOTE_BYTES) {
        return Err(DomainError::InvalidInput(format!(
            "review notes may not exceed {MAX_REVIEW_NOTE_BYTES} bytes"
        )));
    }
    Ok(note.map(str::to_owned))
}

fn validate_iri(value: &str) -> Result<(), DomainError> {
    Ok(IriString::new(value.to_owned()).map(|_| ())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(action: AuditAction, second: i64, actor: &str, subject: Option<&str>) -> AuditEvent {
        AuditEvent {
            iri: format!("urn:uuid:{second}"),
            object_iri: "https://example.org/design".to_owned(),
            action,
            actor_graph: actor.to_owned(),
            subject_graph: subject.map(str::to_owned),
            note: None,
            occurred_at: DateTime::from_timestamp(second, 0).expect("timestamp"),
        }
    }

    #[test]
    fn latest_cycle_is_derived_without_rewriting_history() {
        let owner = "https://example.org/user/owner";
        let curator = "https://example.org/user/curator";
        let events = vec![
            event(AuditAction::ReviewRequested, 1, owner, Some(curator)),
            event(AuditAction::ReviewApproved, 2, curator, Some(curator)),
            event(AuditAction::ReviewRequested, 3, owner, Some(curator)),
        ];
        let case = latest_case("https://example.org/design", events).expect("case");
        assert_eq!(case.status, ReviewStatus::Pending);
        assert_eq!(case.events.len(), 1);
        assert_eq!(case.requested_by_graph, owner);
        assert_eq!(case.curator_graph, curator);
    }
}
