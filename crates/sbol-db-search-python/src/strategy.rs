use std::sync::Arc;

use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::types::PyModule;
use sbol_db_search::EmbeddingStrategyConfig;
use sbol_db_search_sdk::{
    EmbeddingProvider, FilterCapability, FilterKind, PaginationCapability,
    SearchContext as RustSearchContext, SearchError, SearchInputKind, SearchPage, SearchRequest,
    SearchStrategy, StrategyCapabilities, StrategyDescriptor, StrategyRequirements,
    TotalCapability,
};
use serde_json::{json, Value};
use tokio::runtime::Handle;

use crate::context::PythonSearchContext;
use crate::conversion::{json_to_python, python_execution, python_to_json};

/// A Python strategy waiting for the process composition root to resolve its
/// declared embedding profile. This keeps provider lookup centralized and
/// permits a Python strategy to reuse a provider registered by another module.
pub struct PythonStrategyRegistration {
    implementation: Py<PyAny>,
    config: EmbeddingStrategyConfig,
}

impl PythonStrategyRegistration {
    pub(crate) fn new(implementation: Py<PyAny>, config: EmbeddingStrategyConfig) -> Self {
        Self {
            implementation,
            config,
        }
    }

    pub fn embedding_profile(&self) -> &str {
        &self.config.embedding_profile
    }

    pub fn bind(
        self,
        embedding: Arc<dyn EmbeddingProvider>,
    ) -> Result<Arc<dyn SearchStrategy>, SearchError> {
        if embedding.descriptor().id != self.config.embedding_profile {
            return Err(SearchError::Configuration(format!(
                "Python strategy {:?} requests embedding profile {:?}, but was bound to {:?}",
                self.config.id,
                self.config.embedding_profile,
                embedding.descriptor().id
            )));
        }
        let descriptor = StrategyDescriptor {
            id: self.config.id.clone(),
            version: self.config.version.clone(),
            display_name: self.config.display_name.clone(),
            description: self.config.description.clone(),
            capabilities: StrategyCapabilities {
                inputs: vec![SearchInputKind::Text],
                filters: vec![FilterKind::Graph],
                filter_execution: FilterCapability::Native,
                pagination: PaginationCapability::Cursor,
                totals: TotalCapability::Unknown,
                deterministic: false,
                explanations: true,
                data_egress: embedding.descriptor().data_egress,
            },
            requirements: StrategyRequirements {
                embedding_profiles: vec![self.config.embedding_profile.clone()],
                vector_indexes: vec![self.config.vector_index.clone()],
                candidate_sources: Vec::new(),
            },
        };
        Ok(Arc::new(PythonSearchStrategy {
            implementation: self.implementation,
            descriptor,
            config: self.config,
            embedding,
        }))
    }
}

struct PythonSearchStrategy {
    implementation: Py<PyAny>,
    descriptor: StrategyDescriptor,
    config: EmbeddingStrategyConfig,
    embedding: Arc<dyn EmbeddingProvider>,
}

#[async_trait]
impl SearchStrategy for PythonSearchStrategy {
    fn descriptor(&self) -> &StrategyDescriptor {
        &self.descriptor
    }

    async fn search(
        &self,
        ctx: RustSearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError> {
        let vectors = Arc::clone(ctx.vectors()?);
        let documents = Arc::clone(ctx.documents()?);
        let implementation = Python::with_gil(|py| self.implementation.clone_ref(py));
        let descriptor = self.descriptor.clone();
        let requested_limit = request.page.limit;
        let context = PythonSearchContext {
            scope: ctx.scope().clone(),
            budget: ctx.budget().clone(),
            vectors,
            documents,
            embedding: self.embedding.clone(),
            default_index: self.config.vector_index.clone(),
            default_vector_name: self.config.vector_name.clone(),
            runtime: Handle::current(),
        };
        let request = serde_json::to_value(request).map_err(|error| {
            SearchError::Backend(format!("serializing request for Python strategy: {error}"))
        })?;

        tokio::task::spawn_blocking(move || {
            Python::with_gil(|py| {
                let context = Py::new(py, context).map_err(python_execution)?;
                let request = json_to_python(py, &request).map_err(python_execution)?;
                let output = implementation
                    .bind(py)
                    .call_method1("search", (context, request))
                    .map_err(python_execution)?;
                let inspect = PyModule::import(py, "inspect").map_err(python_execution)?;
                let awaitable = inspect
                    .call_method1("isawaitable", (&output,))
                    .and_then(|value| value.extract::<bool>())
                    .map_err(python_execution)?;
                let output = if awaitable {
                    PyModule::import(py, "asyncio")
                        .and_then(|asyncio| asyncio.call_method1("run", (output,)))
                        .map_err(python_execution)?
                } else {
                    output
                };
                let output = python_to_json(py, &output).map_err(python_execution)?;
                python_page(output, &descriptor, requested_limit)
            })
        })
        .await
        .map_err(|error| SearchError::Backend(format!("joining Python strategy task: {error}")))?
    }
}

fn python_page(
    mut value: Value,
    descriptor: &StrategyDescriptor,
    requested_limit: usize,
) -> Result<SearchPage, SearchError> {
    let object = value.as_object_mut().ok_or_else(|| {
        SearchError::Backend(format!(
            "Python strategy {:?} must return a mapping",
            descriptor.id
        ))
    })?;
    object.entry("strategy").or_insert_with(|| {
        json!({
            "id": descriptor.id,
            "version": descriptor.version,
        })
    });
    object
        .entry("total")
        .or_insert_with(|| json!({"kind": "unknown"}));
    object.entry("execution").or_insert_with(|| json!({}));

    let page: SearchPage = serde_json::from_value(value).map_err(|error| {
        SearchError::Backend(format!(
            "Python strategy {:?} returned an invalid search page: {error}",
            descriptor.id
        ))
    })?;
    if page.strategy.id != descriptor.id || page.strategy.version != descriptor.version {
        return Err(SearchError::Backend(format!(
            "Python strategy returned identity {:?}@{:?}, expected {:?}@{:?}",
            page.strategy.id, page.strategy.version, descriptor.id, descriptor.version
        )));
    }
    if page.items.len() > requested_limit {
        return Err(SearchError::Backend(format!(
            "Python strategy {:?} returned {} items for a page limit of {requested_limit}",
            descriptor.id,
            page.items.len()
        )));
    }
    Ok(page)
}
