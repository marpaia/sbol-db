//! Typed server runtime configuration and managed production data layout.
//!
//! Resolution, generation ownership, recovery, and private filesystem mechanics
//! are separated so the production invariants remain explicit and reviewable.

mod configuration;
mod filesystem;
mod layout;

pub use configuration::{resolve_connection, ServerRuntime};
#[allow(unused_imports)]
pub use layout::{
    ManagedDataLayout, RecoveryEvent, RecoveryStatus, RestoreJournalStatus, RestoreOutcome,
    RollbackOutcome,
};

#[cfg(test)]
pub(crate) use configuration::DEFAULT_DATABASE_URL;

#[cfg(test)]
mod tests;
