//! Native public-edge TLS and ACME lifecycle support.
//!
//! Listener policy, renewal polling, handshake admission, durable cache state,
//! and private filesystem mechanics are split into focused review boundaries.

mod acceptor;
mod cache;
mod configuration;
mod lifecycle;

#[allow(unused_imports)]
pub use acceptor::TimeoutAcceptor;
#[allow(unused_imports)]
pub use cache::CertificateState;
#[allow(unused_imports)]
pub use configuration::AcmeTlsConfig;
pub use configuration::EdgeHttpConfig;
pub use lifecycle::run_acme;

#[cfg(test)]
mod tests;
