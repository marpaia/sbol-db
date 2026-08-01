//! Domain types shared across the sbol-db crates. No I/O dependencies live
//! here.

mod blob;
mod config;
mod error;
mod identity;
mod ids;
mod iri;
pub mod kmer;
mod neighborhood;
mod oauth;
pub mod obo;
mod prepared_mutation;
mod projections;
mod record;
pub mod triple;
mod validation;

pub use blob::BlobRef;
pub use config::ConfigEntry;
pub use error::DomainError;
pub use identity::{ApiToken, NewUser, User};
pub use ids::{GraphId, JobId, ObjectId, UserId, ValidationRunId};
pub use iri::{IriString, IriValidationError};
pub use neighborhood::{
    group_by_depth, Direction, EdgeInfo, EdgeObject, NeighborhoodQuery, NeighborhoodResult,
    NodeInfo,
};
pub use oauth::{OAuthAccessToken, OAuthAuthorizationCode, OAuthClient, OAuthRefreshToken};
pub use prepared_mutation::PreparedMutation;
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
