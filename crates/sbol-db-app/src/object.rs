//! ACL-scoped, backend-neutral object understanding.
//!
//! The raw storage record is useful for debugging, but it is not a product
//! contract: it omits inverse relationships, exposes physical graph ids, and
//! leaves every RDF predicate for an HTTP adapter or browser to interpret.
//! [`AppServices::object_details`] assembles the stable object-page projection
//! once in the application layer so V2, compatibility adapters, and future
//! clients can share the same biological and authorization semantics.

use std::collections::{BTreeMap, BTreeSet};

use sbol_db_core::{DomainError, IriString, ObjectTerm, SbolObjectRecord, Triple};
use sbol_db_rdf::GRAPH_IRI_PREFIX;
use sbol_db_sparql::{GraphScope, ResultFormat, SparqlOptions};
use serde::Serialize;

use crate::{AppServices, PUBLIC_GRAPH};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";
const DCTERMS_DESCRIPTION: &str = "http://purl.org/dc/terms/description";
const DCTERMS_CREATED: &str = "http://purl.org/dc/terms/created";
const DCTERMS_MODIFIED: &str = "http://purl.org/dc/terms/modified";
const DCTERMS_CREATOR: &str = "http://purl.org/dc/terms/creator";
const DC_CREATOR: &str = "http://purl.org/dc/elements/1.1/creator";
const PROV_DERIVED_FROM: &str = "http://www.w3.org/ns/prov#wasDerivedFrom";
const PROV_GENERATED_BY: &str = "http://www.w3.org/ns/prov#wasGeneratedBy";

const SBH_OWNED_BY: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy";
const SBH_CAN_VIEW: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#canView";
const SBH_TOP_LEVEL: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel";
const SBH_MUTABLE_PROVENANCE: &str =
    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableProvenance";
const SBH_ATTACHMENT: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachment";
const SBH_ATTACHMENT_HASH: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentHash";
const SBH_ATTACHMENT_SIZE: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentSize";
const SBH_ATTACHMENT_TYPE: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentType";

const OBI_CITATION: &str = "http://purl.obolibrary.org/obo/OBI_0001617";

const SBOL2: &str = "http://sbols.org/v2#";
const SBOL3: &str = "http://sbols.org/v3#";
const SBOL2_DISPLAY_ID: &str = "http://sbols.org/v2#displayId";
const SBOL3_DISPLAY_ID: &str = "http://sbols.org/v3#displayId";
const SBOL2_PERSISTENT_IDENTITY: &str = "http://sbols.org/v2#persistentIdentity";
const SBOL2_VERSION: &str = "http://sbols.org/v2#version";
const SBOL3_NAME: &str = "http://sbols.org/v3#name";
const SBOL3_DESCRIPTION: &str = "http://sbols.org/v3#description";
const SBOL2_ROLE: &str = "http://sbols.org/v2#role";
const SBOL3_ROLE: &str = "http://sbols.org/v3#role";
const SBOL2_SEQUENCE: &str = "http://sbols.org/v2#sequence";
const SBOL3_SEQUENCE: &str = "http://sbols.org/v3#hasSequence";
const SBOL2_ELEMENTS: &str = "http://sbols.org/v2#elements";
const SBOL3_ELEMENTS: &str = "http://sbols.org/v3#elements";
const SBOL2_ENCODING: &str = "http://sbols.org/v2#encoding";
const SBOL3_ENCODING: &str = "http://sbols.org/v3#encoding";
const SBOL2_SEQUENCE_ANNOTATION: &str = "http://sbols.org/v2#sequenceAnnotation";
const SBOL2_COMPONENT: &str = "http://sbols.org/v2#component";
const SBOL3_FEATURE: &str = "http://sbols.org/v3#hasFeature";
const SBOL2_LOCATION: &str = "http://sbols.org/v2#location";
const SBOL3_LOCATION: &str = "http://sbols.org/v3#hasLocation";
const SBOL2_START: &str = "http://sbols.org/v2#start";
const SBOL3_START: &str = "http://sbols.org/v3#start";
const SBOL2_END: &str = "http://sbols.org/v2#end";
const SBOL3_END: &str = "http://sbols.org/v3#end";
const SBOL2_ORIENTATION: &str = "http://sbols.org/v2#orientation";
const SBOL3_ORIENTATION: &str = "http://sbols.org/v3#orientation";
const SBOL2_INTERACTION: &str = "http://sbols.org/v2#interaction";
const SBOL3_INTERACTION: &str = "http://sbols.org/v3#hasInteraction";
const SBOL2_MEMBER: &str = "http://sbols.org/v2#member";
const SBOL3_MEMBER: &str = "http://sbols.org/v3#member";
const SBOL2_ATTACHMENT: &str = "http://sbols.org/v2#attachment";
const SBOL3_ATTACHMENT: &str = "http://sbols.org/v3#hasAttachment";
const SBOL2_SOURCE: &str = "http://sbols.org/v2#source";
const SBOL3_SOURCE: &str = "http://sbols.org/v3#source";
const SBOL2_HASH: &str = "http://sbols.org/v2#hash";
const SBOL3_HASH: &str = "http://sbols.org/v3#hash";
const SBOL2_SIZE: &str = "http://sbols.org/v2#size";
const SBOL3_SIZE: &str = "http://sbols.org/v3#size";
const SBOL2_FORMAT: &str = "http://sbols.org/v2#format";
const SBOL3_FORMAT: &str = "http://sbols.org/v3#format";

/// Whether an object-page section has complete content to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectContentState {
    Available,
    Empty,
    Partial,
    Unsupported,
}

/// The visibility consequence of the graph holding the selected projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectVisibility {
    Public,
    Restricted,
}

/// A stable linked-resource identity. Optional presentation fields are
/// intentionally additive; an IRI is sufficient to preserve the relationship.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectReference {
    pub uri: String,
    pub display_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub object_type: Option<String>,
}

/// A relationship collection with an explicit absence/support state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectReferenceSection {
    pub state: ObjectContentState,
    pub items: Vec<ObjectReference>,
    pub note: Option<String>,
}

/// One normalized RDF property and every persisted value for that predicate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectProperty {
    pub predicate: String,
    pub values: Vec<ObjectPropertyValue>,
}

/// A lossless, JSON-friendly RDF object term.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectPropertyValue {
    Resource {
        value: String,
    },
    BlankNode {
        value: String,
    },
    Literal {
        value: String,
        datatype: String,
        language: Option<String>,
    },
}

/// Provenance values kept distinct from identity and ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectProvenance {
    pub creators: Vec<String>,
    pub derived_from: Vec<String>,
    pub generated_by: Vec<String>,
    pub mutable_source: Vec<String>,
    pub citations: Vec<String>,
}

/// The sequence payload when the current object is itself an SBOL Sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectSequenceContent {
    pub state: ObjectContentState,
    pub elements: Option<String>,
    pub encoding: Option<String>,
    pub length: Option<usize>,
    pub note: Option<String>,
}

/// Attachment metadata accepted from SBOL2, SBOL3, and legacy SynBioHub
/// predicates. `resolved=false` retains a relationship whose target metadata
/// is incomplete instead of silently omitting it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectAttachment {
    pub uri: String,
    pub name: Option<String>,
    pub hash: Option<String>,
    pub size: Option<u64>,
    pub format: Option<String>,
    pub source: Option<String>,
    pub resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectAttachmentSection {
    pub state: ObjectContentState,
    pub items: Vec<ObjectAttachment>,
    pub note: Option<String>,
}

/// The small, deterministic SBOL Visual glyph vocabulary the native object
/// page renders directly. Every other role uses SBOL Visual's unspecified
/// feature glyph rather than guessing biological meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectVisualGlyph {
    Promoter,
    CodingSequence,
    RibosomeEntrySite,
    Terminator,
    Operator,
    OriginOfReplication,
    Unspecified,
}

/// One addressable feature and the coordinate interval used by the design
/// overview. Coordinates remain optional so partial SBOL documents are visible
/// without fabricating position or orientation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectVisualFeature {
    pub uri: String,
    pub label: String,
    pub roles: Vec<String>,
    pub glyph: ObjectVisualGlyph,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub orientation: Option<String>,
}

/// Coordinate-aware, metadata-first SBOL Visual projection. `available` means
/// every asserted addressable feature resolved to a valid range; `partial`
/// retains unresolved features; `empty` is an asserted Component without
/// features; and `unsupported` means visualization does not apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectVisualization {
    pub state: ObjectContentState,
    pub sequence_length: Option<usize>,
    pub features: Vec<ObjectVisualFeature>,
    pub note: Option<String>,
}

/// The normalized object-page resource returned by the application layer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectDetails {
    pub iri: String,
    pub persistent_identity: Option<String>,
    pub display_id: Option<String>,
    pub version: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub object_type: String,
    pub types: Vec<String>,
    pub roles: Vec<String>,
    pub source_graph: Option<String>,
    pub visibility: ObjectVisibility,
    pub owners: Vec<String>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub provenance: ObjectProvenance,
    pub sequence_content: ObjectSequenceContent,
    pub sequences: ObjectReferenceSection,
    pub features: ObjectReferenceSection,
    pub visualization: ObjectVisualization,
    pub interactions: ObjectReferenceSection,
    pub collections: ObjectReferenceSection,
    pub members: ObjectReferenceSection,
    pub attachments: ObjectAttachmentSection,
    pub uses: ObjectReferenceSection,
    pub twins: ObjectReferenceSection,
    pub properties: Vec<ObjectProperty>,
    pub content_fingerprint: Option<String>,
}

struct ScopedSubject {
    triples: Vec<Triple>,
    physical_graph: Option<String>,
    source_graph: Option<String>,
    query_scope: GraphScope,
    record: Option<SbolObjectRecord>,
}

impl AppServices {
    /// Resolve the effective graph scope for reading `iri`, or `None` when the
    /// object is absent or outside the caller's authorization ceiling.
    ///
    /// A subject can legitimately occur in more than one physical graph (for
    /// example, an imported source graph plus the public projection), and a
    /// logical document graph can authorize its physical storage graph. Reuse
    /// the same resolution as [`Self::object_details`] so downloads and JSON
    /// object reads do not collapse that topology to an arbitrary first graph.
    pub async fn object_read_scope(
        &self,
        iri: &str,
        scope: GraphScope,
    ) -> Result<Option<GraphScope>, DomainError> {
        IriString::new(iri.to_owned())?;
        Ok(self
            .scoped_subject(iri, scope)
            .await?
            .map(|subject| subject.query_scope))
    }

    /// Resolve one object under the caller's authorized graph scope and build
    /// its normalized object-understanding projection. Returns `None` for both
    /// an unknown and an out-of-scope IRI, preserving non-disclosure.
    pub async fn object_details(
        &self,
        iri: &str,
        scope: GraphScope,
    ) -> Result<Option<ObjectDetails>, DomainError> {
        IriString::new(iri.to_owned())?;
        let Some(scoped) = self.scoped_subject(iri, scope).await? else {
            return Ok(None);
        };
        let triples = &scoped.triples;

        let mut types = iri_values(triples, &[RDF_TYPE]);
        let record_matches = scoped.record.as_ref().is_some_and(|record| {
            record.graph_id.map(|id| format!("{GRAPH_IRI_PREFIX}{id}")) == scoped.physical_graph
        });
        if types.is_empty() && record_matches {
            if let Some(record) = &scoped.record {
                types.push(record.sbol_class.clone());
            }
        }
        types.sort();
        types.dedup();
        let object_type = scoped
            .record
            .as_ref()
            .filter(|_| record_matches)
            .map(|record| record.sbol_class.clone())
            .or_else(|| primary_type(&types))
            .unwrap_or_else(|| "http://www.w3.org/2000/01/rdf-schema#Resource".to_owned());

        let (sequence_iris, sequence_partial) =
            referenced_iris(triples, &[SBOL2_SEQUENCE, SBOL3_SEQUENCE]);
        let (feature_iris, feature_partial) = referenced_iris(
            triples,
            &[SBOL2_SEQUENCE_ANNOTATION, SBOL2_COMPONENT, SBOL3_FEATURE],
        );
        let (interaction_iris, interaction_partial) =
            referenced_iris(triples, &[SBOL2_INTERACTION, SBOL3_INTERACTION]);
        let (member_iris, member_partial) = referenced_iris(triples, &[SBOL2_MEMBER, SBOL3_MEMBER]);

        let target = sparql_iri(iri)?;
        let collection_query = format!(
            "SELECT DISTINCT ?subject WHERE {{ \
             {{ ?subject <{SBOL2_MEMBER}> {target} . }} UNION \
             {{ ?subject <{SBOL3_MEMBER}> {target} . }} }} ORDER BY ?subject"
        );
        let uses_query = format!(
            "SELECT DISTINCT ?subject WHERE {{ \
             ?subject a ?type . \
             {{ ?subject ?predicate {target} . }} UNION \
             {{ ?subject ?predicate ?middle . ?middle ?middlePredicate {target} . \
                FILTER(?middlePredicate != <{SBH_TOP_LEVEL}>) }} \
             FILTER(?subject != {target}) }} ORDER BY ?subject"
        );
        let twins_query = format!(
            "SELECT DISTINCT ?subject WHERE {{ \
             VALUES ?targetSequencePredicate {{ <{SBOL2_SEQUENCE}> <{SBOL3_SEQUENCE}> }} \
             VALUES ?sequencePredicate {{ <{SBOL2_SEQUENCE}> <{SBOL3_SEQUENCE}> }} \
             VALUES ?targetElementsPredicate {{ <{SBOL2_ELEMENTS}> <{SBOL3_ELEMENTS}> }} \
             VALUES ?elementsPredicate {{ <{SBOL2_ELEMENTS}> <{SBOL3_ELEMENTS}> }} \
             {target} ?targetSequencePredicate ?targetSequence . \
             ?targetSequence ?targetElementsPredicate ?elements . \
             ?subject ?sequencePredicate ?sequence . \
             ?sequence ?elementsPredicate ?elements . \
             ?subject a ?type . FILTER(?subject != {target}) \
             }} ORDER BY ?subject"
        );

        let collection_iris = self
            .object_relation_iris(&collection_query, scoped.query_scope.clone())
            .await?;
        let use_iris = self
            .object_relation_iris(&uses_query, scoped.query_scope.clone())
            .await?;
        let twin_iris = self
            .object_relation_iris(&twins_query, scoped.query_scope.clone())
            .await?;

        let component_like = is_component_like(&object_type);
        let collection_like = object_type.ends_with("#Collection");
        let sequence_like = object_type.ends_with("#Sequence");

        let visualization = self
            .object_visualization(
                component_like,
                &feature_iris,
                feature_partial,
                &sequence_iris,
                scoped.physical_graph.as_deref(),
            )
            .await?;

        let attachments = self
            .object_attachments(triples, scoped.physical_graph.as_deref())
            .await?;
        let unresolved_attachments = attachments.iter().any(|item| !item.resolved);

        let display_id =
            first_literal(triples, &[SBOL2_DISPLAY_ID, SBOL3_DISPLAY_ID]).or_else(|| {
                scoped
                    .record
                    .as_ref()
                    .filter(|_| record_matches)
                    .and_then(|record| record.display_id.clone())
            });
        let name = first_literal(triples, &[DCTERMS_TITLE, SBOL3_NAME]).or_else(|| {
            scoped
                .record
                .as_ref()
                .filter(|_| record_matches)
                .and_then(|record| record.name.clone())
        });
        let description = first_literal(triples, &[DCTERMS_DESCRIPTION, SBOL3_DESCRIPTION])
            .or_else(|| {
                scoped
                    .record
                    .as_ref()
                    .filter(|_| record_matches)
                    .and_then(|record| record.description.clone())
            });

        Ok(Some(ObjectDetails {
            iri: iri.to_owned(),
            persistent_identity: first_iri(triples, &[SBOL2_PERSISTENT_IDENTITY]),
            display_id,
            version: first_literal(triples, &[SBOL2_VERSION]),
            name,
            description,
            object_type,
            types,
            roles: iri_values(triples, &[SBOL2_ROLE, SBOL3_ROLE]),
            source_graph: scoped.source_graph.clone(),
            visibility: if scoped.source_graph.as_deref() == Some(PUBLIC_GRAPH) {
                ObjectVisibility::Public
            } else {
                ObjectVisibility::Restricted
            },
            owners: iri_values(triples, &[SBH_OWNED_BY]),
            created_at: first_literal(triples, &[DCTERMS_CREATED]),
            modified_at: last_literal(triples, &[DCTERMS_MODIFIED]),
            provenance: ObjectProvenance {
                creators: term_values(triples, &[DC_CREATOR, DCTERMS_CREATOR]),
                derived_from: term_values(triples, &[PROV_DERIVED_FROM]),
                generated_by: term_values(triples, &[PROV_GENERATED_BY]),
                mutable_source: term_values(triples, &[SBH_MUTABLE_PROVENANCE]),
                citations: term_values(triples, &[OBI_CITATION]),
            },
            sequence_content: sequence_content(triples, sequence_like),
            sequences: reference_section(
                sequence_iris,
                component_like,
                sequence_partial,
                "Sequence relationships apply to Component designs.",
            ),
            features: reference_section(
                feature_iris,
                component_like,
                feature_partial,
                "Feature structure applies to Component designs.",
            ),
            visualization,
            interactions: reference_section(
                interaction_iris,
                component_like,
                interaction_partial,
                "Interaction structure applies to Component designs.",
            ),
            collections: reference_section(collection_iris, true, false, ""),
            members: reference_section(
                member_iris,
                collection_like,
                member_partial,
                "Member relationships apply to Collection objects.",
            ),
            attachments: ObjectAttachmentSection {
                state: if attachments.is_empty() {
                    ObjectContentState::Empty
                } else if unresolved_attachments {
                    ObjectContentState::Partial
                } else {
                    ObjectContentState::Available
                },
                note: unresolved_attachments.then(|| {
                    "One or more attachment targets lack readable attachment metadata.".to_owned()
                }),
                items: attachments,
            },
            uses: reference_section(use_iris, true, false, ""),
            twins: reference_section(
                twin_iris,
                component_like && has_asserted_sequence(triples),
                false,
                "Exact-sequence twins require a Component with an asserted sequence.",
            ),
            properties: object_properties(triples),
            content_fingerprint: scoped
                .record
                .as_ref()
                .filter(|_| record_matches)
                .map(|record| hex::encode(&record.content_hash)),
        }))
    }

    /// Resolve the exact objects explicitly shared with an account. The share
    /// index is read from the ACL store, while every returned projection still
    /// passes through the ordinary authorized object-details boundary.
    pub async fn shared_object_details(
        &self,
        user_graph: &str,
    ) -> Result<Vec<ObjectDetails>, DomainError> {
        let scope = self.acl_service.compute_scope(Some(user_graph)).await?;
        let mut iris = self.acl.viewable_objects(user_graph).await?;
        iris.sort();
        iris.dedup();
        let mut items = Vec::with_capacity(iris.len());
        for iri in iris {
            if let Some(details) = self.object_details(&iri, scope.clone()).await? {
                items.push(details);
            }
        }
        items.sort_by(|left, right| left.iri.cmp(&right.iri));
        Ok(items)
    }

    /// User graph IRIs carrying an explicit read-only share for `object_iri`.
    /// Authorization for exposing this collaborator list remains the caller's
    /// responsibility; this application query only normalizes the persisted
    /// ACL relation.
    pub async fn object_viewer_graphs(&self, object_iri: &str) -> Result<Vec<String>, DomainError> {
        let target = sparql_iri(object_iri)?;
        let query = format!(
            "SELECT DISTINCT ?subject WHERE {{ ?subject <{SBH_CAN_VIEW}> {target} }} ORDER BY ?subject"
        );
        self.object_relation_iris(&query, GraphScope::Union).await
    }

    async fn scoped_subject(
        &self,
        iri: &str,
        scope: GraphScope,
    ) -> Result<Option<ScopedSubject>, DomainError> {
        let all = self.store.triples_for_subject(iri).await?;
        if all.is_empty() {
            return Ok(None);
        }
        let record = self.store.get_object_by_iri(iri).await?;
        let mut query_scope = scope.clone();
        let mut logical_graph = None;
        let mut physical_record_graph = None;

        if let Some(graph_id) = record.as_ref().and_then(|record| record.graph_id) {
            let physical = format!("{GRAPH_IRI_PREFIX}{graph_id}");
            physical_record_graph = Some(physical.clone());
            if let Some(graph) = self.store.get_graph(graph_id).await? {
                logical_graph = graph.document_iri.map(|value| value.into_inner());
            }
            if let GraphScope::Only(graphs) = &mut query_scope {
                let logical_allowed = logical_graph
                    .as_ref()
                    .is_some_and(|graph| graphs.iter().any(|allowed| allowed == graph));
                let physical_allowed = graphs.iter().any(|allowed| allowed == &physical);
                if logical_allowed && !physical_allowed {
                    graphs.push(physical);
                    graphs.sort();
                    graphs.dedup();
                }
            }
        }

        let mut candidates = all
            .iter()
            .filter_map(|triple| {
                triple
                    .graph_iri
                    .as_ref()
                    .map(|graph| graph.as_str().to_owned())
            })
            .filter(|graph| graph_allowed(graph, &query_scope))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| graph_priority(left).cmp(&graph_priority(right)));
        candidates.dedup();

        let physical_graph = candidates.first().cloned();
        let has_graphless = all.iter().any(|triple| triple.graph_iri.is_none());
        if physical_graph.is_none() && !(matches!(query_scope, GraphScope::Union) && has_graphless)
        {
            return Ok(None);
        }

        let triples = all
            .into_iter()
            .filter(|triple| {
                triple.graph_iri.as_ref().map(|graph| graph.as_str()) == physical_graph.as_deref()
            })
            .collect::<Vec<_>>();
        if triples.is_empty() {
            return Ok(None);
        }
        let source_graph = if physical_graph == physical_record_graph && logical_graph.is_some() {
            logical_graph
        } else {
            physical_graph.clone()
        };

        Ok(Some(ScopedSubject {
            triples,
            physical_graph,
            source_graph,
            query_scope,
            record,
        }))
    }

    async fn object_relation_iris(
        &self,
        query: &str,
        scope: GraphScope,
    ) -> Result<Vec<String>, DomainError> {
        let options = SparqlOptions {
            authorized_graphs: scope,
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
                "object relationship result exceeded its 10000-row safety bound".to_owned(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&outcome.payload.body)?;
        let mut iris = value
            .get("results")
            .and_then(|results| results.get("bindings"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|binding| {
                binding
                    .get("subject")
                    .and_then(|subject| subject.get("value"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        iris.sort();
        iris.dedup();
        Ok(iris)
    }

    async fn object_visualization(
        &self,
        component_like: bool,
        feature_iris: &[String],
        relationship_partial: bool,
        sequence_iris: &[String],
        physical_graph: Option<&str>,
    ) -> Result<ObjectVisualization, DomainError> {
        if !component_like {
            return Ok(ObjectVisualization {
                state: ObjectContentState::Unsupported,
                sequence_length: None,
                features: Vec::new(),
                note: Some(
                    "SBOL Visual applies to Component and ComponentDefinition designs.".to_owned(),
                ),
            });
        }

        let mut sequence_length: Option<usize> = None;
        for sequence_iri in sequence_iris {
            let sequence = self.triples_in_graph(sequence_iri, physical_graph).await?;
            if let Some(elements) = first_literal(&sequence, &[SBOL2_ELEMENTS, SBOL3_ELEMENTS]) {
                sequence_length = Some(
                    sequence_length
                        .unwrap_or_default()
                        .max(elements.chars().count()),
                );
            }
        }

        let mut partial = relationship_partial;
        let mut features = Vec::with_capacity(feature_iris.len());
        for feature_iri in feature_iris {
            let feature = self.triples_in_graph(feature_iri, physical_graph).await?;
            if feature.is_empty() {
                partial = true;
                features.push(ObjectVisualFeature {
                    uri: feature_iri.clone(),
                    label: iri_tail(feature_iri),
                    roles: Vec::new(),
                    glyph: ObjectVisualGlyph::Unspecified,
                    start: None,
                    end: None,
                    orientation: None,
                });
                continue;
            }

            let roles = iri_values(&feature, &[SBOL2_ROLE, SBOL3_ROLE]);
            let (locations, locations_partial) =
                referenced_iris(&feature, &[SBOL2_LOCATION, SBOL3_LOCATION]);
            partial |= locations_partial || locations.len() != 1;
            let mut start = None;
            let mut end = None;
            let mut orientation = None;
            if let Some(location_iri) = locations.first() {
                let location = self.triples_in_graph(location_iri, physical_graph).await?;
                start = first_literal(&location, &[SBOL2_START, SBOL3_START])
                    .and_then(|value| value.parse::<usize>().ok());
                end = first_literal(&location, &[SBOL2_END, SBOL3_END])
                    .and_then(|value| value.parse::<usize>().ok());
                orientation = first_term(&location, &[SBOL2_ORIENTATION, SBOL3_ORIENTATION]);
            }
            if !matches!((start, end), (Some(start), Some(end)) if start > 0 && start <= end) {
                partial = true;
            }
            if let Some(end) = end {
                sequence_length = Some(sequence_length.unwrap_or_default().max(end));
            }
            let label = first_literal(
                &feature,
                &[
                    DCTERMS_TITLE,
                    SBOL3_NAME,
                    SBOL2_DISPLAY_ID,
                    SBOL3_DISPLAY_ID,
                ],
            )
            .unwrap_or_else(|| iri_tail(feature_iri));
            features.push(ObjectVisualFeature {
                uri: feature_iri.clone(),
                label,
                glyph: visual_glyph(&roles),
                roles,
                start,
                end,
                orientation,
            });
        }

        features.sort_by(|left, right| {
            left.start
                .unwrap_or(usize::MAX)
                .cmp(&right.start.unwrap_or(usize::MAX))
                .then_with(|| left.end.cmp(&right.end))
                .then_with(|| left.uri.cmp(&right.uri))
        });
        let state = if partial {
            ObjectContentState::Partial
        } else if features.is_empty() {
            ObjectContentState::Empty
        } else {
            ObjectContentState::Available
        };
        let note = match state {
            ObjectContentState::Available => None,
            ObjectContentState::Empty => {
                Some("No addressable feature structure is asserted.".to_owned())
            }
            ObjectContentState::Partial => Some(
                "Only features with valid, addressable ranges are placed; unresolved structure remains listed below."
                    .to_owned(),
            ),
            ObjectContentState::Unsupported => unreachable!(),
        };
        Ok(ObjectVisualization {
            state,
            sequence_length,
            features,
            note,
        })
    }

    async fn triples_in_graph(
        &self,
        subject: &str,
        physical_graph: Option<&str>,
    ) -> Result<Vec<Triple>, DomainError> {
        Ok(self
            .store
            .triples_for_subject(subject)
            .await?
            .into_iter()
            .filter(|triple| {
                triple.graph_iri.as_ref().map(|graph| graph.as_str()) == physical_graph
            })
            .collect())
    }

    async fn object_attachments(
        &self,
        parent: &[Triple],
        physical_graph: Option<&str>,
    ) -> Result<Vec<ObjectAttachment>, DomainError> {
        let (uris, _) = referenced_iris(
            parent,
            &[SBOL2_ATTACHMENT, SBOL3_ATTACHMENT, SBH_ATTACHMENT],
        );
        let mut items = Vec::with_capacity(uris.len());
        for uri in uris {
            let triples = self.triples_in_graph(&uri, physical_graph).await?;
            let name = first_literal(
                &triples,
                &[DCTERMS_TITLE, SBOL2_DISPLAY_ID, SBOL3_DISPLAY_ID],
            );
            let hash = first_literal(&triples, &[SBOL2_HASH, SBOL3_HASH, SBH_ATTACHMENT_HASH]);
            let size = first_literal(&triples, &[SBOL2_SIZE, SBOL3_SIZE, SBH_ATTACHMENT_SIZE])
                .and_then(|value| value.parse::<u64>().ok());
            let format = first_term(&triples, &[SBOL2_FORMAT, SBOL3_FORMAT, SBH_ATTACHMENT_TYPE]);
            let source = first_term(&triples, &[SBOL2_SOURCE, SBOL3_SOURCE]);
            let resolved = name.is_some()
                || hash.is_some()
                || size.is_some()
                || format.is_some()
                || source.is_some();
            items.push(ObjectAttachment {
                uri,
                name,
                hash,
                size,
                format,
                source,
                resolved,
            });
        }
        Ok(items)
    }
}

fn graph_allowed(graph: &str, scope: &GraphScope) -> bool {
    match scope {
        GraphScope::Union => true,
        GraphScope::Only(graphs) => graphs.iter().any(|allowed| allowed == graph),
    }
}

fn graph_priority(graph: &str) -> (u8, &str) {
    let priority = if graph == PUBLIC_GRAPH {
        0
    } else if graph.starts_with(GRAPH_IRI_PREFIX) {
        2
    } else {
        1
    };
    (priority, graph)
}

fn sparql_iri(value: &str) -> Result<String, DomainError> {
    let value = IriString::new(value.to_owned())?;
    Ok(format!("<{}>", value.as_str()))
}

fn primary_type(types: &[String]) -> Option<String> {
    types
        .iter()
        .find(|value| value.starts_with(SBOL3))
        .or_else(|| types.iter().find(|value| value.starts_with(SBOL2)))
        .or_else(|| types.first())
        .cloned()
}

fn is_component_like(object_type: &str) -> bool {
    object_type.ends_with("#Component") || object_type.ends_with("#ComponentDefinition")
}

fn visual_glyph(roles: &[String]) -> ObjectVisualGlyph {
    if roles.iter().any(|role| role.ends_with("SO:0000167")) {
        ObjectVisualGlyph::Promoter
    } else if roles.iter().any(|role| role.ends_with("SO:0000316")) {
        ObjectVisualGlyph::CodingSequence
    } else if roles.iter().any(|role| role.ends_with("SO:0000139")) {
        ObjectVisualGlyph::RibosomeEntrySite
    } else if roles.iter().any(|role| role.ends_with("SO:0000141")) {
        ObjectVisualGlyph::Terminator
    } else if roles.iter().any(|role| role.ends_with("SO:0000057")) {
        ObjectVisualGlyph::Operator
    } else if roles.iter().any(|role| role.ends_with("SO:0000296")) {
        ObjectVisualGlyph::OriginOfReplication
    } else {
        ObjectVisualGlyph::Unspecified
    }
}

fn iri_tail(iri: &str) -> String {
    iri.rsplit(['#', '/'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(iri)
        .to_owned()
}

fn has_asserted_sequence(triples: &[Triple]) -> bool {
    triples.iter().any(|triple| {
        matches!(triple.predicate.as_str(), SBOL2_SEQUENCE | SBOL3_SEQUENCE)
            && matches!(triple.object, ObjectTerm::Iri(_))
    })
}

fn sequence_content(triples: &[Triple], supported: bool) -> ObjectSequenceContent {
    if !supported {
        return ObjectSequenceContent {
            state: ObjectContentState::Unsupported,
            elements: None,
            encoding: None,
            length: None,
            note: Some(
                "Sequence elements apply when the current object is an SBOL Sequence.".to_owned(),
            ),
        };
    }
    let elements = first_literal(triples, &[SBOL2_ELEMENTS, SBOL3_ELEMENTS]);
    let encoding = first_term(triples, &[SBOL2_ENCODING, SBOL3_ENCODING]);
    let length = elements.as_ref().map(|value| value.chars().count());
    let state = match (&elements, &encoding) {
        (None, _) => ObjectContentState::Empty,
        (Some(_), None) => ObjectContentState::Partial,
        (Some(_), Some(_)) => ObjectContentState::Available,
    };
    let note = (state == ObjectContentState::Partial)
        .then(|| "Sequence elements are present, but no encoding is asserted.".to_owned());
    ObjectSequenceContent {
        state,
        elements,
        encoding,
        length,
        note,
    }
}

fn reference_section(
    iris: Vec<String>,
    supported: bool,
    partial: bool,
    unsupported_note: &str,
) -> ObjectReferenceSection {
    if !supported {
        return ObjectReferenceSection {
            state: ObjectContentState::Unsupported,
            items: Vec::new(),
            note: Some(unsupported_note.to_owned()),
        };
    }
    let items = iris
        .into_iter()
        .map(|uri| ObjectReference {
            uri,
            display_id: None,
            name: None,
            description: None,
            object_type: None,
        })
        .collect::<Vec<_>>();
    let state = if partial {
        ObjectContentState::Partial
    } else if items.is_empty() {
        ObjectContentState::Empty
    } else {
        ObjectContentState::Available
    };
    ObjectReferenceSection {
        state,
        items,
        note: partial.then(|| {
            "One or more blank-node relationships cannot be addressed as object pages.".to_owned()
        }),
    }
}

fn object_properties(triples: &[Triple]) -> Vec<ObjectProperty> {
    let mut properties: BTreeMap<String, BTreeSet<ObjectPropertyValue>> = BTreeMap::new();
    for triple in triples {
        properties
            .entry(triple.predicate.as_str().to_owned())
            .or_default()
            .insert(property_value(&triple.object));
    }
    properties
        .into_iter()
        .map(|(predicate, values)| ObjectProperty {
            predicate,
            values: values.into_iter().collect(),
        })
        .collect()
}

fn property_value(value: &ObjectTerm) -> ObjectPropertyValue {
    match value {
        ObjectTerm::Iri(value) => ObjectPropertyValue::Resource {
            value: value.as_str().to_owned(),
        },
        ObjectTerm::BlankNode(value) => ObjectPropertyValue::BlankNode {
            value: value.clone(),
        },
        ObjectTerm::Literal {
            value,
            datatype,
            language,
        } => ObjectPropertyValue::Literal {
            value: value.clone(),
            datatype: datatype.as_str().to_owned(),
            language: language.clone(),
        },
    }
}

fn referenced_iris(triples: &[Triple], predicates: &[&str]) -> (Vec<String>, bool) {
    let mut values = BTreeSet::new();
    let mut partial = false;
    for triple in triples
        .iter()
        .filter(|triple| predicates.contains(&triple.predicate.as_str()))
    {
        match &triple.object {
            ObjectTerm::Iri(value) => {
                values.insert(value.as_str().to_owned());
            }
            ObjectTerm::BlankNode(_) => partial = true,
            ObjectTerm::Literal { .. } => partial = true,
        }
    }
    (values.into_iter().collect(), partial)
}

fn iri_values(triples: &[Triple], predicates: &[&str]) -> Vec<String> {
    let mut values = triples
        .iter()
        .filter(|triple| predicates.contains(&triple.predicate.as_str()))
        .filter_map(|triple| match &triple.object {
            ObjectTerm::Iri(value) => Some(value.as_str().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn literal_values(triples: &[Triple], predicates: &[&str]) -> Vec<String> {
    let mut values = triples
        .iter()
        .filter(|triple| predicates.contains(&triple.predicate.as_str()))
        .filter_map(|triple| match &triple.object {
            ObjectTerm::Literal { value, .. } => Some(value.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn term_values(triples: &[Triple], predicates: &[&str]) -> Vec<String> {
    let mut values = triples
        .iter()
        .filter(|triple| predicates.contains(&triple.predicate.as_str()))
        .filter_map(|triple| match &triple.object {
            ObjectTerm::Iri(value) => Some(value.as_str().to_owned()),
            ObjectTerm::Literal { value, .. } => Some(value.clone()),
            ObjectTerm::BlankNode(_) => None,
        })
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn first_iri(triples: &[Triple], predicates: &[&str]) -> Option<String> {
    iri_values(triples, predicates).into_iter().next()
}

fn first_literal(triples: &[Triple], predicates: &[&str]) -> Option<String> {
    literal_values(triples, predicates).into_iter().next()
}

fn last_literal(triples: &[Triple], predicates: &[&str]) -> Option<String> {
    literal_values(triples, predicates).into_iter().next_back()
}

fn first_term(triples: &[Triple], predicates: &[&str]) -> Option<String> {
    term_values(triples, predicates).into_iter().next()
}
