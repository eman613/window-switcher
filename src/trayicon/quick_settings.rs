use anyhow::Result;
use windows::{
    core::HSTRING,
    Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
        MF_UNCHECKED,
    },
};

use super::{Menu, TrayMenuState};
use crate::{
    app::{IDM_APPLY_SETTINGS, IDM_CONFIGURE, IDM_ELEVATE, IDM_EXIT, IDM_PAUSE, IDM_STARTUP},
    config::{
        quick::QuickSetting, MonitorFilter, RunLevel, SearchField, SearchMatch, StartupEnabled,
        SwitchOrder, Theme,
    },
    localization::Text,
    startup::StartupState,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickSettingsGroup {
    Appearance,
    Windows,
    Search,
    Startup,
}

use QuickSetting as Setting;
use QuickSettingsGroup as Group;

const ITEMS: &[(u32, Group, Setting)] = &[
    (100, Group::Appearance, Setting::Theme(Theme::Auto)),
    (101, Group::Appearance, Setting::Theme(Theme::Light)),
    (102, Group::Appearance, Setting::Theme(Theme::Dark)),
    (103, Group::Appearance, Setting::Names),
    (104, Group::Appearance, Setting::Badges),
    (105, Group::Appearance, Setting::Preview),
    (110, Group::Windows, Setting::Minimized),
    (111, Group::Windows, Setting::Topmost),
    (112, Group::Windows, Setting::HiddenMinimized),
    (113, Group::Windows, Setting::Scope(MonitorFilter::All)),
    (114, Group::Windows, Setting::Scope(MonitorFilter::Panel)),
    (
        115,
        Group::Windows,
        Setting::Scope(MonitorFilter::Foreground),
    ),
    (116, Group::Windows, Setting::Order(SwitchOrder::Existing)),
    (117, Group::Windows, Setting::Order(SwitchOrder::Mru)),
    (120, Group::Search, Setting::SearchEnabled),
    (121, Group::Search, Setting::Match(SearchMatch::Fuzzy)),
    (122, Group::Search, Setting::Match(SearchMatch::Contains)),
    (123, Group::Search, Setting::Match(SearchMatch::Prefix)),
    (124, Group::Search, Setting::Field(SearchField::App)),
    (125, Group::Search, Setting::Field(SearchField::Title)),
    (126, Group::Search, Setting::Field(SearchField::Exe)),
    (
        130,
        Group::Startup,
        Setting::StartupLevel(RunLevel::Inherit),
    ),
    (
        131,
        Group::Startup,
        Setting::StartupLevel(RunLevel::Standard),
    ),
    (
        132,
        Group::Startup,
        Setting::StartupLevel(RunLevel::Highest),
    ),
];

pub(crate) fn setting(command: u32) -> Option<QuickSetting> {
    ITEMS
        .iter()
        .find(|(id, _, _)| *id == command)
        .map(|(_, _, setting)| *setting)
}

impl Menu {
    fn new() -> Result<Self> {
        Ok(Self(unsafe { CreatePopupMenu() }?))
    }

    fn item(&self, id: u32, label: &str, checked: bool, disabled: bool) -> Result<()> {
        let flags = MF_STRING
            | if checked { MF_CHECKED } else { MF_UNCHECKED }
            | if disabled { MF_GRAYED } else { MF_UNCHECKED };
        unsafe { AppendMenuW(self.0, flags, id as usize, &HSTRING::from(label)) }?;
        Ok(())
    }

    fn separator(&self) -> Result<()> {
        unsafe { AppendMenuW(self.0, MF_SEPARATOR, 0, None) }?;
        Ok(())
    }

    fn submenu(&self, label: &str, child: Menu, disabled: bool) -> Result<()> {
        unsafe {
            AppendMenuW(
                self.0,
                MF_STRING | MF_POPUP | if disabled { MF_GRAYED } else { MF_UNCHECKED },
                child.0 .0 as usize,
                &HSTRING::from(label),
            )
        }?;
        // DestroyMenu on the parent owns all successfully attached submenus.
        std::mem::forget(child);
        Ok(())
    }
}

fn group_menu(group: Group, state: TrayMenuState<'_>, text: Text, busy: bool) -> Result<Menu> {
    let menu = Menu::new()?;
    if group == Group::Startup {
        let checked = match state.configuration.startup_enabled {
            StartupEnabled::Auto => matches!(
                state.startup,
                StartupState::Ready(true) | StartupState::Saved(true)
            ),
            StartupEnabled::Yes => true,
            StartupEnabled::No => false,
        };
        menu.item(
            IDM_STARTUP,
            text.startup(state.startup, state.startup_busy),
            checked,
            busy || !matches!(
                state.startup,
                StartupState::Ready(_) | StartupState::Saved(_)
            ),
        )?;
        menu.item(0, text.startup_effective(state.startup), false, true)?;
        if !state.elevated {
            menu.item(0, text.startup_elevation_hint(), false, true)?;
        }
    }
    for &(id, item_group, setting) in ITEMS {
        if item_group != group {
            continue;
        }
        if matches!(
            setting,
            Setting::Names
                | Setting::Scope(MonitorFilter::All)
                | Setting::Order(SwitchOrder::Existing)
                | Setting::Match(SearchMatch::Fuzzy)
                | Setting::Field(SearchField::App)
                | Setting::StartupLevel(RunLevel::Inherit)
        ) {
            menu.separator()?;
        }
        let disabled =
            busy || (setting == Setting::StartupLevel(RunLevel::Highest) && !state.elevated);
        menu.item(
            id,
            text.quick_setting(setting),
            setting.checked(state.configuration),
            disabled,
        )?;
    }
    Ok(menu)
}

pub(super) fn build(state: TrayMenuState<'_>, text: Text) -> Result<Menu> {
    let menu = Menu::new()?;
    let busy = state.restarting || state.settings_busy || state.startup_busy || state.pause_busy;
    menu.item(0, text.privilege(state.elevated), false, true)?;
    if state.settings_busy {
        menu.item(0, text.settings_busy(), false, true)?;
    } else if state.pending_settings {
        menu.item(0, text.settings_pending(), false, true)?;
    }
    menu.item(
        IDM_ELEVATE,
        text.elevate(state.restarting),
        false,
        busy || state.elevated,
    )?;
    menu.separator()?;
    for group in [
        Group::Appearance,
        Group::Windows,
        Group::Search,
        Group::Startup,
    ] {
        menu.submenu(
            text.quick_group(group),
            group_menu(group, state, text, busy)?,
            busy,
        )?;
    }
    menu.separator()?;
    menu.item(
        IDM_PAUSE,
        text.pause(state.paused, state.pause_busy),
        state.paused,
        busy,
    )?;
    menu.item(IDM_APPLY_SETTINGS, text.apply_settings(), false, busy)?;
    menu.item(IDM_CONFIGURE, text.configure(), false, false)?;
    menu.separator()?;
    menu.item(IDM_EXIT, text.exit(), false, false)?;
    Ok(menu)
}
