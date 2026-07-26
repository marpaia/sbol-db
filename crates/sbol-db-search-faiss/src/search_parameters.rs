use std::ffi::CStr;
use std::ptr::{self, NonNull};

use faiss_next::{Error, IDSelector, IDSelectorBatch, SearchParams};
use faiss_next_sys::{FaissSearchParameters, FaissSearchParametersIVF};

pub(crate) struct FilteredSearchParameters {
    parameters: NonNull<FaissSearchParameters>,
    _selector: IDSelectorBatch,
}

impl FilteredSearchParameters {
    pub(crate) fn new(ids: &[i64]) -> faiss_next::Result<Self> {
        let selector = IDSelectorBatch::new(ids)?;
        let mut parameters = ptr::null_mut();
        let code = unsafe {
            faiss_next_sys::faiss_SearchParameters_new(
                &mut parameters,
                selector.as_ptr().cast_mut(),
            )
        };
        check_return_code(code)?;
        let parameters = NonNull::new(parameters).ok_or(Error::NullPointer)?;
        Ok(Self {
            parameters,
            _selector: selector,
        })
    }
}

impl SearchParams for FilteredSearchParameters {
    fn as_ptr(&self) -> *const FaissSearchParameters {
        self.parameters.as_ptr()
    }
}

impl Drop for FilteredSearchParameters {
    fn drop(&mut self) {
        unsafe {
            faiss_next_sys::faiss_SearchParameters_free(self.parameters.as_ptr());
        }
    }
}

/// IVF query parameters that own the native ID selector referenced by FAISS.
///
/// `faiss-next` exposes both pieces but does not currently connect a selector
/// to `SearchParametersIvf`. Keeping this small bridge here avoids exposing
/// unsafe FFI anywhere in the backend implementation.
pub(crate) struct FilteredSearchParametersIvf {
    parameters: NonNull<FaissSearchParametersIVF>,
    _selector: IDSelectorBatch,
}

impl FilteredSearchParametersIvf {
    pub(crate) fn new(ids: &[i64], nprobe: usize, max_codes: usize) -> faiss_next::Result<Self> {
        let selector = IDSelectorBatch::new(ids)?;
        let mut parameters = ptr::null_mut();
        let code = unsafe {
            faiss_next_sys::faiss_SearchParametersIVF_new_with(
                &mut parameters,
                selector.as_ptr().cast_mut(),
                nprobe,
                max_codes,
            )
        };
        check_return_code(code)?;
        let parameters = NonNull::new(parameters).ok_or(Error::NullPointer)?;
        Ok(Self {
            parameters,
            _selector: selector,
        })
    }
}

impl SearchParams for FilteredSearchParametersIvf {
    fn as_ptr(&self) -> *const FaissSearchParameters {
        self.parameters.as_ptr().cast()
    }
}

impl Drop for FilteredSearchParametersIvf {
    fn drop(&mut self) {
        unsafe {
            faiss_next_sys::faiss_SearchParametersIVF_free(self.parameters.as_ptr());
        }
    }
}

pub(crate) fn check_return_code(code: i32) -> faiss_next::Result<()> {
    if code == faiss_next_sys::FAISS_OK {
        return Ok(());
    }
    let message = unsafe {
        let pointer = faiss_next_sys::faiss_get_last_error();
        if pointer.is_null() {
            "unknown error".to_owned()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        }
    };
    Err(Error::native(code, message))
}
