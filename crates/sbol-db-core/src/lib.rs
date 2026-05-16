//! Domain types shared across the sbol-db crates. No I/O dependencies live
//! here.

mod error;
mod ids;
mod iri;
pub mod kmer;
mod neighborhood;
pub mod obo;
mod projections;
pub mod quad;
mod record;
mod validation;

pub use error::DomainError;
pub use ids::{DocumentId, ObjectId, ValidationRunId};
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
pub use quad::{ObjectTerm, Quad, SubjectTerm};
pub use record::{
    DocumentRecord, ImportReport, NewDocument, ObjectSummary, SbolObjectRecord, SerializationFormat,
};
pub use validation::{Severity, ValidationFinding, ValidationStatus};
