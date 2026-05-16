//! Built-in job handlers that ship with sbol-db.
//!
//! Each module here is one [`crate::JobHandler`] implementation that is
//! registered into the default [`crate::JobRegistry`] used by
//! `sbol-db serve` and `sbol-db worker run`. Library consumers building
//! a custom registry can pick whichever subset they need.

pub mod import_document;

pub use import_document::{ImportDocumentHandler, ImportDocumentPayload};
