//! Typed SBOL projection records. Domain-shaped; persistence converts these
//! into rows in `sbol_components`, `sbol_sequences`, etc.

use serde::{Deserialize, Serialize};

use crate::iri::IriString;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentProjection {
    pub iri: IriString,
    pub types: Vec<String>,
    pub roles: Vec<String>,
    pub sequence_iris: Vec<String>,
    pub feature_iris: Vec<String>,
    pub interaction_iris: Vec<String>,
    pub model_iris: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SequenceAlphabet {
    Dna,
    Rna,
    Protein,
    Smiles,
    Other,
}

impl SequenceAlphabet {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Dna => "DNA",
            Self::Rna => "RNA",
            Self::Protein => "PROTEIN",
            Self::Smiles => "SMILES",
            Self::Other => "OTHER",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceProjection {
    pub iri: IriString,
    pub encoding_iri: Option<IriString>,
    pub elements: Option<String>,
    pub alphabet: Option<SequenceAlphabet>,
    pub content_hash: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureProjection {
    pub iri: IriString,
    pub parent_component_iri: Option<IriString>,
    /// `SubComponent`, `LocalSubComponent`, `ExternallyDefined`, `SequenceFeature`, or `ComponentReference`.
    pub feature_kind: String,
    pub instance_of_iri: Option<IriString>,
    pub roles: Vec<String>,
    pub orientation_iri: Option<IriString>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationProjection {
    pub iri: IriString,
    pub feature_iri: Option<IriString>,
    pub sequence_iri: Option<IriString>,
    pub location_kind: String,
    pub start_pos: Option<i32>,
    pub end_pos: Option<i32>,
    pub cut_pos: Option<i32>,
    pub orientation_iri: Option<IriString>,
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintProjection {
    pub iri: IriString,
    pub parent_component_iri: Option<IriString>,
    pub restriction_iri: Option<IriString>,
    pub subject_iri: Option<IriString>,
    pub object_iri: Option<IriString>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionProjection {
    pub iri: IriString,
    pub parent_component_iri: Option<IriString>,
    pub interaction_types: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipationProjection {
    pub iri: IriString,
    pub interaction_iri: Option<IriString>,
    pub participant_iri: Option<IriString>,
    pub roles: Vec<String>,
}

/// Aggregate of all typed projections extracted from a single document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedProjections {
    pub components: Vec<ComponentProjection>,
    pub sequences: Vec<SequenceProjection>,
    pub features: Vec<FeatureProjection>,
    pub locations: Vec<LocationProjection>,
    pub constraints: Vec<ConstraintProjection>,
    pub interactions: Vec<InteractionProjection>,
    pub participations: Vec<ParticipationProjection>,
}
