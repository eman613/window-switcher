use super::*;
use crate::{
    config::{Config, Language},
    localization::Text,
    picker::ViewKind,
    window_target::WindowTarget,
};
use std::sync::Arc;
use windows::Win32::{Foundation::WPARAM, UI::WindowsAndMessaging::LB_SETCURSEL};
#[test]
fn confirmation_is_local_explicit_and_rejects_changed_selection_or_epoch() {
    let _com = crate::utils::com::ComApartment::sta().unwrap();
    let window = PickerWindow::create(
        HWND::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
        ViewKind::Details,
    )
    .unwrap();
    let state = window.state();
    state.visible.set(true);
    window.replace(&["one".into(), "two".into()], 0, 1).unwrap();
    window
        .update_close_targets(
            vec![
                (WindowIdentity::fixture(1), "one".into()),
                (WindowIdentity::fixture(2), "two".into()),
            ],
            || Ok(()),
        )
        .unwrap();
    command(state, CLOSE);
    assert!(!confirming(state));
    assert!(state.close.request.get().is_none());
    state.close.configure(&Config {
        close_enable: true,
        ..Default::default()
    });
    command(state, CLOSE);
    assert!(confirming(state));
    assert!(state.close.request.get().is_none());
    assert!(escape(state));
    assert!(!confirming(state));
    command(state, CLOSE);
    unsafe { SendMessageW(state.list.get(), LB_SETCURSEL, Some(WPARAM(1)), None) };
    command(state, YES);
    assert!(state.close.request.get().is_none());
    command(state, CLOSE);
    state.epoch.set(2);
    command(state, YES);
    assert!(state.close.request.get().is_none());
    command(state, CLOSE);
    command(state, YES);
    command(state, YES);
    assert_eq!(
        window.take_events().close,
        Some((2, 1, WindowIdentity::fixture(2)))
    );
    command(state, CLOSE);
    state.changed();
    command(state, YES);
    assert!(state.close.request.get().is_none());
    state.busy.set(false);
    state.close.configure(&Config {
        close_enable: true,
        close_confirm: false,
        ..Default::default()
    });
    command(state, CLOSE);
    command(state, CLOSE);
    assert_eq!(
        window.take_events().close,
        Some((2, 1, WindowIdentity::fixture(2)))
    );
    assert!(window.take_events().close.is_none());
    press(state, state.close.close.get());
    state.epoch.set(3);
    native_command(state, CLOSE);
    assert!(window.take_events().close.is_none());
    state.composing.set(true);
    command(state, CLOSE);
    assert!(state.close.request.get().is_none());
    window.hide();
    command(state, CLOSE);
    assert!(state.close.request.get().is_none());
}

#[test]
fn background_refresh_rebinds_confirmation_but_never_a_mouse_press() {
    let _com = crate::utils::com::ComApartment::sta().unwrap();
    for kind in [ViewKind::Details, ViewKind::Search] {
        let window = PickerWindow::create(
            HWND::default(),
            Arc::new(WindowTarget::new(HWND::default())),
            Text::new(Language::English),
            kind,
        )
        .unwrap();
        let state = window.state();
        state.visible.set(true);
        state.close.configure(&Config {
            close_enable: true,
            ..Default::default()
        });
        let first = WindowIdentity::fixture(1);
        let second = WindowIdentity::fixture(2);
        window
            .update_close_targets(vec![(first, "one".into()), (second, "two".into())], || {
                window.replace(&["one".into(), "two".into()], 0, 1)
            })
            .unwrap();
        command(state, CLOSE);
        press(state, state.close.yes.get());
        for epoch in 2..10 {
            window
                .update_close_targets(vec![(second, "two".into()), (first, "one".into())], || {
                    command(state, YES); // A reentrant callback cannot close a partial update.
                    assert!(state.close.request.get().is_none());
                    window.replace(&["two".into(), "one".into()], 1, epoch)
                })
                .unwrap();
            assert_eq!(state.close.pending.get(), Some((epoch, 1, first)));
        }
        native_command(state, YES);
        assert!(state.close.request.get().is_none());
        assert!(confirming(state));
        command(state, YES);
        assert_eq!(window.take_events().close, Some((9, 1, first)));
        command(state, CLOSE);
        window
            .update_close_targets(vec![(second, "two".into())], || {
                window.replace(&["two".into()], 0, 10)
            })
            .unwrap();
        assert!(!confirming(state));
        command(state, YES);
        assert!(state.close.request.get().is_none());
        command(state, CLOSE);
        let failed = window.update_close_targets(vec![(second, "two".into())], || {
            Err(anyhow::anyhow!("fixture failure"))
        });
        assert!(failed.is_err());
        assert!(!confirming(state));
    }
}
