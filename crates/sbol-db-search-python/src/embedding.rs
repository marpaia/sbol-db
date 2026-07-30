use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use sbol_db_search_sdk::{
    EmbeddingBatch, EmbeddingDescriptor, EmbeddingOutput, EmbeddingProvider, EmbeddingVector,
    SearchError,
};

use crate::conversion::python_embedding;

pub(crate) struct PythonEmbeddingProvider {
    implementation: Py<PyAny>,
    descriptor: EmbeddingDescriptor,
}

impl PythonEmbeddingProvider {
    pub(crate) fn new(implementation: Py<PyAny>, descriptor: EmbeddingDescriptor) -> Self {
        Self {
            implementation,
            descriptor,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for PythonEmbeddingProvider {
    fn descriptor(&self) -> &EmbeddingDescriptor {
        &self.descriptor
    }

    async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError> {
        if batch.profile != self.descriptor.id {
            return Err(SearchError::Configuration(format!(
                "embedding batch requested profile {:?}, but Python provider is {:?}",
                batch.profile, self.descriptor.id
            )));
        }
        let implementation = Python::with_gil(|py| self.implementation.clone_ref(py));
        let descriptor = self.descriptor.clone();
        tokio::task::spawn_blocking(move || {
            Python::with_gil(|py| {
                let texts = batch
                    .inputs
                    .iter()
                    .map(|input| input.text.as_str())
                    .collect::<Vec<_>>();
                let kinds = batch
                    .inputs
                    .iter()
                    .map(|input| match input.kind {
                        sbol_db_search_sdk::EmbeddingInputKind::Query => "query",
                        sbol_db_search_sdk::EmbeddingInputKind::Document => "document",
                    })
                    .collect::<Vec<_>>();
                let kind = kinds.first().copied().unwrap_or("document");
                if kinds.iter().any(|candidate| *candidate != kind) {
                    return Err(SearchError::InvalidRequest(
                        "Python embedding batches must contain only queries or only documents"
                            .to_owned(),
                    ));
                }
                let kwargs = PyDict::new(py);
                kwargs.set_item("kind", kind).map_err(python_embedding)?;
                let output = implementation
                    .bind(py)
                    .call_method("embed", (texts,), Some(&kwargs))
                    .map_err(python_embedding)?;
                let vectors: Vec<Vec<f32>> = output.extract().map_err(python_embedding)?;
                validate_vectors(&vectors, batch.inputs.len(), &descriptor)?;
                Ok(EmbeddingOutput {
                    vectors: vectors.into_iter().map(EmbeddingVector::Dense).collect(),
                })
            })
        })
        .await
        .map_err(|error| SearchError::Backend(format!("joining Python embedding task: {error}")))?
    }
}

fn validate_vectors(
    vectors: &[Vec<f32>],
    expected_count: usize,
    descriptor: &EmbeddingDescriptor,
) -> Result<(), SearchError> {
    if vectors.len() != expected_count {
        return Err(SearchError::Backend(format!(
            "Python embedding provider {:?} returned {} vectors for {expected_count} inputs",
            descriptor.id,
            vectors.len()
        )));
    }
    for (index, vector) in vectors.iter().enumerate() {
        if vector.len() != descriptor.dimension {
            return Err(SearchError::Backend(format!(
                "Python embedding provider {:?} returned {} dimensions for vector {index}, expected {}",
                descriptor.id,
                vector.len(),
                descriptor.dimension
            )));
        }
        if vector.iter().any(|value| !value.is_finite()) {
            return Err(SearchError::Backend(format!(
                "Python embedding provider {:?} returned a non-finite value in vector {index}",
                descriptor.id
            )));
        }
    }
    Ok(())
}
