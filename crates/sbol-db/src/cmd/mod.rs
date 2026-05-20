//! Per-noun handler modules. Each module owns one branch of the
//! top-level `Cli::Command` enum (or, for `server` / `worker`, the
//! daemon entry points) and exposes a `run` function invoked from
//! `main.rs`.

pub mod db;
pub mod doc;
pub mod inspect;
pub mod jobs;
pub mod object;
pub mod ontology;
pub mod query;
pub mod server;
pub mod util;
pub mod worker;
