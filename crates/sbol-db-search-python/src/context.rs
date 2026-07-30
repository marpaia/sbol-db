use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use sbol_db_search_sdk::{
    EmbeddingBatch, EmbeddingInput, EmbeddingInputKind, EmbeddingProvider, EmbeddingVector,
    ScopedDocumentHydrator, ScopedVectorSearch, SearchBudget, SearchScope, VectorFilter,
    VectorQuery, VectorValue,
};
use serde_json::{json, Value};
use tokio::runtime::Handle;

use crate::conversion::{
    json_to_python, python_to_json, search_error_to_python, vector_error_to_python,
};

/// Python view of the exact request-scoped services given to a Rust strategy.
/// Vector and document handles are already authorization scoped by sbol-db.
#[pyclass(name = "SearchContext", module = "sbol_db.search")]
pub(crate) struct PythonSearchContext {
    pub(crate) scope: SearchScope,
    pub(crate) budget: SearchBudget,
    pub(crate) vectors: Arc<dyn ScopedVectorSearch>,
    pub(crate) documents: Arc<dyn ScopedDocumentHydrator>,
    pub(crate) embedding: Arc<dyn EmbeddingProvider>,
    pub(crate) default_index: String,
    pub(crate) default_vector_name: String,
    pub(crate) runtime: Handle,
}

#[pymethods]
impl PythonSearchContext {
    #[getter]
    fn scope(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let value = match &self.scope {
            SearchScope::Union => json!({"kind": "union"}),
            SearchScope::Only(graphs) => json!({"kind": "only", "graphs": graphs}),
        };
        json_to_python(py, &value)
    }

    #[getter]
    fn budget(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_python(
            py,
            &json!({
                "timeout_ms": self.budget.timeout_ms,
                "max_candidates": self.budget.max_candidates,
                "max_tool_calls": self.budget.max_tool_calls,
            }),
        )
    }

    #[getter]
    fn vectors(&self, py: Python<'_>) -> PyResult<Py<PythonVectorSearch>> {
        Py::new(
            py,
            PythonVectorSearch {
                inner: self.vectors.clone(),
                default_index: self.default_index.clone(),
                default_vector_name: self.default_vector_name.clone(),
                max_candidates: self.budget.max_candidates,
                runtime: self.runtime.clone(),
            },
        )
    }

    #[getter]
    fn documents(&self, py: Python<'_>) -> PyResult<Py<PythonDocumentHydrator>> {
        Py::new(
            py,
            PythonDocumentHydrator {
                inner: self.documents.clone(),
                runtime: self.runtime.clone(),
            },
        )
    }

    /// Embed one or more strings with the profile declared by this strategy.
    #[pyo3(signature = (texts, /, *, kind="query"))]
    fn embed(
        &self,
        py: Python<'_>,
        texts: &Bound<'_, PyAny>,
        kind: &str,
    ) -> PyResult<Vec<Vec<f32>>> {
        let texts = if let Ok(text) = texts.extract::<String>() {
            vec![text]
        } else {
            texts.extract::<Vec<String>>()?
        };
        let kind = match kind {
            "query" => EmbeddingInputKind::Query,
            "document" => EmbeddingInputKind::Document,
            _ => {
                return Err(PyValueError::new_err(
                    "embedding kind must be 'query' or 'document'",
                ));
            }
        };
        let embedding = self.embedding.clone();
        let profile = embedding.descriptor().id.clone();
        let runtime = self.runtime.clone();
        let output = py.allow_threads(move || {
            runtime.block_on(
                embedding.embed(EmbeddingBatch {
                    profile,
                    inputs: texts
                        .into_iter()
                        .map(|text| EmbeddingInput { kind, text })
                        .collect(),
                }),
            )
        });
        output
            .map_err(search_error_to_python)?
            .vectors
            .into_iter()
            .map(|vector| match vector {
                EmbeddingVector::Dense(vector) => Ok(vector),
                _ => Err(PyTypeError::new_err(
                    "this Python context currently requires dense embeddings",
                )),
            })
            .collect()
    }
}

#[pyclass(name = "VectorSearch", module = "sbol_db.search")]
struct PythonVectorSearch {
    inner: Arc<dyn ScopedVectorSearch>,
    default_index: String,
    default_vector_name: String,
    max_candidates: usize,
    runtime: Handle,
}

#[pymethods]
impl PythonVectorSearch {
    #[pyo3(signature = (vector, /, *, index=None, vector_name=None, filter=None, limit=50, cursor=None, score_threshold=None, parameters=None))]
    #[allow(clippy::too_many_arguments)]
    fn query(
        &self,
        py: Python<'_>,
        vector: &Bound<'_, PyAny>,
        index: Option<String>,
        vector_name: Option<String>,
        filter: Option<&Bound<'_, PyAny>>,
        limit: usize,
        cursor: Option<String>,
        score_threshold: Option<f32>,
        parameters: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        if limit == 0 || limit > self.max_candidates {
            return Err(PyValueError::new_err(format!(
                "vector query limit must be between 1 and {}",
                self.max_candidates
            )));
        }
        let index = index.unwrap_or_else(|| self.default_index.clone());
        if index != self.default_index {
            return Err(PyValueError::new_err(format!(
                "strategy declared vector index {:?}; querying {index:?} is not permitted",
                self.default_index
            )));
        }
        let vector_name = vector_name.unwrap_or_else(|| self.default_vector_name.clone());
        if vector_name != self.default_vector_name {
            return Err(PyValueError::new_err(format!(
                "strategy declared vector name {:?}; querying {vector_name:?} is not permitted",
                self.default_vector_name
            )));
        }
        let vector = parse_vector_value(python_to_json(py, vector)?)?;
        let filter = filter
            .map(|value| {
                serde_json::from_value::<VectorFilter>(python_to_json(py, value)?).map_err(
                    |error| PyValueError::new_err(format!("invalid vector filter: {error}")),
                )
            })
            .transpose()?;
        let parameters = parameters
            .map(|value| {
                serde_json::from_value::<BTreeMap<String, Value>>(python_to_json(py, value)?)
                    .map_err(|error| {
                        PyValueError::new_err(format!("invalid vector parameters: {error}"))
                    })
            })
            .transpose()?
            .unwrap_or_default();
        let query = VectorQuery {
            index,
            vector_name,
            vector,
            filter,
            limit,
            cursor,
            score_threshold,
            parameters,
        };
        let inner = self.inner.clone();
        let runtime = self.runtime.clone();
        let page = py
            .allow_threads(move || runtime.block_on(inner.query(query)))
            .map_err(vector_error_to_python)?;
        let page = serde_json::to_value(page)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_to_python(py, &page)
    }
}

#[pyclass(name = "DocumentHydrator", module = "sbol_db.search")]
struct PythonDocumentHydrator {
    inner: Arc<dyn ScopedDocumentHydrator>,
    runtime: Handle,
}

#[pymethods]
impl PythonDocumentHydrator {
    fn hydrate(&self, py: Python<'_>, document_ids: Vec<String>) -> PyResult<Py<PyAny>> {
        let document_ids = document_ids
            .into_iter()
            .map(sbol_db_search_sdk::DocumentId)
            .collect();
        let inner = self.inner.clone();
        let runtime = self.runtime.clone();
        let documents = py
            .allow_threads(move || runtime.block_on(inner.hydrate(document_ids)))
            .map_err(search_error_to_python)?;
        let documents = serde_json::to_value(documents)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_to_python(py, &documents)
    }
}

fn parse_vector_value(value: Value) -> PyResult<VectorValue> {
    if let Value::Array(values) = &value {
        if values.iter().all(Value::is_number) {
            return serde_json::from_value::<Vec<f32>>(value)
                .map(VectorValue::Dense)
                .map_err(|error| PyValueError::new_err(format!("invalid dense vector: {error}")));
        }
        if values.iter().all(Value::is_array) {
            return serde_json::from_value::<Vec<Vec<f32>>>(value)
                .map(VectorValue::MultiDense)
                .map_err(|error| {
                    PyValueError::new_err(format!("invalid multi-dense vector: {error}"))
                });
        }
    }
    serde_json::from_value::<VectorValue>(value).map_err(|error| {
        PyValueError::new_err(format!(
            "vector must be a dense list, multi-dense list, or SDK vector mapping: {error}"
        ))
    })
}
