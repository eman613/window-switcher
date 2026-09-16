#![allow(non_upper_case_globals)] // Pattern IDs retain their Windows API names.
use super::{
    abi::*,
    arrays,
    node::Node,
    snapshot::{ActionKind, Bridge},
};
use std::{collections::HashSet, ffi::c_void, ptr, sync::Arc};
use windows::{
    core::{implement, IUnknown, IUnknownImpl, Interface, Result, BOOL, HRESULT},
    Win32::{
        Foundation::HWND,
        System::{Com::SAFEARRAY, Variant::VARIANT},
        UI::Accessibility::*,
    },
};

#[implement(SimpleAbi, FragmentAbi, RootAbi, ISelectionProvider)]
struct RootProvider {
    node: Node,
}

#[implement(SimpleAbi, FragmentAbi, ISelectionItemProvider, IInvokeProvider)]
struct ItemProvider {
    node: Node,
}

pub(super) fn root(shared: Arc<Bridge>) -> Result<IRawElementProviderSimple> {
    let mut cached = shared.root.lock();
    if let Some(root) = cached.upgrade() {
        return Ok(root);
    }
    let raw: SimpleAbi = RootProvider {
        node: Node {
            shared: shared.clone(),
            item: None,
        },
    }
    .into();
    // SimpleAbi has the same IID and exact vtable layout as the native interface.
    let root = unsafe { IRawElementProviderSimple::from_raw(raw.into_raw()) };
    // A fragment root returns its own COM identity, including on event threads.
    // Weak storage avoids a Bridge -> provider -> Bridge ownership cycle.
    *cached = root.downgrade()?;
    Ok(root)
}

pub(super) fn item(
    shared: Arc<Bridge>,
    session: u64,
    id: u32,
) -> Result<IRawElementProviderSimple> {
    let snapshot = shared.read()?;
    if !shared.visible(session)
        || snapshot.session != session
        || !snapshot.entries.iter().any(|entry| entry.id == id)
    {
        return Err(super::snapshot::unavailable());
    }
    let mut cached = shared.items.lock();
    if let Some(item) = cached.get(&(session, id)).and_then(|item| item.upgrade()) {
        return Ok(item);
    }
    if !shared.visible(session) {
        return Err(super::snapshot::unavailable());
    }
    // Keep weak identities only for the active snapshot, even during long sessions.
    let ids: HashSet<_> = snapshot.entries.iter().map(|entry| entry.id).collect();
    cached.retain(|(entry_session, id), _| *entry_session == session && ids.contains(id));
    let raw: SimpleAbi = ItemProvider {
        node: Node {
            shared: shared.clone(),
            item: Some((session, id)),
        },
    }
    .into();
    let item = unsafe { IRawElementProviderSimple::from_raw(raw.into_raw()) };
    cached.insert((session, id), item.downgrade()?);
    Ok(item)
}

macro_rules! base_provider {
    ($implementation:ty) => {
        impl SimpleAbi_Impl for $implementation {
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
                interface_out(output, self.pattern(pattern))
            }
            unsafe fn GetPropertyValue(
                &self,
                property: UIA_PROPERTY_ID,
                output: *mut VARIANT,
            ) -> HRESULT {
                value_out(output, self.node.property(property))
            }
            unsafe fn HostRawElementProvider(&self, output: *mut *mut c_void) -> HRESULT {
                interface_out(
                    output,
                    self.node.snapshot().and_then(|_| {
                        if self.node.item.is_some() {
                            Ok(None)
                        } else {
                            UiaHostProviderFromHwnd(HWND(self.node.shared.target.window_id() as _))
                                .map(Some)
                        }
                    }),
                )
            }
        }
        impl FragmentAbi_Impl for $implementation {
            unsafe fn Navigate(
                &self,
                direction: NavigateDirection,
                output: *mut *mut c_void,
            ) -> HRESULT {
                interface_out(output, self.node.navigate(direction))
            }
            unsafe fn GetRuntimeId(&self, output: *mut *mut SAFEARRAY) -> HRESULT {
                if output.is_null() {
                    return windows::Win32::Foundation::E_POINTER;
                }
                value_out(output, self.node.runtime_id())
            }
            unsafe fn BoundingRectangle(&self, output: *mut UiaRect) -> HRESULT {
                value_out(output, self.node.bounds())
            }
            unsafe fn GetEmbeddedFragmentRoots(&self, output: *mut *mut SAFEARRAY) -> HRESULT {
                value_out(output, Ok(ptr::null_mut()))
            }
            unsafe fn SetFocus(&self) -> HRESULT {
                self.node.act(ActionKind::Select).into()
            }
            unsafe fn FragmentRoot(&self, output: *mut *mut c_void) -> HRESULT {
                interface_out(
                    output,
                    self.node.snapshot().and_then(|_| {
                        self.node
                            .root()?
                            .cast::<IRawElementProviderFragmentRoot>()
                            .map(Some)
                    }),
                )
            }
        }
    };
}
base_provider!(RootProvider_Impl);
base_provider!(ItemProvider_Impl);

impl RootProvider_Impl {
    fn pattern(&self, pattern: UIA_PATTERN_ID) -> Result<Option<IUnknown>> {
        self.node.snapshot()?;
        if pattern == UIA_SelectionPatternId {
            self.to_interface::<ISelectionProvider>().cast().map(Some)
        } else {
            Ok(None)
        }
    }
}
impl ItemProvider_Impl {
    fn pattern(&self, pattern: UIA_PATTERN_ID) -> Result<Option<IUnknown>> {
        self.node.snapshot()?;
        match pattern {
            UIA_SelectionItemPatternId => self
                .to_interface::<ISelectionItemProvider>()
                .cast()
                .map(Some),
            UIA_InvokePatternId => self.to_interface::<IInvokeProvider>().cast().map(Some),
            _ => Ok(None),
        }
    }
}
impl RootAbi_Impl for RootProvider_Impl {
    unsafe fn ElementProviderFromPoint(&self, x: f64, y: f64, output: *mut *mut c_void) -> HRESULT {
        interface_out(output, self.node.provider_at_point(x, y))
    }
    unsafe fn GetFocus(&self, output: *mut *mut c_void) -> HRESULT {
        interface_out(output, self.node.focus())
    }
}
impl ISelectionProvider_Impl for RootProvider_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let selected = self
            .node
            .selection()?
            .map(|provider| provider.cast::<IUnknown>())
            .transpose()?;
        arrays::providers(&selected.into_iter().collect::<Vec<_>>())
    }
    fn CanSelectMultiple(&self) -> Result<BOOL> {
        self.node.snapshot()?;
        Ok(false.into())
    }
    fn IsSelectionRequired(&self) -> Result<BOOL> {
        self.node.snapshot()?;
        Ok(true.into())
    }
}
impl ISelectionItemProvider_Impl for ItemProvider_Impl {
    fn Select(&self) -> Result<()> {
        self.node.act(ActionKind::Select)
    }
    fn AddToSelection(&self) -> Result<()> {
        self.node.act(ActionKind::Select)
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        self.node.snapshot()?;
        Err(HRESULT(UIA_E_INVALIDOPERATION as i32).into())
    }
    fn IsSelected(&self) -> Result<BOOL> {
        self.node.selected().map(Into::into)
    }
    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        self.node.snapshot()?;
        self.node.root()
    }
}
impl IInvokeProvider_Impl for ItemProvider_Impl {
    fn Invoke(&self) -> Result<()> {
        self.node.act(ActionKind::Invoke)
    }
}
