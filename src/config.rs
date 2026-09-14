use std::{collections::HashSet, path::PathBuf, process::Command};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use ini::Ini;
use log::LevelFilter;
use windows::core::w;

use crate::utils::{get_exe_folder, RegKey};

mod document;
mod encoding;
mod hotkey;
mod storage;
mod validation;
pub(crate) mod watch;

use hotkey::parse_hotkeys;
pub use hotkey::Hotkey;
pub use storage::prepare_log_file;

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod validation_tests;

pub const SWITCH_WINDOWS_HOTKEY_ID: u32 = 1;
pub const SWITCH_APPS_HOTKEY_ID: u32 = 2;
pub const DEFAULT_BADGE_MAX: u32 = 99;
pub const DEFAULT_BADGE_COLOR: u32 = 0x4c7094;
pub const DEFAULT_BADGE_TEXT_COLOR: u32 = 0xffffff;
pub const DEFAULT_BADGE_FONT_SIZE: u32 = 12;
pub const DEFAULT_RESTART_DELAY_MS: u32 = 1000;
const MIN_BADGE_MAX: u32 = 2;
const MAX_BADGE_MAX: u32 = 9999;

const DEFAULT_CONFIG: &str = include_str!("../window-switcher.ini");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub trayicon: bool,
    pub auto_restart: bool,
    pub restart_delay_ms: u32,
    pub log_level: LevelFilter,
    pub log_file: Option<PathBuf>,
    pub switch_windows_hotkey: Vec<Hotkey>,
    pub switch_windows_blacklist: HashSet<String>,
    pub switch_windows_ignore_minimal: bool,
    switch_windows_only_current_desktop: Option<bool>,
    pub switch_apps_enable: bool,
    pub switch_apps_hotkey: Vec<Hotkey>,
    pub switch_apps_ignore_minimal: bool,
    pub switch_apps_override_icons: IndexMap<String, String>,
    pub switch_apps_show_badge: bool,
    pub switch_apps_badge_max: u32,
    pub switch_apps_badge_color: u32,
    pub switch_apps_badge_text_color: u32,
    pub switch_apps_badge_font_size: u32,
    switch_apps_only_current_desktop: Option<bool>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trayicon: true,
            auto_restart: true,
            restart_delay_ms: DEFAULT_RESTART_DELAY_MS,
            log_level: LevelFilter::Info,
            log_file: None,
            switch_windows_hotkey: vec![Hotkey::create(
                SWITCH_WINDOWS_HOTKEY_ID,
                "switch windows",
                "alt + `",
            )
            .unwrap()],
            switch_windows_blacklist: Default::default(),
            switch_windows_ignore_minimal: false,
            switch_windows_only_current_desktop: None,
            switch_apps_enable: false,
            switch_apps_hotkey: vec![Hotkey::create(
                SWITCH_APPS_HOTKEY_ID,
                "switch apps",
                "alt + tab",
            )
            .unwrap()],
            switch_apps_ignore_minimal: false,
            switch_apps_override_icons: Default::default(),
            switch_apps_show_badge: true,
            switch_apps_badge_max: DEFAULT_BADGE_MAX,
            switch_apps_badge_color: DEFAULT_BADGE_COLOR,
            switch_apps_badge_text_color: DEFAULT_BADGE_TEXT_COLOR,
            switch_apps_badge_font_size: DEFAULT_BADGE_FONT_SIZE,
            switch_apps_only_current_desktop: None,
        }
    }
}

impl Config {
    pub fn load(ini_conf: &Ini) -> Result<Self> {
        validation::validate_values(ini_conf)?;
        let mut conf = Config::default();
        if let Some(section) = ini_conf.section(None::<String>) {
            if let Some(v) = section.get("trayicon").and_then(Config::to_bool) {
                conf.trayicon = v;
            }
            if let Some(v) = section.get("auto_restart").and_then(Config::to_bool) {
                conf.auto_restart = v;
            }
            if let Some(v) = section.get("restart_delay_ms").and_then(|v| v.parse().ok()) {
                conf.restart_delay_ms = v;
            }
        }

        if let Some(section) = ini_conf.section(Some("log")) {
            if let Some(level) = section.get("level").and_then(|v| v.parse().ok()) {
                conf.log_level = level;
            }
            if let Some(path) = section.get("path") {
                if !path.trim().is_empty() {
                    let mut path = PathBuf::from(path);
                    if !path.is_absolute() {
                        let parent = get_exe_folder()?;
                        path = parent.join(path);
                    }
                    conf.log_file = Some(path);
                }
            }
        }

        if let Some(section) = ini_conf.section(Some("switch-windows")) {
            if let Some(v) = section.get("hotkey") {
                if !v.trim().is_empty() {
                    conf.switch_windows_hotkey =
                        parse_hotkeys(SWITCH_WINDOWS_HOTKEY_ID, "switch windows", v)?;
                }
            }

            if let Some(v) = section.get("blacklist").map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .collect()
            }) {
                conf.switch_windows_blacklist = v;
            }
            if let Some(v) = section.get("ignore_minimal").and_then(Config::to_bool) {
                conf.switch_windows_ignore_minimal = v;
            }
            if let Some(v) = section
                .get("only_current_desktop")
                .and_then(Config::to_bool)
            {
                conf.switch_windows_only_current_desktop = Some(v);
            }
        }
        if let Some(section) = ini_conf.section(Some("switch-apps")) {
            if let Some(v) = section.get("enable").and_then(Config::to_bool) {
                conf.switch_apps_enable = v;
            }
            if let Some(v) = section.get("hotkey") {
                if !v.trim().is_empty() {
                    conf.switch_apps_hotkey =
                        parse_hotkeys(SWITCH_APPS_HOTKEY_ID, "switch apps", v)?;
                }
            }
            if let Some(v) = section.get("ignore_minimal").and_then(Config::to_bool) {
                conf.switch_apps_ignore_minimal = v;
            }
            if let Some(v) = section.get("override_icons") {
                conf.switch_apps_override_icons = v
                    .split([',', ';'])
                    .filter_map(|v| {
                        v.trim()
                            .split_once("=")
                            .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
                    })
                    .collect();
            }
            if let Some(v) = section.get("show_badge").and_then(Config::to_bool) {
                conf.switch_apps_show_badge = v;
            }
            if let Some(v) = section.get("badge_max").and_then(parse_badge_max) {
                conf.switch_apps_badge_max = v;
            }
            if let Some(v) = section.get("badge_color").and_then(validation::parse_color) {
                conf.switch_apps_badge_color = v;
            }
            if let Some(v) = section
                .get("badge_text_color")
                .and_then(validation::parse_color)
            {
                conf.switch_apps_badge_text_color = v;
            }
            if let Some(v) = section.get("badge_font_size").and_then(|v| v.parse().ok()) {
                conf.switch_apps_badge_font_size = v;
            }

            if let Some(v) = section
                .get("only_current_desktop")
                .and_then(Config::to_bool)
            {
                conf.switch_apps_only_current_desktop = Some(v);
            }
        }
        Ok(conf)
    }

    pub fn to_hotkeys(&self) -> Vec<&Hotkey> {
        let mut hotkeys: Vec<&Hotkey> = self.switch_windows_hotkey.iter().collect();
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

pub struct LoadedConfig {
    pub config: Config,
    pub path: PathBuf,
    pub migrated: bool,
    pub(crate) contents: Vec<u8>,
}

pub fn load_config() -> Result<LoadedConfig> {
    storage::load_at(get_config_path()?)
}

pub(crate) fn edit_config_file() -> Result<()> {
    let filepath = get_config_path()?;
    debug!("open config file '{}'", filepath.display());
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

fn get_config_path() -> Result<PathBuf> {
    Ok(get_exe_folder()?.join("window-switcher.ini"))
}

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
