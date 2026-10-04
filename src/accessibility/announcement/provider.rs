//! Hosted popup provider: WindowPattern is required for UIA notifications.
#![allow(non_upper_case_globals)]
use super::Shared;
use crate::accessibility::abi::*;
use std::{ffi::c_void, sync::Arc};
use windows::{
    core::{implement, IUnknownImpl, Interface, BOOL, HRESULT},
    Win32::{
        Foundation::HWND,
        System::Variant::VARIANT,
        UI::{Accessibility::*, WindowsAndMessaging::WM_CLOSE},
    },
};

#[implement(SimpleAbi, IWindowProvider)]
struct Provider {
    shared: Arc<Shared>,
}

pub(super) fn root(shared: Arc<Shared>) -> windows::core::Result<IRawElementProviderSimple> {
    let mut cache = shared.root.lock();
    if let Some(root) = cache.upgrade() {
        return Ok(root);
    }
    let raw: SimpleAbi = Provider {
        shared: shared.clone(),
    }
    .into();
    let root = unsafe { IRawElementProviderSimple::from_raw(raw.into_raw()) };
    *cache = root.downgrade()?;
    Ok(root)
}

impl SimpleAbi_Impl for Provider_Impl {
    unsafe fn ProviderOptions(&self, output: *mut ProviderOptions) -> HRESULT {
        value_out(
            output,
            Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading),
        )
    }
    unsafe fn GetPatternProvider(
        &self,
        pattern: UIA_PATTERN_ID,
        output: *mut *mut c_void,
    ) -> HRESULT {
        interface_out(
            output,
            self.shared.available().and_then(|_| {
                if pattern == UIA_WindowPatternId {
                    self.to_interface::<IWindowProvider>()
                        .cast::<windows::core::IUnknown>()
                        .map(Some)
                } else {
                    Ok(None)
                }
            }),
        )
    }
    unsafe fn GetPropertyValue(&self, property: UIA_PROPERTY_ID, output: *mut VARIANT) -> HRESULT {
        value_out(
            output,
            self.shared.available().map(|_| match property {
                UIA_NamePropertyId => self.shared.name.as_str().into(),
                UIA_ControlTypePropertyId => UIA_WindowControlTypeId.0.into(),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => true.into(),
                _ => VARIANT::default(),
            }),
        )
    }
    unsafe fn HostRawElementProvider(&self, output: *mut *mut c_void) -> HRESULT {
        interface_out(
            output,
            self.shared.available().and_then(|_| {
                UiaHostProviderFromHwnd(HWND(self.shared.target.window_id() as _)).map(Some)
            }),
        )
    }
}

impl IWindowProvider_Impl for Provider_Impl {
    fn SetVisualState(&self, state: WindowVisualState) -> windows::core::Result<()> {
        self.shared.available()?;
        if state == WindowVisualState_Normal {
            Ok(())
        } else {
            Err(windows::core::HRESULT(UIA_E_INVALIDOPERATION as i32).into())
        }
    }
    fn Close(&self) -> windows::core::Result<()> {
        self.shared.available()?;
        if self.shared.target.try_post(WM_CLOSE) {
            Ok(())
        } else {
            Err(crate::accessibility::snapshot::unavailable())
        }
    }
    fn WaitForInputIdle(&self, _: i32) -> windows::core::Result<BOOL> {
        self.shared.available().map(|_| false.into())
    }
    fn CanMaximize(&self) -> windows::core::Result<BOOL> {
        self.shared.available().map(|_| false.into())
    }
    fn CanMinimize(&self) -> windows::core::Result<BOOL> {
        self.shared.available().map(|_| false.into())
    }
    fn IsModal(&self) -> windows::core::Result<BOOL> {
        self.shared.available().map(|_| false.into())
    }
    fn WindowVisualState(&self) -> windows::core::Result<WindowVisualState> {
        self.shared.available().map(|_| WindowVisualState_Normal)
    }
    fn WindowInteractionState(&self) -> windows::core::Result<WindowInteractionState> {
        self.shared
            .available()
            .map(|_| WindowInteractionState_ReadyForUserInteraction)
    }
    fn IsTopmost(&self) -> windows::core::Result<BOOL> {
        self.shared.available().map(|_| true.into())
    }
}
