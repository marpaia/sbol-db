use std::fs::{self, File};
use std::path::Path;

use faiss_next::{index_factory, read_index, write_index, Idx, Index, IndexImpl, MetricType};
use sbol_db_search_sdk::{DistanceMetric, VectorError, VectorFilter};

use crate::config::FaissBackendConfig;
use crate::filter::PayloadIndex;
use crate::model::{GenerationManifest, IndexProfile, StoredRecord, FORMAT_VERSION};
use crate::persistence::{checksum_file, io_error, sync_directory};
use crate::search_parameters::{
    check_return_code, FilteredSearchParameters, FilteredSearchParametersIvf,
};

pub(crate) struct LoadedGeneration {
    pub(crate) manifest: GenerationManifest,
    pub(crate) records: Vec<StoredRecord>,
    payload_index: PayloadIndex,
    index: IndexImpl,
}

impl LoadedGeneration {
    pub(crate) fn load(
        directory: &Path,
        manifest: GenerationManifest,
        records: Vec<StoredRecord>,
    ) -> Result<Self, VectorError> {
        if manifest.format_version != FORMAT_VERSION {
            return Err(VectorError::Unsupported(format!(
                "unsupported FAISS generation format {}",
                manifest.format_version
            )));
        }
        if manifest.vector_count != records.len() {
            return Err(VectorError::Backend(format!(
                "generation {:?}/{:?} manifest count {} does not match records count {}",
                manifest.spec.artifact_id,
                manifest.spec.generation,
                manifest.vector_count,
                records.len()
            )));
        }
        let index_path = directory.join("index.faiss");
        let index_checksum = checksum_file(&index_path)?;
        if index_checksum != manifest.index_sha3_256 {
            return Err(VectorError::Backend(format!(
                "refusing to load corrupted FAISS index {:?}",
                index_path
            )));
        }
        let path = utf8_path(&index_path)?;
        let index = read_index(path).map_err(faiss_error)?;
        if index.d() as usize != manifest.spec.dimension {
            return Err(VectorError::Backend(format!(
                "FAISS index dimension {} does not match generation dimension {}",
                index.d(),
                manifest.spec.dimension
            )));
        }
        if index.ntotal() as usize != manifest.vector_count {
            return Err(VectorError::Backend(format!(
                "FAISS index count {} does not match generation count {}",
                index.ntotal(),
                manifest.vector_count
            )));
        }
        let payload_index = PayloadIndex::build(&records)?;
        Ok(Self {
            manifest,
            records,
            payload_index,
            index,
        })
    }

    pub(crate) fn search(
        &self,
        mut query: Vec<f32>,
        filter: Option<&VectorFilter>,
        k: usize,
        nprobe: usize,
        max_codes: usize,
    ) -> Result<Vec<(usize, f32)>, VectorError> {
        if self.records.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        if self.manifest.spec.distance == DistanceMetric::Cosine {
            normalize(&mut query)?;
        }
        let allowed = match filter {
            Some(filter) => self.payload_index.allowed(filter)?,
            None => self.payload_index.all_ids(),
        };
        if allowed.is_empty() {
            return Ok(Vec::new());
        }
        let result = match self.manifest.profile {
            IndexProfile::Flat => {
                let parameters = FilteredSearchParameters::new(&allowed).map_err(faiss_error)?;
                search_read_only(&self.index, &query, k, &parameters)?
            }
            IndexProfile::IvfFlat => {
                let parameters = FilteredSearchParametersIvf::new(&allowed, nprobe, max_codes)
                    .map_err(faiss_error)?;
                search_read_only(&self.index, &query, k, &parameters)?
            }
        };
        result
            .labels
            .into_iter()
            .zip(result.distances)
            .filter_map(|(label, distance)| label.get().map(|label| (label, distance)))
            .map(|(label, distance)| {
                let id = usize::try_from(label).map_err(|_| {
                    VectorError::Backend(format!("FAISS returned out-of-range id {label}"))
                })?;
                if id >= self.records.len() {
                    return Err(VectorError::Backend(format!(
                        "FAISS returned unknown document id {label}"
                    )));
                }
                Ok((id, portable_score(distance, self.manifest.spec.distance)))
            })
            .collect()
    }
}

pub(crate) fn build_index(
    directory: &Path,
    spec: &sbol_db_search_sdk::IndexGenerationSpec,
    records: &[StoredRecord],
    config: &FaissBackendConfig,
    records_checksum: String,
) -> Result<GenerationManifest, VectorError> {
    let requested_nlist = optional_usize(&spec.parameters, "nlist")?;
    let requested_nprobe = optional_usize(&spec.parameters, "nprobe")?;
    let cutoff = optional_usize(&spec.parameters, "flat_search_cutoff")?
        .unwrap_or(config.flat_search_cutoff);
    let profile = if records.len() < cutoff || records.is_empty() {
        IndexProfile::Flat
    } else {
        IndexProfile::IvfFlat
    };
    let nlist = match profile {
        IndexProfile::Flat => 0,
        IndexProfile::IvfFlat => {
            let maximum = (records.len() / 39).max(1);
            match requested_nlist {
                Some(value) if value == 0 || value > maximum => {
                    return Err(VectorError::InvalidRequest(format!(
                        "nlist must be between 1 and {maximum} for {} training vectors",
                        records.len()
                    )));
                }
                Some(value) => value,
                None => config.default_nlist.min(maximum).max(1),
            }
        }
    };
    let default_nprobe = match profile {
        IndexProfile::Flat => 0,
        IndexProfile::IvfFlat => requested_nprobe
            .unwrap_or(config.default_nprobe)
            .clamp(1, nlist),
    };
    let index_description = match profile {
        IndexProfile::Flat => "IDMap2,Flat".to_owned(),
        IndexProfile::IvfFlat => format!("IVF{nlist},Flat"),
    };
    let metric = match spec.distance {
        DistanceMetric::Cosine | DistanceMetric::Dot => MetricType::InnerProduct,
        DistanceMetric::Euclidean => MetricType::L2,
        other => {
            return Err(VectorError::Unsupported(format!(
                "distance {other:?} is not supported by the FAISS backend"
            )));
        }
    };
    let dimension = u32::try_from(spec.dimension).map_err(|_| {
        VectorError::Unsupported("FAISS vector dimension exceeds u32::MAX".to_owned())
    })?;
    let mut index = index_factory(dimension, &index_description, metric).map_err(faiss_error)?;
    let mut vectors = Vec::with_capacity(records.len().saturating_mul(spec.dimension));
    for record in records {
        let mut vector = record.vector.clone();
        if spec.distance == DistanceMetric::Cosine {
            normalize(&mut vector)?;
        }
        vectors.extend(vector);
    }
    if profile == IndexProfile::IvfFlat {
        index.train(&vectors).map_err(faiss_error)?;
    }
    if !records.is_empty() {
        let ids = (0..records.len())
            .map(|id| Idx::new(id as u64))
            .collect::<Vec<_>>();
        index.add_with_ids(&vectors, &ids).map_err(faiss_error)?;
    }

    let index_path = directory.join("index.faiss");
    let building_path = directory.join("index.faiss.building");
    if building_path.exists() {
        fs::remove_file(&building_path).map_err(io_error)?;
    }
    write_index(&index, utf8_path(&building_path)?).map_err(faiss_error)?;
    File::open(&building_path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    fs::rename(&building_path, &index_path).map_err(io_error)?;
    sync_directory(directory)?;
    let index_checksum = checksum_file(&index_path)?;
    Ok(GenerationManifest {
        format_version: FORMAT_VERSION,
        spec: spec.clone(),
        profile,
        index_factory: index_description,
        vector_count: records.len(),
        nlist,
        default_nprobe,
        faiss_version: option_env!("FAISS_VERSION").unwrap_or("unknown").to_owned(),
        records_sha3_256: records_checksum,
        index_sha3_256: index_checksum,
    })
}

fn search_read_only<P: faiss_next::SearchParams>(
    index: &IndexImpl,
    query: &[f32],
    k: usize,
    parameters: &P,
) -> Result<faiss_next::SearchResult, VectorError> {
    let mut distances = vec![0.0_f32; k];
    let mut labels = vec![Idx::NONE; k];
    let code = unsafe {
        faiss_next_sys::faiss_Index_search_with_params(
            index.inner_ptr(),
            1,
            query.as_ptr(),
            k as i64,
            parameters.as_ptr(),
            distances.as_mut_ptr(),
            labels.as_mut_ptr().cast(),
        )
    };
    check_return_code(code).map_err(faiss_error)?;
    Ok(faiss_next::SearchResult::new(distances, labels))
}

fn optional_usize(
    parameters: &std::collections::BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Result<Option<usize>, VectorError> {
    let Some(value) = parameters.get(key) else {
        return Ok(None);
    };
    let value = value.as_u64().ok_or_else(|| {
        VectorError::InvalidRequest(format!(
            "FAISS parameter {key:?} must be an unsigned integer"
        ))
    })?;
    usize::try_from(value)
        .map(Some)
        .map_err(|_| VectorError::InvalidRequest(format!("FAISS parameter {key:?} is too large")))
}

pub(crate) fn normalize(vector: &mut [f32]) -> Result<(), VectorError> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return Err(VectorError::InvalidRequest(
            "cosine vector must have finite, non-zero magnitude".to_owned(),
        ));
    }
    for value in vector {
        *value /= norm;
    }
    Ok(())
}

fn portable_score(distance: f32, metric: DistanceMetric) -> f32 {
    match metric {
        DistanceMetric::Cosine | DistanceMetric::Dot => distance,
        DistanceMetric::Euclidean => -distance.max(0.0).sqrt(),
        _ => unreachable!("unsupported metrics are rejected during generation creation"),
    }
}

fn utf8_path(path: &Path) -> Result<&str, VectorError> {
    path.to_str().ok_or_else(|| {
        VectorError::Unsupported(format!("FAISS paths must be valid UTF-8: {path:?}"))
    })
}

fn faiss_error(error: faiss_next::Error) -> VectorError {
    VectorError::Backend(error.to_string())
}
