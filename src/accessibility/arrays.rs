use std::ffi::c_void;
use windows::{
    core::{IUnknown, Interface, Result},
    Win32::{
        Foundation::E_OUTOFMEMORY,
        System::{
            Com::SAFEARRAY,
            Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement},
            Variant::{VARENUM, VT_I4, VT_UNKNOWN},
        },
    },
};

struct Array(*mut SAFEARRAY);
impl Drop for Array {
    fn drop(&mut self) {
        if let Err(error) = unsafe { SafeArrayDestroy(self.0) } {
            debug!("uia stage=array-release code={:#x}", error.code().0);
        }
    }
}

fn build(
    kind: VARENUM,
    count: usize,
    pointer: impl Fn(usize) -> *const c_void,
) -> Result<*mut SAFEARRAY> {
    let array = Array(unsafe { SafeArrayCreateVector(kind, 0, count as u32) });
    if array.0.is_null() {
        return Err(E_OUTOFMEMORY.into());
    }
    for index in 0..count {
        unsafe { SafeArrayPutElement(array.0, &(index as i32), pointer(index)) }?;
    }
    let result = array.0;
    std::mem::forget(array);
    Ok(result)
}

pub(super) fn integers(values: &[i32]) -> Result<*mut SAFEARRAY> {
    build(VT_I4, values.len(), |index| {
        (&values[index] as *const i32).cast()
    })
}

pub(super) fn providers(values: &[IUnknown]) -> Result<*mut SAFEARRAY> {
    // VT_UNKNOWN takes the interface pointer itself and AddRefs it, not its address.
    build(VT_UNKNOWN, values.len(), |index| {
        values[index].as_raw().cast_const()
    })
}
