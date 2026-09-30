use super::*;
mod native_activation;
use crate::config::Language;
use windows::{
    core::w,
    Win32::{
        Foundation::WPARAM,
        UI::WindowsAndMessaging::{
            SendMessageW, SetWindowTextW, WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
        },
    },
};

fn pending_session() -> (SearchSession, HWND) {
    let mut session = SearchSession::new(
        HWND::default(),
        &Config::default(),
        Arc::new(WindowTarget::new(HWND::default())),
        Text::new(Language::English),
    )
    .unwrap();
    let identity = WindowIdentity::fixture(1);
    let entry = SearchEntry {
        identity,
        executable: "fixture.exe".into(),
        key: IconKey {
            group: "fixture.exe".into(),
            identity,
        },
        elevated: Some(false),
        minimized: false,
        title: "Displayed window".into(),
        app: "Fixture".into(),
    };
    let edit = session
        .window
        .fixture_rows(vec![entry.row(session.text)], 7)
        .unwrap();
    session.results = vec![entry];
    session.displayed_generation = 7;
    session.generation = 8;
    session.pending = true;
    (session, edit)
}

fn key(edit: HWND, value: usize) {
    unsafe {
        SendMessageW(edit, WM_KEYDOWN, Some(WPARAM(value)), None);
    }
}

#[test]
fn enter_during_background_refresh_activates_the_displayed_identity_once() {
    let (mut session, edit) = pending_session();
    key(edit, 0x0d);
    let Some(SearchAction::Activate(entry)) = session.poll().unwrap() else {
        panic!("visible result confirmation was lost during refresh");
    };
    assert_eq!(entry.identity, WindowIdentity::fixture(1));
    assert!(session.poll().unwrap().is_none());
}

#[test]
fn changed_query_and_composition_never_activate_an_old_displayed_result() {
    let (mut session, edit) = pending_session();
    key(edit, 0x0d);
    unsafe {
        SetWindowTextW(edit, w!("changed")).unwrap();
    }
    assert!(session.poll().unwrap().is_none());
    let (mut session, edit) = pending_session();
    key(edit, 0x0d);
    unsafe {
        SendMessageW(edit, WM_IME_STARTCOMPOSITION, None, None);
    }
    assert!(session.poll().unwrap().is_none());
}

#[test]
fn cancellation_and_invalidated_or_stale_rows_take_precedence_over_accept() {
    let (mut session, edit) = pending_session();
    key(edit, 0x0d);
    key(edit, 0x1b);
    assert!(matches!(
        session.poll().unwrap(),
        Some(SearchAction::Cancel)
    ));
    let (mut session, edit) = pending_session();
    key(edit, 0x0d);
    session.invalidate().unwrap();
    assert!(session.poll().unwrap().is_none());
    let (mut session, edit) = pending_session();
    key(edit, 0x0d);
    session.displayed_generation = 9;
    assert!(session.poll().unwrap().is_none());
}
