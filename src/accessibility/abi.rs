//! Nullable COM outputs are explicit. The generated 0.61 UIA traits cannot return
//! S_OK + null for optional providers, so these ABI-identical declarations own
//! output initialization. IIDs/order/types match the locked Windows bindings.
#![allow(non_snake_case)] // Preserve the native COM method names in the ABI.
use std::{ffi::c_void, ptr};
use windows::{
    core::{interface, IUnknown, IUnknown_Vtbl, Interface, HRESULT},
    Win32::{
        Foundation::{E_POINTER, S_OK},
        System::{Com::SAFEARRAY, Variant::VARIANT},
        UI::Accessibility::{
            NavigateDirection, ProviderOptions, UiaRect, UIA_PATTERN_ID, UIA_PROPERTY_ID,
        },
    },
};

#[interface("d6dd68d1-86fd-4332-8666-9abedea2d24c")]
pub(super) unsafe trait SimpleAbi: IUnknown {
    fn ProviderOptions(&self, output: *mut ProviderOptions) -> HRESULT;
    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID, output: *mut *mut c_void) -> HRESULT;
    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID, output: *mut VARIANT) -> HRESULT;
    fn HostRawElementProvider(&self, output: *mut *mut c_void) -> HRESULT;
}

#[interface("f7063da8-8359-439c-9297-bbc5299a7d87")]
pub(super) unsafe trait FragmentAbi: IUnknown {
    fn Navigate(&self, direction: NavigateDirection, output: *mut *mut c_void) -> HRESULT;
    fn GetRuntimeId(&self, output: *mut *mut SAFEARRAY) -> HRESULT;
    fn BoundingRectangle(&self, output: *mut UiaRect) -> HRESULT;
    fn GetEmbeddedFragmentRoots(&self, output: *mut *mut SAFEARRAY) -> HRESULT;
    fn SetFocus(&self) -> HRESULT;
    fn FragmentRoot(&self, output: *mut *mut c_void) -> HRESULT;
}

#[interface("620ce2a5-ab8f-40a9-86cb-de3c75599b58")]
pub(super) unsafe trait RootAbi: IUnknown {
    fn ElementProviderFromPoint(&self, x: f64, y: f64, output: *mut *mut c_void) -> HRESULT;
    fn GetFocus(&self, output: *mut *mut c_void) -> HRESULT;
}

pub(super) unsafe fn interface_out<T: Interface>(
    output: *mut *mut c_void,
    value: windows::core::Result<Option<T>>,
) -> HRESULT {
    if output.is_null() {
        return E_POINTER;
    }
    output.write(ptr::null_mut());
    match value {
        Ok(Some(value)) => {
            output.write(value.into_raw());
            S_OK
        }
        Ok(None) => S_OK,
        Err(error) => error.into(),
    }
}

pub(super) unsafe fn value_out<T: Default>(
    output: *mut T,
    value: windows::core::Result<T>,
) -> HRESULT {
    if output.is_null() {
        return E_POINTER;
    }
    output.write(T::default());
    match value {
        Ok(value) => {
            output.write(value);
            S_OK
        }
        Err(error) => error.into(),
    }
}
