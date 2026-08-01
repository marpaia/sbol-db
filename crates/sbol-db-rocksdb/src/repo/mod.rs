//! Per-surface repositories over the shared RocksDB handle.

pub mod accel;
pub mod cluster;
pub mod config;
pub mod graph;
pub mod job;
pub mod lab;
pub mod neighborhood;
pub mod oauth;
pub mod object;
pub mod ontology;
pub mod pagerank;
pub mod prepared_mutation;
pub mod sequence_search;
pub mod sketch;
pub mod tokens;
pub mod triple;
pub mod users;

pub use cluster::RocksdbClusterStore;
pub use config::RocksdbConfigStore;
pub use graph::GraphRepository;
pub use job::JobRepository;
pub use lab::LabRepository;
pub use oauth::RocksdbOAuthStore;
pub use object::ObjectRepository;
pub use ontology::OntologyRepository;
pub use pagerank::RocksdbPageRankStore;
pub use prepared_mutation::RocksdbPreparedMutationStore;
pub use sequence_search::SequenceSearchRepository;
pub use sketch::RocksdbSketchStore;
pub use tokens::RocksdbTokenStore;
pub use triple::TripleRepository;
pub use users::RocksdbUserStore;
