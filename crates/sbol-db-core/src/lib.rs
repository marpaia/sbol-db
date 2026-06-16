//! Domain types shared across the sbol-db crates. No I/O dependencies live
//! here.

mod error;
mod ids;
mod iri;
pub mod kmer;
mod neighborhood;
pub mod obo;
mod projections;
mod record;
pub mod triple;
mod validation;

pub use error::DomainError;
pub use ids::{GraphId, JobId, ObjectId, ValidationRunId};
pub use iri::{IriString, IriValidationError};
pub use neighborhood::{
    group_by_depth, Direction, EdgeInfo, EdgeObject, NeighborhoodQuery, NeighborhoodResult,
    NodeInfo,
};
pub use projections::{
    ComponentProjection, ConstraintProjection, FeatureProjection, InteractionProjection,
    LocationProjection, ParticipationProjection, SequenceAlphabet, SequenceProjection,
    TypedProjections,
};
pub use record::{
    GraphRecord, ImportReport, NewGraph, ObjectSummary, SbolObjectRecord, SerializationFormat,
};
pub use triple::{ObjectTerm, SubjectTerm, Triple};
pub use validation::{Severity, ValidationFinding, ValidationStatus};
