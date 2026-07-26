//! Embedded FAISS vector-search backend for sbol-db.
//!
//! FAISS is used only for vector indexing and nearest-neighbor execution.
//! sbol-db remains responsible for document identity, authorization payloads,
//! durable generations, activation, and maintenance.

#[cfg(feature = "native")]
mod backend;
mod config;
#[cfg(feature = "native")]
mod engine;
#[cfg(feature = "native")]
mod filter;
#[cfg(feature = "native")]
mod model;
#[cfg(feature = "native")]
mod persistence;
#[cfg(feature = "native")]
mod search_parameters;

#[cfg(feature = "native")]
pub use backend::FaissVectorBackend;
pub use config::FaissBackendConfig;

#[cfg(all(test, feature = "native"))]
mod tests {
    use faiss_next::{index_factory, Idx, Index, MetricType};

    use crate::search_parameters::FilteredSearchParametersIvf;

    #[test]
    fn ivf_search_applies_an_id_selector_before_ranking() {
        const DIMENSION: usize = 4;
        const COUNT: usize = 160;

        let mut vectors = Vec::with_capacity(COUNT * DIMENSION);
        for id in 0..COUNT {
            vectors.extend_from_slice(&[id as f32, 1.0, 0.0, 0.0]);
        }

        let mut index =
            index_factory(DIMENSION as u32, "IVF4,Flat", MetricType::L2).expect("create IVF index");
        index.train(&vectors).expect("train IVF index");
        let ids = (0..COUNT).map(|id| Idx::new(id as u64)).collect::<Vec<_>>();
        index
            .add_with_ids(&vectors, &ids)
            .expect("populate IVF index");

        // Permit only ids 80..=159. The nearest unfiltered vector to this
        // query would be id 0, so returning id 80 proves pre-ranking filtering.
        let allowed = (80..COUNT).map(|id| id as i64).collect::<Vec<_>>();
        let parameters =
            FilteredSearchParametersIvf::new(&allowed, 4, 0).expect("create search parameters");
        let result = index
            .search_with_params(&[0.0, 1.0, 0.0, 0.0], 1, &parameters)
            .expect("search IVF index");

        assert_eq!(result.labels, vec![Idx::new(80)]);
    }
}
