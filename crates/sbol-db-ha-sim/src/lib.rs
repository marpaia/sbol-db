#![allow(clippy::result_large_err)] // OpenRaft fixes its RPC error shape.

pub mod cluster;
pub mod corpus;
pub mod network;
pub mod scenario;

pub use corpus::{load_corpus, Corpus, CorpusDocument, CorpusManifest};
pub use scenario::{
    run_corpus_chaos, ScenarioConfig, SimulationReport, TraceEvent, TraceEventKind,
};
