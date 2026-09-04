use std::{collections::HashSet, fs, path::PathBuf, process::Command, str::FromStr};

use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use ini::{Ini, ParseOption};
use log::LevelFilter;
use windows::core::w;

use crate::utils::{get_exe_folder, RegKey};

pub const SWITCH_WINDOWS_HOTKEY_ID: u32 = 1;
pub const SWITCH_APPS_HOTKEY_ID: u32 = 2;

const DEFAULT_CONFIG: &str = include_str!("../window-switcher.ini");

const ICON_SIZE_MIN: i32 = 24;
const ICON_SIZE_MAX: i32 = 256;
const SPACING_MAX: i32 = 256;
const PANEL_EXTENT_MAX: i32 = 32_768;
const GRID_EXTENT_MAX: usize = 256;
const BADGE_MAX_MIN: usize = 2;
const BADGE_MAX_MAX: usize = 9_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorTarget {
    #[default]
    Cursor,
    Foreground,
    Primary,
}

impl FromStr for MonitorTarget {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cursor" => Ok(Self::Cursor),
            "foreground" => Ok(Self::Foreground),
            "primary" => Ok(Self::Primary),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    #[default]
    SingleRow,
    Grid,
    Paged,
}

impl FromStr for LayoutMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "single-row" => Ok(Self::SingleRow),
            "grid" => Ok(Self::Grid),
            "paged" => Ok(Self::Paged),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceConfig {
    pub monitor: MonitorTarget,
    pub use_work_area: bool,
    pub icon_size: i32,
    pub icon_padding: i32,
    pub item_gap: i32,
    pub panel_padding: i32,
    pub max_width: i32,
    pub max_height: i32,
    pub layout: LayoutMode,
    pub max_columns: usize,
    pub max_rows: usize,
    pub show_badge: bool,
    pub badge_max: usize,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            monitor: MonitorTarget::Cursor,
            use_work_area: true,
            icon_size: 64,
            icon_padding: 4,
            item_gap: 8,
            panel_padding: 10,
            max_width: 0,
            max_height: 0,
            layout: LayoutMode::SingleRow,
            max_columns: 0,
            max_rows: 0,
            show_badge: true,
            badge_max: 99,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub trayicon: bool,
    pub run_as_admin: bool,
    pub appearance: AppearanceConfig,
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
    switch_apps_only_current_desktop: Option<bool>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trayicon: true,
            run_as_admin: false,
            appearance: AppearanceConfig::default(),
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
            switch_apps_only_current_desktop: None,
        }
    }
}

impl Config {
    pub fn load(ini_conf: &Ini) -> Result<Self> {
        let mut conf = Config::default();
        if let Some(section) = ini_conf.section(None::<String>) {
            if let Some(v) = section.get("trayicon").and_then(Config::to_bool) {
                conf.trayicon = v;
            }
        }

        if let Some(section) = ini_conf.section(Some("startup")) {
            if let Some(v) = section.get("run_as_admin").and_then(Config::to_bool) {
                conf.run_as_admin = v;
            }
        }

        if let Some(section) = ini_conf.section(Some("appearance")) {
            if let Some(value) = section.get("monitor") {
                conf.appearance.monitor =
                    parse_enum_or_default("appearance.monitor", value, conf.appearance.monitor);
            }
            if let Some(value) = section.get("use_work_area") {
                conf.appearance.use_work_area = parse_bool_or_default(
                    "appearance.use_work_area",
                    value,
                    conf.appearance.use_work_area,
                );
            }
            if let Some(value) = section.get("icon_size") {
                conf.appearance.icon_size = parse_i32_or_default(
                    "appearance.icon_size",
                    value,
                    ICON_SIZE_MIN,
                    ICON_SIZE_MAX,
                    conf.appearance.icon_size,
                );
            }
            if let Some(value) = section.get("icon_padding") {
                conf.appearance.icon_padding = parse_i32_or_default(
                    "appearance.icon_padding",
                    value,
                    0,
                    SPACING_MAX,
                    conf.appearance.icon_padding,
                );
            }
            if let Some(value) = section.get("item_gap") {
                conf.appearance.item_gap = parse_i32_or_default(
                    "appearance.item_gap",
                    value,
                    0,
                    SPACING_MAX,
                    conf.appearance.item_gap,
                );
            }
            if let Some(value) = section.get("panel_padding") {
                conf.appearance.panel_padding = parse_i32_or_default(
                    "appearance.panel_padding",
                    value,
                    0,
                    SPACING_MAX,
                    conf.appearance.panel_padding,
                );
            }
            if let Some(value) = section.get("max_width") {
                conf.appearance.max_width = parse_i32_or_default(
                    "appearance.max_width",
                    value,
                    0,
                    PANEL_EXTENT_MAX,
                    conf.appearance.max_width,
                );
            }
            if let Some(value) = section.get("max_height") {
                conf.appearance.max_height = parse_i32_or_default(
                    "appearance.max_height",
                    value,
                    0,
                    PANEL_EXTENT_MAX,
                    conf.appearance.max_height,
                );
            }
            if let Some(value) = section.get("layout") {
                conf.appearance.layout =
                    parse_enum_or_default("appearance.layout", value, conf.appearance.layout);
            }
            if let Some(value) = section.get("max_columns") {
                conf.appearance.max_columns = parse_usize_or_default(
                    "appearance.max_columns",
                    value,
                    0,
                    GRID_EXTENT_MAX,
                    conf.appearance.max_columns,
                );
            }
            if let Some(value) = section.get("max_rows") {
                conf.appearance.max_rows = parse_usize_or_default(
                    "appearance.max_rows",
                    value,
                    0,
                    GRID_EXTENT_MAX,
                    conf.appearance.max_rows,
                );
            }
            if let Some(value) = section.get("show_badge") {
                conf.appearance.show_badge = parse_bool_or_default(
                    "appearance.show_badge",
                    value,
                    conf.appearance.show_badge,
                );
            }
            if let Some(value) = section.get("badge_max") {
                conf.appearance.badge_max = parse_usize_or_default(
                    "appearance.badge_max",
                    value,
                    BADGE_MAX_MIN,
                    BADGE_MAX_MAX,
                    conf.appearance.badge_max,
                );
            }
        }

        if let Some(section) = ini_conf.section(Some("log")) {
            if let Some(level) = section.get("level").and_then(|v| v.parse().ok()) {
                conf.log_level = level;
            }
            if let Some(path) = section.get("path").map(normalize_path_value) {
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

            if let Some(v) = section.get("blacklist").map(normalize_path_value).map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
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
            if let Some(v) = section.get("override_icons").map(normalize_path_value) {
                conf.switch_apps_override_icons = v
                    .split([',', ';'])
                    .filter_map(|v| {
                        v.trim()
                            .split_once("=")
                            .map(|(k, v)| (k.to_lowercase(), v.to_string()))
                    })
                    .collect();
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub id: u32,
    pub name: String,
    pub modifier: [u32; 2],
    pub code: u32,
}

impl Hotkey {
    pub fn create(id: u32, name: &str, value: &str) -> Result<Self> {
        let (modifier, code) =
            Self::parse(value).ok_or_else(|| anyhow!("Invalid {name} hotkey"))?;
        Ok(Self {
            id,
            name: name.to_string(),
            modifier,
            code,
        })
    }

    pub fn get_modifier(&self) -> u32 {
        self.modifier[0]
    }

    pub fn parse(value: &str) -> Option<([u32; 2], u32)> {
        let value = value
            .to_ascii_lowercase()
            .replace(' ', "")
            .replace("vk_", "");
        let keys: Vec<&str> = value.split('+').collect();
        if keys.len() != 2 {
            return None;
        }
        let modifier = match keys[0] {
            "win" => [0x5b, 0x5c],
            "alt" => [0x38, 0x38],
            "ctrl" => [0x1d, 0x1d],
            _ => {
                return None;
            }
        };
        // see <https://kbdlayout.info/kbdus/overview+scancodes>
        let code = match keys[1] {
            "esc" | "escape" => 0x01,
            "1" | "!" => 0x02,
            "2" | "@" => 0x03,
            "3" | "#" => 0x04,
            "4" | "$" => 0x05,
            "5" | "%" => 0x06,
            "6" | "^" => 0x07,
            "7" | "&" => 0x08,
            "8" | "*" => 0x09,
            "9" | "(" => 0x0a,
            "0" | ")" => 0x0b,
            "-" | "_" | "oem_minus" => 0x0c,
            "+" | "=" | "oem_plus" => 0x0d,
            "bs" | "backspace" => 0x0e,
            "tab" => 0x0f,
            "q" => 0x10,
            "w" => 0x11,
            "e" => 0x12,
            "r" => 0x13,
            "t" => 0x14,
            "y" => 0x15,
            "u" => 0x16,
            "i" => 0x17,
            "o" => 0x18,
            "p" => 0x19,
            "{" | "[" | "oem_4" => 0x1a,
            "}" | "]" | "oem_6" => 0x1b,
            "enter" | "return" => 0x1c,
            "a" => 0x1e,
            "s" => 0x1f,
            "d" => 0x20,
            "f" => 0x21,
            "g" => 0x22,
            "h" => 0x23,
            "j" => 0x24,
            "k" => 0x25,
            "l" => 0x26,
            ":" | ";" | "oem_1" => 0x27,
            "\"" | "'" | "oem_7" => 0x28,
            "~" | "`" | "oem_3" => 0x29,
            "|" | "\\" | "oem_5" => 0x2b,
            "z" => 0x2c,
            "x" => 0x2d,
            "c" => 0x2e,
            "v" => 0x2f,
            "b" => 0x30,
            "n" => 0x31,
            "m" => 0x32,
            "<" | "," | "oem_comma" => 0x33,
            ">" | "." | "oem_period" => 0x34,
            "?" | "/" | "oem_2" => 0x35,
            "space" => 0x39,
            "capslock" => 0x3a,
            "f1" => 0x3b,
            "f2" => 0x3c,
            "f3" => 0x3d,
            "f4" => 0x3e,
            "f5" => 0x3f,
            "f6" => 0x40,
            "f7" => 0x41,
            "f8" => 0x42,
            "f9" => 0x43,
            "f10" => 0x44,
            "numlock" => 0x45,
            "scrolllock" => 0x46,
            "home" => 0x47,
            "up" => 0x48,
            "pageup" => 0x49,
            "left" => 0x4b,
            "right" => 0x4d,
            "end" => 0x4f,
            "down" => 0x50,
            "pagedown" => 0x51,
            "insert" => 0x52,
            "delete" => 0x53,
            "prtsc" | "printscreen" => 0x54,
            "oem_102" => 0x56,
            "f11" => 0x57,
            "f12" => 0x58,
            "menu" => 0x5d,
            _ => return None,
        };
        Some((modifier, code))
    }
}

pub fn load_config() -> Result<Config> {
    let _timer = crate::metrics::StageTimer::new("config_load");
    let filepath = get_config_path()?;
    if !filepath.exists() {
        return Ok(Config::default());
    }
    let opt = ParseOption {
        enabled_escape: false,
        ..Default::default()
    };
    let conf = Ini::load_from_file_opt(&filepath, opt)
        .map_err(|err| anyhow!("Failed to load config file '{}', {err}", filepath.display()))?;
    Config::load(&conf)
}

pub(crate) fn edit_config_file() -> Result<bool> {
    let filepath = get_config_path()?;
    debug!("open config file '{}'", filepath.display());
    if !filepath.exists() {
        fs::write(&filepath, DEFAULT_CONFIG).map_err(|err| {
            anyhow!(
                "Failed to write config file '{}', {err}",
                filepath.display()
            )
        })?;
    }
    let exit = Command::new("notepad.exe")
        .arg(&filepath)
        .spawn()
        .map_err(|err| anyhow!("Failed to open config file '{}', {err}", filepath.display()))?
        .wait()
        .map_err(|err| {
            anyhow!(
                "Failed to close config file '{}', {err}",
                filepath.display()
            )
        })?;

    Ok(exit.success())
}

fn get_config_path() -> Result<PathBuf> {
    let folder = get_exe_folder()?;
    let config_path = folder.join("window-switcher.ini");
    Ok(config_path)
}

fn normalize_path_value(value: &str) -> String {
    value.replace("\\\\", "\\")
}

fn parse_enum_or_default<T>(name: &str, value: &str, default: T) -> T
where
    T: FromStr + Copy,
{
    match value.parse() {
        Ok(parsed) => parsed,
        Err(_) => {
            warn!("invalid {name}={value:?}; using default");
            default
        }
    }
}

fn parse_bool_or_default(name: &str, value: &str, default: bool) -> bool {
    match Config::to_bool(value.trim()) {
        Some(parsed) => parsed,
        None => {
            warn!("invalid {name}={value:?}; using default {default}");
            default
        }
    }
}

fn parse_i32_or_default(name: &str, value: &str, min: i32, max: i32, default: i32) -> i32 {
    match value.trim().parse::<i32>() {
        Ok(parsed) if (min..=max).contains(&parsed) => parsed,
        _ => {
            warn!("invalid {name}={value:?}; expected {min}..={max}, using default {default}");
            default
        }
    }
}

fn parse_usize_or_default(
    name: &str,
    value: &str,
    min: usize,
    max: usize,
    default: usize,
) -> usize {
    match value.trim().parse::<usize>() {
        Ok(parsed) if (min..=max).contains(&parsed) => parsed,
        _ => {
            warn!("invalid {name}={value:?}; expected {min}..={max}, using default {default}");
            default
        }
    }
}

fn parse_hotkeys(id: u32, name: &str, value: &str) -> Result<Vec<Hotkey>> {
    let parts: Vec<&str> = value.split("||").collect();
    let mut hotkeys = vec![];
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        hotkeys.push(Hotkey::create(id, name, part)?);
    }
    if hotkeys.is_empty() {
        return Err(anyhow!("Invalid {name} hotkey"));
    }
    Ok(hotkeys)
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
    fn test_run_as_admin_config() {
        let ini = Ini::load_from_str("[startup]\nrun_as_admin = yes\n").unwrap();
        assert!(Config::load(&ini).unwrap().run_as_admin);

        let ini = Ini::load_from_str("[startup]\nrun_as_admin = no\n").unwrap();
        assert!(!Config::load(&ini).unwrap().run_as_admin);
    }

    #[test]
    fn empty_blacklist_does_not_enable_foreground_monitoring() {
        let ini = Ini::load_from_str("[switch-windows]\nblacklist =\n").unwrap();

        assert!(Config::load(&ini)
            .unwrap()
            .switch_windows_blacklist
            .is_empty());
    }

    #[test]
    fn appearance_settings_are_loaded() {
        let ini = Ini::load_from_str(
            "[appearance]\nmonitor = foreground\nuse_work_area = no\nicon_size = 80\nicon_padding = 6\nitem_gap = 12\npanel_padding = 16\nmax_width = 1200\nmax_height = 800\nlayout = grid\nmax_columns = 8\nmax_rows = 3\nshow_badge = no\nbadge_max = 999\n",
        )
        .unwrap();

        assert_eq!(
            Config::load(&ini).unwrap().appearance,
            AppearanceConfig {
                monitor: MonitorTarget::Foreground,
                use_work_area: false,
                icon_size: 80,
                icon_padding: 6,
                item_gap: 12,
                panel_padding: 16,
                max_width: 1200,
                max_height: 800,
                layout: LayoutMode::Grid,
                max_columns: 8,
                max_rows: 3,
                show_badge: false,
                badge_max: 999,
            }
        );
    }

    #[test]
    fn invalid_appearance_settings_keep_safe_defaults() {
        let ini = Ini::load_from_str(
            "[appearance]\nmonitor = elsewhere\nicon_size = 0\nitem_gap = -1\nlayout = diagonal\nmax_columns = 999\nbadge_max = 1\n",
        )
        .unwrap();

        assert_eq!(
            Config::load(&ini).unwrap().appearance,
            AppearanceConfig::default()
        );
    }
}
