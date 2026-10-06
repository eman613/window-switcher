use super::*;
mod quick;
use crate::{
    app::{IDM_APPLY_SETTINGS, IDM_CONFIGURE, IDM_ELEVATE, IDM_EXIT, IDM_PAUSE, IDM_STARTUP},
    config::{Language, Theme},
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetIconInfo, GetMenuItemCount, GetMenuItemID, GetMenuState, GetMenuStringW, GetSubMenu, IsMenu,
    ICONINFO, MF_BYCOMMAND, MF_BYPOSITION, MF_CHECKED, MF_GRAYED,
};

fn menu_state(configuration: &Config) -> TrayMenuState<'_> {
    TrayMenuState {
        configuration,
        startup: StartupState::Ready(false),
        startup_busy: false,
        paused: false,
        pause_busy: false,
        elevated: false,
        restarting: false,
        settings_busy: false,
        pending_settings: false,
        report_busy: false,
        report_available: false,
    }
}

fn containing_menu(menu: HMENU, command: u32) -> Option<HMENU> {
    for index in 0..unsafe { GetMenuItemCount(Some(menu)) } {
        if unsafe { GetMenuItemID(menu, index) } == command {
            return Some(menu);
        }
        let child = unsafe { GetSubMenu(menu, index) };
        if !child.is_invalid() {
            if let Some(found) = containing_menu(child, command) {
                return Some(found);
            }
        }
    }
    None
}

fn flags(menu: HMENU, command: u32) -> u32 {
    unsafe {
        GetMenuState(
            containing_menu(menu, command).expect("menu command missing"),
            command,
            MF_BYCOMMAND,
        )
    }
}

fn label(menu: HMENU, command: u32) -> String {
    let mut buffer = [0; 256];
    let length = unsafe {
        GetMenuStringW(
            containing_menu(menu, command).expect("menu command missing"),
            command,
            Some(&mut buffer),
            MF_BYCOMMAND,
        )
    };
    String::from_utf16(&buffer[..length as usize]).unwrap()
}

#[test]
fn native_menu_preserves_commands_checks_and_unknown_startup_state() {
    let tray = TrayIcon::create().unwrap();
    let config = Config::default();
    let text = Text::new(Language::Chinese);
    for (startup, busy, checked, disabled) in [
        (StartupState::Ready(false), false, false, false),
        (StartupState::Ready(true), false, true, false),
        (StartupState::Ready(true), true, true, true),
        (StartupState::Saved(true), false, true, false),
        (StartupState::Pending, true, false, true),
        (StartupState::Failed, false, false, true),
    ] {
        let mut state = menu_state(&config);
        state.startup = startup;
        state.startup_busy = busy;
        let menu = tray.create_menu(state, text).unwrap();
        for (id, expected) in [
            (IDM_CONFIGURE, "编辑配置"),
            (IDM_STARTUP, text.startup(startup, busy)),
            (IDM_PAUSE, "暂停快捷键"),
            (IDM_EXIT, "退出"),
            (IDM_APPLY_SETTINGS, "应用已保存设置"),
        ] {
            assert_eq!(label(menu.0, id), expected);
        }
        assert_eq!(flags(menu.0, IDM_STARTUP) & MF_CHECKED.0 != 0, checked);
        assert_eq!(flags(menu.0, IDM_STARTUP) & MF_GRAYED.0 != 0, disabled);
        assert_eq!(flags(menu.0, IDM_EXIT) & MF_GRAYED.0, 0);
    }
}

#[test]
fn pending_configuration_and_real_privilege_are_separate_menu_states() {
    let tray = TrayIcon::create().unwrap();
    let config = Config {
        theme: Theme::Dark,
        ..Default::default()
    };
    let text = Text::new(Language::Chinese);
    for elevated in [false, true] {
        let mut state = menu_state(&config);
        state.pending_settings = true;
        state.elevated = elevated;
        let menu = tray.create_menu(state, text).unwrap();
        assert_ne!(flags(menu.0, 102) & MF_CHECKED.0, 0);
        assert_eq!(flags(menu.0, IDM_ELEVATE) & MF_GRAYED.0 != 0, elevated);
        assert_eq!(flags(menu.0, 132) & MF_GRAYED.0 != 0, !elevated);
        assert_eq!(label(menu.0, 132), "以管理员身份开机启动");
        let startup_menu = containing_menu(menu.0, 132).unwrap();
        if !elevated {
            let mut hint = [0; 256];
            let length = unsafe { GetMenuStringW(startup_menu, 2, Some(&mut hint), MF_BYPOSITION) };
            assert_eq!(
                String::from_utf16(&hint[..length as usize]).unwrap(),
                text.startup_elevation_hint()
            );
        }
        let mut caption = [0; 256];
        let length = unsafe { GetMenuStringW(menu.0, 1, Some(&mut caption), MF_BYPOSITION) };
        assert_eq!(
            String::from_utf16(&caption[..length as usize]).unwrap(),
            text.settings_pending()
        );
        for id in [
            100, 101, 102, 103, 104, 105, 110, 111, 112, 113, 114, 115, 116, 117, 120, 121, 122,
            123, 124, 125, 126, 130, 131, 132,
        ] {
            assert!(quick_settings::setting(id).is_some());
            assert!(!label(menu.0, id).is_empty());
        }
    }
    assert!(quick_settings::setting(IDM_EXIT).is_none());
}

#[test]
fn pause_and_pending_save_states_disable_competing_operations() {
    let tray = TrayIcon::create().unwrap();
    let config = Config::default();
    let text = Text::new(Language::Chinese);
    for (paused, busy) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut state = menu_state(&config);
        state.paused = paused;
        state.pause_busy = busy;
        let menu = tray.create_menu(state, text).unwrap();
        assert_eq!(flags(menu.0, IDM_PAUSE) & MF_CHECKED.0 != 0, paused);
        assert_eq!(flags(menu.0, IDM_PAUSE) & MF_GRAYED.0 != 0, busy);
        assert_eq!(label(menu.0, IDM_PAUSE), text.pause(paused, busy));
        assert_eq!(flags(menu.0, 101) & MF_GRAYED.0 != 0, busy);
    }
}

#[test]
fn repeated_menu_creation_releases_submenus_and_the_owned_icon() {
    let tray = TrayIcon::create().unwrap();
    let config = Config::default();
    for _ in 0..10_000 {
        let menu = tray
            .create_menu(menu_state(&config), Text::new(Language::English))
            .unwrap();
        let submenu = containing_menu(menu.0, 101).unwrap();
        let root = menu.0;
        drop(menu);
        assert!(!unsafe { IsMenu(root) }.as_bool());
        assert!(!unsafe { IsMenu(submenu) }.as_bool());
    }
    let icon = tray.data.hIcon;
    drop(tray);
    assert!(unsafe { GetIconInfo(icon, &mut ICONINFO::default()) }.is_err());
}

#[test]
fn notifications_never_split_surrogate_pairs_or_lose_terminators() {
    assert_eq!(notification_text::<3>("a𐐀"), [b'a' as u16, 0, 0]);
    assert_eq!(notification_text::<1>("long text"), [0]);
    let text = notification_text::<4>("a𐐀b");
    assert_eq!(String::from_utf16(&text[..3]).unwrap(), "a𐐀");
    assert_eq!(text[3], 0);
}
