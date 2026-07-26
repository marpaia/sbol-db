use std::ptr::{self, NonNull};

use crate::ffi;
use crate::native::{check_return_code, Error, IDSelectorBatch, SearchParams};

pub(crate) struct FilteredSearchParameters {
    parameters: NonNull<ffi::FaissSearchParameters>,
    _selector: IDSelectorBatch,
}

impl FilteredSearchParameters {
    pub(crate) fn new(ids: &[i64]) -> crate::native::Result<Self> {
        let selector = IDSelectorBatch::new(ids)?;
        let mut parameters = ptr::null_mut();
        check_return_code(unsafe {
            ffi::faiss_SearchParameters_new(&mut parameters, selector.as_ptr().cast_mut())
        })?;
        let parameters = NonNull::new(parameters).ok_or(Error::NullPointer)?;
        Ok(Self {
            parameters,
            _selector: selector,
        })
    }
}

impl SearchParams for FilteredSearchParameters {
    fn as_ptr(&self) -> *const ffi::FaissSearchParameters {
        self.parameters.as_ptr()
    }
}

impl Drop for FilteredSearchParameters {
    fn drop(&mut self) {
        unsafe { ffi::faiss_SearchParameters_free(self.parameters.as_ptr()) }
    }
}

/// IVF query parameters that own the native ID selector referenced by FAISS.
pub(crate) struct FilteredSearchParametersIvf {
    parameters: NonNull<ffi::FaissSearchParametersIVF>,
    _selector: IDSelectorBatch,
}

impl FilteredSearchParametersIvf {
    pub(crate) fn new(ids: &[i64], nprobe: usize, max_codes: usize) -> crate::native::Result<Self> {
        let selector = IDSelectorBatch::new(ids)?;
        let mut parameters = ptr::null_mut();
        check_return_code(unsafe {
            ffi::faiss_SearchParametersIVF_new_with(
                &mut parameters,
                selector.as_ptr().cast_mut(),
                nprobe,
                max_codes,
            )
        })?;
        let parameters = NonNull::new(parameters).ok_or(Error::NullPointer)?;
        Ok(Self {
            parameters,
            _selector: selector,
        })
    }
}

impl SearchParams for FilteredSearchParametersIvf {
    fn as_ptr(&self) -> *const ffi::FaissSearchParameters {
        self.parameters.as_ptr().cast()
    }
}

impl Drop for FilteredSearchParametersIvf {
    fn drop(&mut self) {
        unsafe { ffi::faiss_SearchParametersIVF_free(self.parameters.as_ptr()) }
    }
}
