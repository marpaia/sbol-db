use pyo3::exceptions::{PyNotImplementedError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyModule;
use sbol_db_search_sdk::{SearchError, VectorError};
use serde_json::Value;

pub(crate) fn json_to_python(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    let serialized =
        serde_json::to_string(value).map_err(|error| PyValueError::new_err(error.to_string()))?;
    PyModule::import(py, "json")?
        .call_method1("loads", (serialized,))
        .map(Bound::unbind)
}

pub(crate) fn python_to_json(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Value> {
    let serialized = PyModule::import(py, "json")?
        .call_method1("dumps", (value,))?
        .extract::<String>()?;
    serde_json::from_str(&serialized).map_err(|error| PyValueError::new_err(error.to_string()))
}

pub(crate) fn search_error_to_python(error: SearchError) -> PyErr {
    match error {
        SearchError::InvalidRequest(message) => PyValueError::new_err(message),
        SearchError::Unsupported(message) => PyNotImplementedError::new_err(message),
        error => PyRuntimeError::new_err(error.to_string()),
    }
}

pub(crate) fn vector_error_to_python(error: VectorError) -> PyErr {
    match error {
        VectorError::InvalidRequest(message) => PyValueError::new_err(message),
        VectorError::Unsupported(message) => PyNotImplementedError::new_err(message),
        error => PyRuntimeError::new_err(error.to_string()),
    }
}

pub(crate) fn python_configuration(error: PyErr) -> SearchError {
    SearchError::Configuration(format!("Python search plugin failed: {error}"))
}

pub(crate) fn python_execution(error: PyErr) -> SearchError {
    SearchError::Backend(format!("Python search plugin execution failed: {error}"))
}

pub(crate) fn python_embedding(error: PyErr) -> SearchError {
    SearchError::Backend(format!("Python embedding provider failed: {error}"))
}
