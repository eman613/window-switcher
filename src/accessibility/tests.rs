use super::*;
use crate::{
    config::Language, icon_cache::IconKey, layout::PixelRect,
    utils::window_identity::WindowIdentity,
};
use windows::{
    core::{IUnknown, Interface, BSTR},
    Win32::{Foundation::HWND, UI::Accessibility::*},
};

fn fixture() -> Arc<Bridge> {
    let shared = Bridge::new(
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::Chinese),
    );
    *shared.snapshot.write() = Arc::new(Snapshot {
        session: 11,
        focused: true,
        selected: Some(1),
        bounds: PixelRect {
            left: 100,
            top: 200,
            right: 400,
            bottom: 300,
        },
        entries: (1..=2)
            .map(|id| Entry {
                id,
                key: IconKey {
                    group: format!("test-{id}").into(),
                    identity: WindowIdentity::fixture(id as usize),
                },
                name: format!("完整应用名称 {id}").into(),
                status: "2 个窗口".into(),
                bounds: (id == 1).then_some(PixelRect {
                    left: 120,
                    top: 210,
                    right: 190,
                    bottom: 280,
                }),
            })
            .collect(),
    });
    shared.active.store(11, Ordering::Release);
    shared
}

#[test]
fn native_uia_names_status_selection_and_bounded_actions_use_value_snapshots() {
    let shared = fixture();
    let first = provider::item(shared.clone(), 11, 1).unwrap();
    assert_eq!(
        BSTR::try_from(&unsafe { first.GetPropertyValue(UIA_NamePropertyId) }.unwrap())
            .unwrap()
            .to_string(),
        "完整应用名称 1"
    );
    assert_eq!(
        BSTR::try_from(&unsafe { first.GetPropertyValue(UIA_ItemStatusPropertyId) }.unwrap())
            .unwrap()
            .to_string(),
        "2 个窗口"
    );
    let selection: ISelectionItemProvider =
        unsafe { first.GetPatternProvider(UIA_SelectionItemPatternId) }
            .unwrap()
            .cast()
            .unwrap();
    assert!(unsafe { selection.IsSelected() }.unwrap().as_bool());
    let second = provider::item(shared.clone(), 11, 2).unwrap();
    let select: ISelectionItemProvider = second.cast().unwrap();
    for _ in 0..32 {
        unsafe { select.Select() }.unwrap();
    }
    assert!(unsafe { select.Select() }.is_err());
    let actions = shared.take();
    assert_eq!(actions.len(), 32);
    assert!(actions
        .iter()
        .all(|action| action.session == 11 && action.key.identity.window == 2));
    shared.hide();
    assert!(unsafe { selection.Select() }.is_err());
    let invoke: IInvokeProvider = second.cast().unwrap();
    assert!(unsafe { invoke.Invoke() }.is_err());
    shared.active.store(12, Ordering::Release);
    assert!(unsafe { first.GetPropertyValue(UIA_NamePropertyId) }.is_err());
    shared.target.close();
    assert!(unsafe {
        provider::root(shared)
            .unwrap()
            .GetPropertyValue(UIA_NamePropertyId)
    }
    .is_err());
}

#[test]
fn optional_native_interfaces_write_null_and_never_return_uninitialized_outputs() {
    let shared = fixture();
    let root = provider::root(shared.clone()).unwrap();
    let host_focus = unsafe { root.GetPropertyValue(UIA_HasKeyboardFocusPropertyId) }.unwrap();
    assert_eq!(
        unsafe { host_focus.Anonymous.Anonymous.vt },
        windows::Win32::System::Variant::VT_EMPTY
    );
    let last: IRawElementProviderFragment = provider::item(shared, 11, 2).unwrap().cast().unwrap();
    unsafe {
        let mut result = std::ptr::dangling_mut::<std::ffi::c_void>();
        (root.vtable().GetPatternProvider)(root.as_raw(), UIA_TextPatternId, &mut result)
            .ok()
            .unwrap();
        assert!(result.is_null());
        result = std::ptr::dangling_mut();
        (last.vtable().Navigate)(last.as_raw(), NavigateDirection_NextSibling, &mut result)
            .ok()
            .unwrap();
        assert!(result.is_null());
        let last: IRawElementProviderSimple = last.cast().unwrap();
        result = std::ptr::dangling_mut();
        (last.vtable().HostRawElementProvider)(last.as_raw(), &mut result)
            .ok()
            .unwrap();
        assert!(result.is_null());
    }
    assert_eq!(
        std::mem::size_of::<abi::SimpleAbi_Vtbl>(),
        std::mem::size_of::<IRawElementProviderSimple_Vtbl>()
    );
    assert_eq!(
        std::mem::size_of::<abi::FragmentAbi_Vtbl>(),
        std::mem::size_of::<IRawElementProviderFragment_Vtbl>()
    );
}

#[test]
fn fragment_root_identity_is_stable_across_threads_without_ownership_cycles() {
    let shared = fixture();
    let lifetime = Arc::downgrade(&shared);
    let root = provider::root(shared.clone()).unwrap();
    let identity = root.cast::<IUnknown>().unwrap();
    for fragment in [root.clone(), provider::item(shared.clone(), 11, 1).unwrap()] {
        let fragment: IRawElementProviderFragment = fragment.cast().unwrap();
        let returned = unsafe { fragment.FragmentRoot() }.unwrap();
        assert_eq!(returned.cast::<IUnknown>().unwrap(), identity);
    }
    let other = shared.clone();
    let from_mta = std::thread::spawn(move || {
        let _com = crate::utils::com::ComApartment::mta().unwrap();
        provider::root(other)
            .unwrap()
            .cast::<IUnknown>()
            .unwrap()
            .as_raw() as usize
    })
    .join()
    .unwrap();
    assert_eq!(from_mta, identity.as_raw() as usize);
    {
        let selected = provider::item(shared.clone(), 11, 1).unwrap();
        let focused = unsafe {
            root.cast::<IRawElementProviderFragmentRoot>()
                .unwrap()
                .GetFocus()
        }
        .unwrap();
        assert_eq!(
            selected.cast::<IUnknown>().unwrap(),
            focused.cast::<IUnknown>().unwrap()
        );
    }
    drop(shared);
    drop(identity);
    assert!(lifetime.upgrade().is_some());
    drop(root);
    assert!(lifetime.upgrade().is_none());
}
