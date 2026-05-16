//! Extract typed SBOL projections (Components, Sequences, Features, Locations,
//! Constraints, Interactions, Participations) from a parsed `sbol::Document`.

use sbol::{Document, Resource, SbolObject};
use sbol_db_core::{
    ComponentProjection, ConstraintProjection, FeatureProjection, InteractionProjection, IriString,
    LocationProjection, ParticipationProjection, SequenceAlphabet, SequenceProjection,
    TypedProjections,
};

use crate::hash::hash_bytes;

/// IUPAC DNA/RNA encoding IRI per EDAM, matched by `sbol-rs` vocabulary.
const EDAM_IUPAC_DNA_RNA: &str = "https://identifiers.org/edam:format_1207";
const EDAM_IUPAC_PROTEIN: &str = "https://identifiers.org/edam:format_1208";
const EDAM_SMILES: &str = "https://identifiers.org/edam:format_1196";

/// Walk every typed object in `doc` and build per-table projection records.
/// Objects whose identity is a blank node are skipped — they cannot satisfy
/// the `sbol_iri` Postgres domain.
pub fn document_to_projections(doc: &Document) -> TypedProjections {
    let mut out = TypedProjections::default();

    for obj in doc.typed_objects() {
        let identity_iri = match obj.identity().as_iri() {
            Some(iri) => iri,
            None => continue,
        };
        let iri = IriString::unchecked(identity_iri.as_str());
        let parent = obj.parent_identity().and_then(|p| match p {
            Resource::Iri(iri) => Some(IriString::unchecked(iri.as_str())),
            _ => None,
        });

        match obj {
            SbolObject::Component(c) => {
                out.components.push(ComponentProjection {
                    iri,
                    types: iris_to_strings(&c.types),
                    roles: iris_to_strings(&c.roles),
                    sequence_iris: resources_to_strings(&c.sequences),
                    feature_iris: resources_to_strings(&c.features),
                    interaction_iris: resources_to_strings(&c.interactions),
                    model_iris: resources_to_strings(&c.models),
                });
            }
            SbolObject::Sequence(s) => {
                let encoding_iri = s
                    .encoding
                    .as_ref()
                    .map(|i| IriString::unchecked(i.as_str()));
                let alphabet = s.encoding.as_ref().and_then(alphabet_from_encoding);
                let content_hash = s.elements.as_ref().map(|e| hash_bytes(e.as_bytes()));
                out.sequences.push(SequenceProjection {
                    iri,
                    encoding_iri,
                    elements: s.elements.clone(),
                    alphabet,
                    content_hash,
                });
            }
            SbolObject::SubComponent(f) => {
                out.features.push(FeatureProjection {
                    iri,
                    parent_component_iri: parent,
                    feature_kind: "SubComponent".to_owned(),
                    instance_of_iri: resource_iri(&f.instance_of),
                    roles: iris_to_strings(&f.feature.roles),
                    orientation_iri: option_iri(&f.feature.orientation),
                });
            }
            SbolObject::LocalSubComponent(f) => {
                out.features.push(FeatureProjection {
                    iri,
                    parent_component_iri: parent,
                    feature_kind: "LocalSubComponent".to_owned(),
                    instance_of_iri: None,
                    roles: iris_to_strings(&f.feature.roles),
                    orientation_iri: option_iri(&f.feature.orientation),
                });
            }
            SbolObject::ExternallyDefined(f) => {
                out.features.push(FeatureProjection {
                    iri,
                    parent_component_iri: parent,
                    feature_kind: "ExternallyDefined".to_owned(),
                    instance_of_iri: resource_iri(&f.definition),
                    roles: iris_to_strings(&f.feature.roles),
                    orientation_iri: option_iri(&f.feature.orientation),
                });
            }
            SbolObject::SequenceFeature(f) => {
                out.features.push(FeatureProjection {
                    iri,
                    parent_component_iri: parent,
                    feature_kind: "SequenceFeature".to_owned(),
                    instance_of_iri: None,
                    roles: iris_to_strings(&f.feature.roles),
                    orientation_iri: option_iri(&f.feature.orientation),
                });
            }
            SbolObject::ComponentReference(f) => {
                out.features.push(FeatureProjection {
                    iri,
                    parent_component_iri: parent,
                    feature_kind: "ComponentReference".to_owned(),
                    instance_of_iri: resource_iri(&f.refers_to),
                    roles: iris_to_strings(&f.feature.roles),
                    orientation_iri: option_iri(&f.feature.orientation),
                });
            }
            SbolObject::Range(loc) => {
                out.locations.push(LocationProjection {
                    iri,
                    feature_iri: parent,
                    sequence_iri: resource_iri(&loc.location.sequence),
                    location_kind: "Range".to_owned(),
                    start_pos: loc.start.and_then(i64_to_i32),
                    end_pos: loc.end.and_then(i64_to_i32),
                    cut_pos: None,
                    orientation_iri: option_iri(&loc.location.orientation),
                    data: serde_json::json!({
                        "order": loc.location.order,
                    }),
                });
            }
            SbolObject::Cut(loc) => {
                out.locations.push(LocationProjection {
                    iri,
                    feature_iri: parent,
                    sequence_iri: resource_iri(&loc.location.sequence),
                    location_kind: "Cut".to_owned(),
                    start_pos: None,
                    end_pos: None,
                    cut_pos: loc.at.and_then(i64_to_i32),
                    orientation_iri: option_iri(&loc.location.orientation),
                    data: serde_json::json!({
                        "order": loc.location.order,
                    }),
                });
            }
            SbolObject::EntireSequence(loc) => {
                out.locations.push(LocationProjection {
                    iri,
                    feature_iri: parent,
                    sequence_iri: resource_iri(&loc.location.sequence),
                    location_kind: "EntireSequence".to_owned(),
                    start_pos: None,
                    end_pos: None,
                    cut_pos: None,
                    orientation_iri: option_iri(&loc.location.orientation),
                    data: serde_json::json!({
                        "order": loc.location.order,
                    }),
                });
            }
            SbolObject::Constraint(c) => {
                out.constraints.push(ConstraintProjection {
                    iri,
                    parent_component_iri: parent,
                    restriction_iri: option_iri(&c.restriction),
                    subject_iri: resource_iri(&c.subject),
                    object_iri: resource_iri(&c.constrained_object),
                });
            }
            SbolObject::Interaction(i) => {
                out.interactions.push(InteractionProjection {
                    iri,
                    parent_component_iri: parent,
                    interaction_types: iris_to_strings(&i.types),
                });
            }
            SbolObject::Participation(p) => {
                out.participations.push(ParticipationProjection {
                    iri,
                    interaction_iri: parent,
                    participant_iri: resource_iri(&p.participant),
                    roles: iris_to_strings(&p.roles),
                });
            }
            _ => {}
        }
    }

    out
}

fn alphabet_from_encoding(iri: &sbol::Iri) -> Option<SequenceAlphabet> {
    match iri.as_str() {
        EDAM_IUPAC_DNA_RNA => Some(SequenceAlphabet::Dna),
        EDAM_IUPAC_PROTEIN => Some(SequenceAlphabet::Protein),
        EDAM_SMILES => Some(SequenceAlphabet::Smiles),
        _ => Some(SequenceAlphabet::Other),
    }
}

fn iris_to_strings(iris: &[sbol::Iri]) -> Vec<String> {
    iris.iter().map(|i| i.as_str().to_owned()).collect()
}

fn resources_to_strings(resources: &[sbol::Resource]) -> Vec<String> {
    resources
        .iter()
        .filter_map(|r| match r {
            sbol::Resource::Iri(iri) => Some(iri.as_str().to_owned()),
            _ => None,
        })
        .collect()
}

fn resource_iri(resource: &Option<sbol::Resource>) -> Option<IriString> {
    resource.as_ref().and_then(|r| match r {
        sbol::Resource::Iri(iri) => Some(IriString::unchecked(iri.as_str())),
        _ => None,
    })
}

fn option_iri(iri: &Option<sbol::Iri>) -> Option<IriString> {
    iri.as_ref().map(|i| IriString::unchecked(i.as_str()))
}

fn i64_to_i32(value: i64) -> Option<i32> {
    i32::try_from(value).ok()
}
