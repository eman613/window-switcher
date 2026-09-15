use std::{path::PathBuf, process::Command};

use anyhow::{Context, Result};
use ini::Ini;
use windows::core::w;

use crate::utils::{get_exe_folder, RegKey};

mod document;
mod encoding;
mod file_identity;
mod hotkey;
mod logging;
mod metadata;
mod notifications;
mod parsers;
pub(crate) mod reload;
mod schema;
pub(crate) mod settings;
mod storage;
mod transaction;
mod types;
mod validation;
pub(crate) mod watch;
mod watch_state;

#[cfg(test)]
use hotkey::parse_hotkeys;
pub use hotkey::Hotkey;
pub(crate) use logging::initialize_logging;
pub use logging::prepare_log_file;
pub(crate) use logging::take_log_failure;

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod schema_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod validation_tests;

pub const SWITCH_WINDOWS_HOTKEY_ID: u32 = 1;
pub const SWITCH_APPS_HOTKEY_ID: u32 = 2;
#[cfg(test)]
pub const DEFAULT_BADGE_MAX: u32 = 99;
#[cfg(test)]
pub const DEFAULT_BADGE_COLOR: u32 = 0x4c7094;
#[cfg(test)]
pub const DEFAULT_BADGE_FONT_SIZE: u32 = 12;

#[cfg(test)]
const MIN_BADGE_MAX: u32 = 2;
#[cfg(test)]
const MAX_BADGE_MAX: u32 = 9999;

const DEFAULT_CONFIG: &str = include_str!("../window-switcher.ini");

pub use schema::Config;
pub use types::{
    BatteryPolicy, ForegroundPolicy, InjectedPolicy, Language, MonitorPolicy, RenderScale,
    RunLevel, StartupEnabled, WatchMode,
};

impl Config {
    pub fn load(ini_conf: &Ini) -> Result<Self> {
        schema::load(ini_conf)
    }

    pub fn to_hotkeys(&self) -> Vec<&Hotkey> {
        let mut hotkeys: Vec<&Hotkey> = self
            .switch_windows_hotkey
            .iter()
            .filter(|_| self.switch_windows_enable)
            .collect();
        if self.switch_apps_enable {
            hotkeys.extend(self.switch_apps_hotkey.iter());
        }
        hotkeys
    }

    pub fn to_bool(v: &str) -> Option<bool> {
        match v {
            "yes" | "true" | "on" | "1" => Some(true),
            "no" | "false" | "off" | "0" => Some(false),
            _ => None,
        }
    }

    /// Whether the user has configured app switching to include other desktops.
    /// If the configured value is not a valid bool, the Windows registry will be
    /// used as a fallback.
    pub fn switch_apps_only_current_desktop(&self) -> bool {
        self.switch_apps_only_current_desktop
            .unwrap_or_else(Self::system_switcher_only_current_desktop)
    }

    /// Whether the user has configured window switching to include other desktops.
    /// If the configured value is not a valid bool, the Windows registry will be
    /// used as a fallback.
    pub fn switch_windows_only_current_desktop(&self) -> bool {
        self.switch_windows_only_current_desktop
            .unwrap_or_else(Self::system_switcher_only_current_desktop)
    }

    fn system_switcher_only_current_desktop() -> bool {
        let alt_tab_filter = RegKey::new_hkcu(
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced"),
            w!("VirtualDesktopAltTabFilter"),
        )
        .and_then(|k| k.get_int())
        .unwrap_or(1);

        alt_tab_filter != 0
    }
}

#[derive(Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: PathBuf,
    pub migrated: bool,
    pub(crate) contents: Vec<u8>,
}

pub fn load_config() -> Result<LoadedConfig> {
    storage::load_at(get_config_path()?)
}

pub(crate) fn read_config_bytes(path: &std::path::Path) -> Result<Vec<u8>> {
    transaction::require_no_recovery(path)?;
    storage::read_bytes(path)
}

pub(crate) fn edit_config_file() -> Result<()> {
    let filepath = get_config_path()?;
    debug!("config stage=open-editor");
    Command::new("notepad.exe")
        .arg(&filepath)
        .spawn()
        .with_context(|| {
            format!(
                "无法打开配置文件 '{}'，请检查记事本是否可用",
                filepath.display()
            )
        })?;
    Ok(())
}

pub(crate) fn get_config_path() -> Result<PathBuf> {
    Ok(get_exe_folder()?.join("window-switcher.ini"))
}

#[cfg(test)]
fn parse_badge_max(value: &str) -> Option<u32> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| (MIN_BADGE_MAX..=MAX_BADGE_MAX).contains(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey() {
        assert_eq!(Hotkey::parse("alt + `"), Some(([0x38, 0x38], 0x29)));
        assert_eq!(Hotkey::parse("alt + tab"), Some(([0x38, 0x38], 0x0f)));
    }

    #[test]
    fn test_parse_hotkeys() {
        let hotkeys = parse_hotkeys(1, "test", "alt+` || alt+tab").unwrap();
        assert_eq!(hotkeys.len(), 2);
        assert_eq!(hotkeys[0].modifier, [0x38, 0x38]);
        assert_eq!(hotkeys[0].code, 0x29);
        assert_eq!(hotkeys[1].modifier, [0x38, 0x38]);
        assert_eq!(hotkeys[1].code, 0x0f);

        let hotkeys = parse_hotkeys(1, "test", "alt+`").unwrap();
        assert_eq!(hotkeys.len(), 1);
        assert_eq!(hotkeys[0].modifier, [0x38, 0x38]);
        assert_eq!(hotkeys[0].code, 0x29);
    }

    #[test]
    fn test_badge_defaults_and_limits() {
        let config = Config::load(&Ini::load_from_str("[switch-apps]\n").unwrap()).unwrap();
        assert!(config.switch_apps_show_badge);
        assert_eq!(config.switch_apps_badge_max, DEFAULT_BADGE_MAX);

        assert_eq!(parse_badge_max("2"), Some(2));
        assert_eq!(parse_badge_max("9999"), Some(9999));
        assert_eq!(parse_badge_max("1"), None);
        assert_eq!(parse_badge_max("10000"), None);
        assert_eq!(parse_badge_max("not-a-number"), None);
    }

    #[test]
    fn test_badge_config_values() {
        let ini = Ini::load_from_str("[switch-apps]\nshow_badge = no\nbadge_max = 250\n").unwrap();
        let config = Config::load(&ini).unwrap();
        assert!(!config.switch_apps_show_badge);
        assert_eq!(config.switch_apps_badge_max, 250);
    }
}
