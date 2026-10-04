use super::*;
use windows::core::Interface;

#[test]
fn provider_owner_keeps_explicit_sta_until_its_last_resource_is_released() {
    use windows::Win32::System::Com::{
        CoGetApartmentType, APTTYPE, APTTYPEQUALIFIER, APTTYPEQUALIFIER_IMPLICIT_MTA,
        APTTYPE_MAINSTA, APTTYPE_STA,
    };
    std::thread::spawn(|| {
        let caller = ComApartment::sta().unwrap();
        let announcer = Announcer::fixture(HWND::default());
        drop(caller);
        let mut apartment = APTTYPE::default();
        let mut qualifier = APTTYPEQUALIFIER::default();
        unsafe { CoGetApartmentType(&mut apartment, &mut qualifier) }.unwrap();
        assert!(apartment == APTTYPE_STA || apartment == APTTYPE_MAINSTA);
        assert_ne!(qualifier, APTTYPEQUALIFIER_IMPLICIT_MTA);
        drop(announcer);
        assert!(unsafe { CoGetApartmentType(&mut apartment, &mut qualifier) }.is_err());
    })
    .join()
    .unwrap();
}

#[test]
fn hidden_cancelled_and_retired_notifications_never_survive_as_pending_work() {
    let announcer = Announcer::fixture(HWND::default());
    announcer.say("hidden".into());
    assert!(announcer.take_pending().is_none());
    announcer.set_visible(true);
    announcer.say("not yet emitted".into());
    announcer.suspend();
    announcer.say("not yet emitted".into());
    assert_eq!(
        announcer.take_pending().as_deref(),
        Some("not yet emitted"),
        "cancelled queued speech must be eligible again"
    );
    announcer.suspend();
    announcer.say("not yet emitted".into());
    assert!(
        announcer.take_pending().is_none(),
        "successful handoff must stay deduplicated"
    );
    announcer.say("old result".into());
    announcer.say("current result".into());
    assert_eq!(announcer.take_pending().as_deref(), Some("current result"));
    announcer.say("current result".into());
    assert!(
        announcer.take_pending().is_none(),
        "unchanged refresh must be silent"
    );
    announcer.say("stale query".into());
    announcer.cancel();
    assert!(announcer.take_pending().is_none());
    announcer.say("reopened query".into());
    announcer.set_visible(false);
    assert!(announcer.take_pending().is_none());
    announcer.set_visible(true);
    announcer.say("reopened query".into());
    assert_eq!(announcer.take_pending().as_deref(), Some("reopened query"));
    announcer.retire();
    announcer.set_visible(true);
    announcer.say("destroyed target".into());
    assert!(announcer.take_pending().is_none());
}

#[test]
fn popup_window_pattern_and_name_remain_available_only_for_live_visible_target() {
    let _com = ComApartment::sta().unwrap();
    let announcer = Announcer::fixture(HWND::default());
    assert!(unsafe { announcer.root.GetPropertyValue(UIA_NamePropertyId) }.is_err());
    announcer.set_visible(true);
    let window: IWindowProvider = unsafe { announcer.root.GetPatternProvider(UIA_WindowPatternId) }
        .unwrap()
        .cast()
        .unwrap();
    assert!(!unsafe { window.CanMaximize() }.unwrap().as_bool());
    assert!(!unsafe { window.CanMinimize() }.unwrap().as_bool());
    assert!(unsafe { window.IsTopmost() }.unwrap().as_bool());
    assert!(unsafe { window.SetVisualState(WindowVisualState_Maximized) }.is_err());
    let name =
        BSTR::try_from(&unsafe { announcer.root.GetPropertyValue(UIA_NamePropertyId) }.unwrap())
            .unwrap();
    assert_eq!(name.to_string(), "Search fixture");
    announcer.retire();
    assert!(unsafe { window.Close() }.is_err());
    assert!(unsafe { announcer.root.GetPropertyValue(UIA_NamePropertyId) }.is_err());
}
