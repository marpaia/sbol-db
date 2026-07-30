use std::path::PathBuf;
use std::sync::Arc;

use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule};
use sbol_db_search::EmbeddingStrategyConfig;
use sbol_db_search_sdk::{
    DataEgress, DistanceMetric, EmbeddingDescriptor, EmbeddingProvider, Normalization, SearchError,
};
use serde::{Deserialize, Serialize};

use crate::conversion::python_configuration;
use crate::embedding::PythonEmbeddingProvider;
use crate::strategy::PythonStrategyRegistration;

/// One Python module loaded into the process-level search composition root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonSearchPluginConfig {
    pub module: String,
    #[serde(default = "default_register_function")]
    pub register: String,
    /// Optional directory prepended to `sys.path` before importing `module`.
    /// Installed packages should omit this; it is useful for local plugins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

fn default_register_function() -> String {
    "register".to_owned()
}

/// Native registrations produced by one Python module.
pub struct PythonSearchPlugin {
    pub embeddings: Vec<Arc<dyn EmbeddingProvider>>,
    pub embedding_strategies: Vec<EmbeddingStrategyConfig>,
    pub strategies: Vec<PythonStrategyRegistration>,
}

struct EmbeddingRegistration {
    implementation: Py<PyAny>,
    descriptor: EmbeddingDescriptor,
}

#[derive(Default)]
#[pyclass(name = "SearchPlugin", module = "sbol_db.search")]
struct RegistrationSink {
    embeddings: Vec<EmbeddingRegistration>,
    embedding_strategies: Vec<EmbeddingStrategyConfig>,
    strategies: Vec<PythonStrategyRegistration>,
}

#[pymethods]
impl RegistrationSink {
    /// Register a Python object with `embed(texts, *, kind)` as a native
    /// `EmbeddingProvider`.
    #[pyo3(signature = (implementation, /, **kwargs))]
    fn add_embedding(
        &mut self,
        implementation: Py<PyAny>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let kwargs = required_kwargs(kwargs)?;
        let embed = implementation.bind(kwargs.py()).getattr("embed");
        if !matches!(embed, Ok(method) if method.is_callable()) {
            return Err(PyTypeError::new_err(
                "embedding implementation must define embed(texts, *, kind)",
            ));
        }
        let descriptor = EmbeddingDescriptor {
            id: required_string(kwargs, "id")?,
            provider: string_or(kwargs, "provider", "python")?,
            model: required_string(kwargs, "model")?,
            revision: required_string(kwargs, "revision")?,
            dimension: required_usize(kwargs, "dimension")?,
            normalization: parse_normalization(&string_or(kwargs, "normalization", "l2")?)?,
            data_egress: parse_data_egress(&string_or(kwargs, "data_egress", "none")?)?,
        };
        validate_embedding_descriptor(&descriptor)?;
        reject_unknown(
            kwargs,
            &[
                "id",
                "provider",
                "model",
                "revision",
                "dimension",
                "normalization",
                "data_egress",
            ],
        )?;
        self.embeddings.push(EmbeddingRegistration {
            implementation,
            descriptor,
        });
        Ok(())
    }

    /// Register a Python object with `search(ctx, request)`, or omit the object
    /// to configure an instance of sbol-db's native dense strategy. In both
    /// forms, the embedding is resolved by profile ID at startup rather than
    /// being passed into a Python constructor.
    #[pyo3(signature = (implementation=None, /, **kwargs))]
    fn add_strategy(
        &mut self,
        implementation: Option<Py<PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let kwargs = required_kwargs(kwargs)?;
        let id = required_string(kwargs, "id")?;
        let strategy = EmbeddingStrategyConfig {
            id: id.clone(),
            version: string_or(kwargs, "version", "1")?,
            display_name: optional_string_or(kwargs, "display_name", &id)?,
            description: string_or(kwargs, "description", "Python embedding search")?,
            embedding_profile: required_string(kwargs, "embedding_profile")?,
            vector_index: required_string(kwargs, "vector_index")?,
            vector_name: string_or(kwargs, "vector_name", "content")?,
            graph_payload_field: string_or(kwargs, "graph_payload_field", "graph")?,
            distance: parse_distance(&string_or(kwargs, "distance", "cosine")?)?,
        };
        reject_unknown(
            kwargs,
            &[
                "id",
                "version",
                "display_name",
                "description",
                "embedding_profile",
                "vector_index",
                "vector_name",
                "graph_payload_field",
                "distance",
            ],
        )?;
        if let Some(implementation) = implementation {
            let search = implementation.bind(kwargs.py()).getattr("search");
            if !matches!(search, Ok(method) if method.is_callable()) {
                return Err(PyTypeError::new_err(
                    "strategy implementation must define search(ctx, request)",
                ));
            }
            self.strategies
                .push(PythonStrategyRegistration::new(implementation, strategy));
        } else {
            self.embedding_strategies.push(strategy);
        }
        Ok(())
    }
}

/// Load a Python module, call its registration function, and turn the declared
/// objects into native SDK plugins. Import and model-construction failures are
/// startup configuration errors rather than request-time surprises.
pub fn load_plugin(config: &PythonSearchPluginConfig) -> Result<PythonSearchPlugin, SearchError> {
    validate_plugin_config(config)?;
    Python::with_gil(|py| {
        if let Some(path) = &config.path {
            let sys = PyModule::import(py, "sys").map_err(python_configuration)?;
            let paths = sys
                .getattr("path")
                .and_then(|value| value.downcast_into::<PyList>().map_err(Into::into))
                .map_err(python_configuration)?;
            paths
                .insert(0, path.to_string_lossy().as_ref())
                .map_err(python_configuration)?;
        }

        let module = PyModule::import(py, &config.module).map_err(python_configuration)?;
        let register = module
            .getattr(&config.register)
            .map_err(python_configuration)?;
        if !register.is_callable() {
            return Err(SearchError::Configuration(format!(
                "Python search plugin {:?} attribute {:?} is not callable",
                config.module, config.register
            )));
        }
        let sink = Py::new(py, RegistrationSink::default()).map_err(python_configuration)?;
        register
            .call1((sink.clone_ref(py),))
            .map_err(python_configuration)?;

        let mut sink = sink.borrow_mut(py);
        let embeddings = std::mem::take(&mut sink.embeddings)
            .into_iter()
            .map(|registration| {
                Arc::new(PythonEmbeddingProvider::new(
                    registration.implementation,
                    registration.descriptor,
                )) as Arc<dyn EmbeddingProvider>
            })
            .collect();
        Ok(PythonSearchPlugin {
            embeddings,
            embedding_strategies: std::mem::take(&mut sink.embedding_strategies),
            strategies: std::mem::take(&mut sink.strategies),
        })
    })
}

fn required_kwargs<'py>(
    kwargs: Option<&'py Bound<'py, PyDict>>,
) -> PyResult<&'py Bound<'py, PyDict>> {
    kwargs.ok_or_else(|| PyTypeError::new_err("keyword arguments are required"))
}

fn required_string(kwargs: &Bound<'_, PyDict>, name: &str) -> PyResult<String> {
    let value = kwargs
        .get_item(name)?
        .ok_or_else(|| PyKeyError::new_err(name.to_owned()))?;
    let value = value.extract::<String>()?;
    if value.trim().is_empty() {
        return Err(PyValueError::new_err(format!("{name} cannot be empty")));
    }
    Ok(value)
}

fn string_or(kwargs: &Bound<'_, PyDict>, name: &str, default: &str) -> PyResult<String> {
    match kwargs.get_item(name)? {
        Some(value) => {
            let value = value.extract::<String>()?;
            if value.trim().is_empty() {
                Err(PyValueError::new_err(format!("{name} cannot be empty")))
            } else {
                Ok(value)
            }
        }
        None => Ok(default.to_owned()),
    }
}

fn optional_string_or(kwargs: &Bound<'_, PyDict>, name: &str, default: &str) -> PyResult<String> {
    match kwargs.get_item(name)? {
        Some(value) if !value.is_none() => {
            let value = value.extract::<String>()?;
            if value.trim().is_empty() {
                Err(PyValueError::new_err(format!("{name} cannot be empty")))
            } else {
                Ok(value)
            }
        }
        _ => Ok(default.to_owned()),
    }
}

fn required_usize(kwargs: &Bound<'_, PyDict>, name: &str) -> PyResult<usize> {
    kwargs
        .get_item(name)?
        .ok_or_else(|| PyKeyError::new_err(name.to_owned()))?
        .extract::<usize>()
}

fn reject_unknown(kwargs: &Bound<'_, PyDict>, allowed: &[&str]) -> PyResult<()> {
    for (key, _) in kwargs.iter() {
        let key = key.extract::<String>()?;
        if !allowed.contains(&key.as_str()) {
            return Err(PyTypeError::new_err(format!(
                "unexpected keyword argument {key:?}"
            )));
        }
    }
    Ok(())
}

fn parse_normalization(value: &str) -> PyResult<Normalization> {
    match value {
        "none" => Ok(Normalization::None),
        "l2" => Ok(Normalization::L2),
        _ => Err(PyValueError::new_err(
            "normalization must be 'none' or 'l2'",
        )),
    }
}

fn parse_data_egress(value: &str) -> PyResult<DataEgress> {
    match value {
        "none" => Ok(DataEgress::None),
        "configured_remote" => Ok(DataEgress::ConfiguredRemote),
        _ => Err(PyValueError::new_err(
            "data_egress must be 'none' or 'configured_remote'",
        )),
    }
}

fn parse_distance(value: &str) -> PyResult<DistanceMetric> {
    match value {
        "cosine" => Ok(DistanceMetric::Cosine),
        "dot" => Ok(DistanceMetric::Dot),
        "euclidean" => Ok(DistanceMetric::Euclidean),
        "manhattan" => Ok(DistanceMetric::Manhattan),
        "hamming" => Ok(DistanceMetric::Hamming),
        "jaccard" => Ok(DistanceMetric::Jaccard),
        _ => Err(PyValueError::new_err(format!(
            "unsupported distance {value:?}"
        ))),
    }
}

fn validate_embedding_descriptor(descriptor: &EmbeddingDescriptor) -> PyResult<()> {
    if descriptor.dimension == 0 {
        return Err(PyValueError::new_err("dimension must be greater than zero"));
    }
    for (name, value) in [
        ("id", &descriptor.id),
        ("provider", &descriptor.provider),
        ("model", &descriptor.model),
        ("revision", &descriptor.revision),
    ] {
        if value.trim().is_empty() {
            return Err(PyValueError::new_err(format!("{name} cannot be empty")));
        }
    }
    Ok(())
}

fn validate_plugin_config(config: &PythonSearchPluginConfig) -> Result<(), SearchError> {
    if config.module.trim().is_empty() || config.register.trim().is_empty() {
        return Err(SearchError::Configuration(
            "Python search plugin module and register function cannot be empty".to_owned(),
        ));
    }
    Ok(())
}
