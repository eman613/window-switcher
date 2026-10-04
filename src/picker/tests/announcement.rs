use super::*;
use crate::accessibility::announcement::Announcer;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;

fn fixture() -> PickerWindow {
    let view = PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        ViewKind::Search,
    )
    .unwrap();
    let announcer = Announcer::fixture(view.hwnd);
    announcer.set_visible(true);
    *view.state().announcer.borrow_mut() = Some(announcer);
    view
}

fn take(view: &PickerWindow) -> Option<String> {
    view.state()
        .announcer
        .borrow()
        .as_ref()
        .unwrap()
        .take_pending()
}

#[test]
fn list_navigation_announces_selection_without_moving_edit_focus_and_refresh_stays_silent() {
    let view = fixture();
    view.fixture_rows(vec![row(1, "first"), row(2, "second")], 1)
        .unwrap();
    assert!(take(&view).unwrap().contains("first"));
    let before = unsafe { GetFocus() };
    unsafe {
        SendMessageW(view.controls().edit, WM_KEYDOWN, Some(WPARAM(0x28)), None);
    }
    assert_eq!(view.selected(), Some(1));
    assert_eq!(unsafe { GetFocus() }, before);
    assert!(take(&view).unwrap().contains("second"));
    view.replace_rows(vec![row(1, "first"), row(2, "second")], 1, 2)
        .unwrap();
    assert!(take(&view).is_none());
    view.replace_rows(Vec::new(), 0, 3).unwrap();
    assert_eq!(take(&view).as_deref(), Some(view.text.search_empty_title()));
    view.pending().unwrap();
    view.replace_rows(Vec::new(), 0, 4).unwrap();
    assert!(
        take(&view).is_none(),
        "empty background refresh must not repeat speech"
    );
    view.failure().unwrap();
    assert_eq!(
        take(&view).as_deref(),
        Some(view.text.search_failed_title())
    );
}

#[test]
fn query_composition_cancel_and_hide_invalidate_old_speech() {
    let view = fixture();
    view.fixture_rows(vec![row(1, "old")], 1).unwrap();
    view.state().changed();
    assert!(take(&view).is_none());
    view.state().composing.set(true);
    view.replace_rows(vec![row(2, "composition")], 0, 2)
        .unwrap();
    assert!(take(&view).is_none());
    view.state().composing.set(false);
    view.state().announce();
    view.state().signal(messages::CANCEL);
    assert!(take(&view).is_none());
    view.state().announce();
    view.hide();
    assert!(take(&view).is_none());
    view.state().announce();
    assert!(take(&view).is_none());
}
