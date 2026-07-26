use std::ffi::{CStr, CString};
use std::ptr::{self, NonNull};

use crate::ffi;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("FAISS native error (code={code}): {message}")]
    Native { code: i32, message: String },
    #[error("FAISS returned a null pointer")]
    NullPointer,
    #[error("FAISS input contains an interior NUL byte")]
    InteriorNul,
    #[error("FAISS vector data length {actual} is not divisible by dimension {dimension}")]
    InvalidVectorLength { actual: usize, dimension: usize },
    #[error("FAISS id count {ids} does not match vector count {vectors}")]
    IdCountMismatch { ids: usize, vectors: usize },
    #[error("FAISS dimension {0} exceeds the C API limit")]
    DimensionTooLarge(u32),
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

pub(crate) fn check_return_code(code: i32) -> Result<()> {
    if code == 0 {
        return Ok(());
    }
    let message = unsafe {
        let pointer = ffi::faiss_get_last_error();
        if pointer.is_null() {
            "unknown error".to_owned()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        }
    };
    Err(Error::Native { code, message })
}

#[derive(Clone, Copy)]
pub(crate) enum MetricType {
    InnerProduct,
    L2,
}

impl MetricType {
    fn as_native(self) -> ffi::FaissMetricType {
        match self {
            Self::InnerProduct => ffi::FaissMetricType::METRIC_INNER_PRODUCT,
            Self::L2 => ffi::FaissMetricType::METRIC_L2,
        }
    }
}

pub(crate) struct Index {
    pointer: NonNull<ffi::FaissIndex>,
}

// FAISS documents concurrent CPU index searches as thread-safe. sbol-db only
// mutates an Index while constructing an unpublished generation; LoadedIndex
// exposes shared search access after activation.
unsafe impl Send for Index {}
unsafe impl Sync for Index {}

impl Index {
    pub(crate) fn d(&self) -> u32 {
        unsafe { ffi::faiss_Index_d(self.pointer.as_ptr()) as u32 }
    }

    pub(crate) fn ntotal(&self) -> u64 {
        unsafe { ffi::faiss_Index_ntotal(self.pointer.as_ptr()) as u64 }
    }

    pub(crate) fn train(&mut self, vectors: &[f32]) -> Result<()> {
        let count = self.vector_count(vectors)?;
        check_return_code(unsafe {
            ffi::faiss_Index_train(self.pointer.as_ptr(), count as i64, vectors.as_ptr())
        })
    }

    pub(crate) fn add_with_ids(&mut self, vectors: &[f32], ids: &[i64]) -> Result<()> {
        let count = self.vector_count(vectors)?;
        if count != ids.len() {
            return Err(Error::IdCountMismatch {
                ids: ids.len(),
                vectors: count,
            });
        }
        check_return_code(unsafe {
            ffi::faiss_Index_add_with_ids(
                self.pointer.as_ptr(),
                count as i64,
                vectors.as_ptr(),
                ids.as_ptr(),
            )
        })
    }

    pub(crate) fn search_with_params<P: SearchParams>(
        &self,
        query: &[f32],
        k: usize,
        parameters: &P,
    ) -> Result<SearchResult> {
        if query.len() != self.d() as usize {
            return Err(Error::InvalidVectorLength {
                actual: query.len(),
                dimension: self.d() as usize,
            });
        }
        let mut distances = vec![0.0_f32; k];
        let mut labels = vec![-1_i64; k];
        check_return_code(unsafe {
            ffi::faiss_Index_search_with_params(
                self.as_ptr(),
                1,
                query.as_ptr(),
                k as i64,
                parameters.as_ptr(),
                distances.as_mut_ptr(),
                labels.as_mut_ptr(),
            )
        })?;
        Ok(SearchResult { distances, labels })
    }

    pub(crate) fn as_ptr(&self) -> *const ffi::FaissIndex {
        self.pointer.as_ptr()
    }

    fn vector_count(&self, vectors: &[f32]) -> Result<usize> {
        let dimension = self.d() as usize;
        if dimension == 0 || !vectors.len().is_multiple_of(dimension) {
            return Err(Error::InvalidVectorLength {
                actual: vectors.len(),
                dimension,
            });
        }
        Ok(vectors.len() / dimension)
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        unsafe { ffi::faiss_Index_free(self.pointer.as_ptr()) }
    }
}

pub(crate) fn index_factory(
    dimension: u32,
    description: &str,
    metric: MetricType,
) -> Result<Index> {
    let dimension = i32::try_from(dimension).map_err(|_| Error::DimensionTooLarge(dimension))?;
    let description = CString::new(description).map_err(|_| Error::InteriorNul)?;
    let mut pointer = ptr::null_mut();
    check_return_code(unsafe {
        ffi::faiss_index_factory(
            &mut pointer,
            dimension,
            description.as_ptr(),
            metric.as_native(),
        )
    })?;
    Ok(Index {
        pointer: NonNull::new(pointer).ok_or(Error::NullPointer)?,
    })
}

pub(crate) fn read_index(path: &str) -> Result<Index> {
    let path = CString::new(path).map_err(|_| Error::InteriorNul)?;
    let mut pointer = ptr::null_mut();
    check_return_code(unsafe { ffi::faiss_read_index_fname(path.as_ptr(), 0, &mut pointer) })?;
    Ok(Index {
        pointer: NonNull::new(pointer).ok_or(Error::NullPointer)?,
    })
}

pub(crate) fn write_index(index: &Index, path: &str) -> Result<()> {
    let path = CString::new(path).map_err(|_| Error::InteriorNul)?;
    check_return_code(unsafe { ffi::faiss_write_index_fname(index.as_ptr(), path.as_ptr()) })
}

pub(crate) struct SearchResult {
    pub(crate) distances: Vec<f32>,
    pub(crate) labels: Vec<i64>,
}

pub(crate) trait SearchParams {
    fn as_ptr(&self) -> *const ffi::FaissSearchParameters;
}

pub(crate) struct SearchParameters {
    pointer: NonNull<ffi::FaissSearchParameters>,
}

impl SearchParameters {
    pub(crate) fn new() -> Result<Self> {
        let mut pointer = ptr::null_mut();
        check_return_code(unsafe {
            ffi::faiss_SearchParameters_new(&mut pointer, ptr::null_mut())
        })?;
        Ok(Self {
            pointer: NonNull::new(pointer).ok_or(Error::NullPointer)?,
        })
    }
}

impl SearchParams for SearchParameters {
    fn as_ptr(&self) -> *const ffi::FaissSearchParameters {
        self.pointer.as_ptr()
    }
}

impl Drop for SearchParameters {
    fn drop(&mut self) {
        unsafe { ffi::faiss_SearchParameters_free(self.pointer.as_ptr()) }
    }
}

pub(crate) struct SearchParametersIvf {
    pointer: NonNull<ffi::FaissSearchParametersIVF>,
}

impl SearchParametersIvf {
    pub(crate) fn with_params(nprobe: usize, max_codes: usize) -> Result<Self> {
        let mut pointer = ptr::null_mut();
        check_return_code(unsafe {
            ffi::faiss_SearchParametersIVF_new_with(
                &mut pointer,
                ptr::null_mut(),
                nprobe,
                max_codes,
            )
        })?;
        Ok(Self {
            pointer: NonNull::new(pointer).ok_or(Error::NullPointer)?,
        })
    }
}

impl SearchParams for SearchParametersIvf {
    fn as_ptr(&self) -> *const ffi::FaissSearchParameters {
        self.pointer.as_ptr().cast()
    }
}

impl Drop for SearchParametersIvf {
    fn drop(&mut self) {
        unsafe { ffi::faiss_SearchParametersIVF_free(self.pointer.as_ptr()) }
    }
}

pub(crate) struct IDSelectorBatch {
    pointer: NonNull<ffi::FaissIDSelectorBatch>,
}

impl IDSelectorBatch {
    pub(crate) fn new(ids: &[i64]) -> Result<Self> {
        let mut pointer = ptr::null_mut();
        check_return_code(unsafe {
            ffi::faiss_IDSelectorBatch_new(&mut pointer, ids.len(), ids.as_ptr())
        })?;
        Ok(Self {
            pointer: NonNull::new(pointer).ok_or(Error::NullPointer)?,
        })
    }

    pub(crate) fn as_ptr(&self) -> *const ffi::FaissIDSelector {
        self.pointer.as_ptr().cast()
    }
}

impl Drop for IDSelectorBatch {
    fn drop(&mut self) {
        unsafe { ffi::faiss_IDSelector_free(self.pointer.as_ptr().cast()) }
    }
}
