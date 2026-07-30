//! Runtime bridge for declaring native sbol-db search plugins in Python.

mod context;
mod conversion;
mod embedding;
mod plugin;
mod strategy;

pub use plugin::{load_plugin, PythonSearchPlugin, PythonSearchPluginConfig};
pub use strategy::PythonStrategyRegistration;

#[cfg(test)]
mod tests;
