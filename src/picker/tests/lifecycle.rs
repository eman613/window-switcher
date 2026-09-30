use super::*;

#[test]
fn owner_destruction_retires_owned_pickers_and_releases_rust_owners() {
    let target = Arc::new(WindowTarget::new(HWND::default()));
    let text = Text::new(Language::English);
    let owner =
        PickerWindow::create(HWND::default(), target.clone(), text, ViewKind::Details).unwrap();
    let mut search =
        PickerWindow::create(owner.hwnd, target.clone(), text, ViewKind::Search).unwrap();
    let details =
        PickerWindow::create(owner.hwnd, target.clone(), text, ViewKind::Details).unwrap();
    search.state().visible.set(true);
    search.state().accept.set(Some((1, 0)));
    drop(owner);
    for window in [&search, &details] {
        assert!(!unsafe { IsWindow(Some(window.hwnd)) }.as_bool());
        assert!(!window.state().alive.get());
        assert!(!window.visible());
        assert!(window.take_events().accept.is_none());
        assert!(window.selected().is_none());
        assert!(window.query().is_err());
        window.hide();
    }
    assert!(search.layout().is_err());
    drop(search);
    drop(details);
    assert_eq!(Arc::strong_count(&target), 1, "retired picker state leaked");
}

#[test]
fn direct_native_destruction_clears_userdata_and_drop_does_not_leak() {
    let target = Arc::new(WindowTarget::new(HWND::default()));
    let window = PickerWindow::create(
        HWND::default(),
        target.clone(),
        Text::new(Language::English),
        ViewKind::Search,
    )
    .unwrap();
    unsafe { DestroyWindow(window.hwnd) }.unwrap();
    assert!(!window.state().alive.get());
    assert_eq!(crate::utils::get_window_user_data(window.hwnd), 0);
    drop(window);
    assert_eq!(Arc::strong_count(&target), 1);
}
