use super::*;
use crate::config::Language;
use windows::Win32::UI::WindowsAndMessaging::{
    GetIconInfo, GetMenuItemID, GetMenuState, GetMenuStringW, IsMenu, ICONINFO, MF_BYPOSITION,
};

#[test]
fn native_menu_preserves_ids_chinese_labels_checks_and_pending_states() {
    let tray = TrayIcon::create().unwrap();
    let text = Text::new(Language::Chinese);
    for (state, busy, checked, disabled) in [
        (StartupState::Ready(false), false, false, false),
        (StartupState::Ready(true), false, true, false),
        (StartupState::Ready(true), true, true, true),
        (StartupState::Saved(true), false, true, true),
        (StartupState::Pending, true, false, true),
        (StartupState::Failed, false, false, true),
    ] {
        let menu = tray.create_menu(state, busy, false, false, text).unwrap();
        for (position, (id, expected)) in [
            (IDM_CONFIGURE, "编辑配置"),
            (IDM_STARTUP, text.startup(state, busy)),
            (IDM_PAUSE, "暂停快捷键"),
            (IDM_EXIT, "退出"),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(unsafe { GetMenuItemID(menu.0, position as i32) }, id);
            let mut label = [0; 128];
            let size =
                unsafe { GetMenuStringW(menu.0, position as u32, Some(&mut label), MF_BYPOSITION) };
            assert_eq!(
                String::from_utf16(&label[..size as usize]).unwrap(),
                expected
            );
        }
        let flags = unsafe { GetMenuState(menu.0, 1, MF_BYPOSITION) };
        assert_eq!(flags & MF_CHECKED.0 != 0, checked);
        assert_eq!(flags & MF_GRAYED.0 != 0, disabled);
        let handle = menu.0;
        drop(menu);
        assert!(!unsafe { IsMenu(handle) }.as_bool());
    }
}

#[test]
fn repeated_menu_creation_releases_native_handles_and_owned_icon() {
    let tray = TrayIcon::create().unwrap();
    for _ in 0..10_000 {
        let menu = tray
            .create_menu(
                StartupState::Ready(false),
                false,
                false,
                false,
                Text::new(Language::English),
            )
            .unwrap();
        let handle = menu.0;
        drop(menu);
        assert!(!unsafe { IsMenu(handle) }.as_bool());
    }
    let icon = tray.data.hIcon;
    drop(tray);
    assert!(unsafe { GetIconInfo(icon, &mut ICONINFO::default()) }.is_err());
}

#[test]
fn pause_menu_reflects_committed_state_and_disables_pending_save() {
    let tray = TrayIcon::create().unwrap();
    let text = Text::new(Language::Chinese);
    for (paused, busy) in [(false, false), (true, false), (false, true), (true, true)] {
        let menu = tray
            .create_menu(StartupState::Ready(false), false, paused, busy, text)
            .unwrap();
        let flags = unsafe { GetMenuState(menu.0, 2, MF_BYPOSITION) };
        assert_eq!(flags & MF_CHECKED.0 != 0, paused);
        assert_eq!(flags & MF_GRAYED.0 != 0, busy);
        let mut label = [0; 128];
        let count = unsafe { GetMenuStringW(menu.0, 2, Some(&mut label), MF_BYPOSITION) };
        assert_eq!(
            String::from_utf16(&label[..count as usize]).unwrap(),
            text.pause(paused, busy)
        );
    }
}

#[test]
fn notifications_never_split_surrogate_pairs_or_lose_terminators() {
    assert_eq!(notification_text::<3>("a𐐀"), [b'a' as u16, 0, 0]);
    assert_eq!(notification_text::<1>("long text"), [0]);
    let text = notification_text::<4>("a𐐀b");
    assert_eq!(String::from_utf16(&text[..3]).unwrap(), "a𐐀");
    assert_eq!(text[3], 0);
}
