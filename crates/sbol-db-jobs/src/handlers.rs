//! Built-in job handlers that ship with sbol-db.
//!
//! Each module here is one [`crate::JobHandler`] implementation that is
//! registered into the default [`crate::JobRegistry`] used by
//! `sbol-db serve` and `sbol-db worker run`. Library consumers building
//! a custom registry can pick whichever subset they need.

pub mod complete_backup;
pub mod import_document;
pub mod import_remote_document;
pub mod rebuild_search_index;
pub mod rebuild_vector_index;
pub mod update_vector_index;
pub mod wor_sync;

pub use complete_backup::{
    BackupTrigger, CompleteBackupHandler, CompleteBackupPayload, COMPLETE_BACKUP_JOB_KIND,
};
pub use import_document::{ImportDocumentHandler, ImportDocumentPayload};
pub use import_remote_document::{ImportRemoteDocumentHandler, ImportRemoteDocumentPayload};
pub use rebuild_search_index::RebuildSearchIndexHandler;
pub use rebuild_vector_index::RebuildVectorIndexHandler;
pub use update_vector_index::{
    MaintainVectorIndexHandler, MaintainVectorIndexPayload, UpdateVectorIndexHandler,
    UpdateVectorIndexPayload,
};
pub use wor_sync::{WorSyncHandler, WorSyncPayload};
