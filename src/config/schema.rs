//! One declaration owns each setting's typed field, default, parser and consumer.
//! The shipped Chinese template is checked against this declaration in tests.
use std::{collections::HashSet, path::PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use ini::Ini;
use log::LevelFilter;

use super::{parsers::*, types::*, Hotkey, SWITCH_APPS_HOTKEY_ID, SWITCH_WINDOWS_HOTKEY_ID};

pub(super) struct Setting {
    pub(super) section: &'static str,
    pub(super) key: &'static str,
    pub(super) default: &'static str,
    #[cfg(test)]
    pub(super) consumer: &'static str,
    apply: fn(&mut Config, &str) -> Result<()>,
}

macro_rules! settings {
    ($($field:ident : $kind:ty => ($section:literal, $key:literal, $default:literal, $parser:expr, $consumer:literal);)+) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Config { $(pub $field: $kind,)+ }

        impl Default for Config {
            fn default() -> Self {
                Self { $($field: ($parser)($default).expect(concat!("invalid built-in default: ", $section, ".", $key)),)+ }
            }
        }

        pub(super) const SETTINGS: &[Setting] = &[
            $(Setting {
                section: $section, key: $key, default: $default,
                #[cfg(test)]
                consumer: $consumer,
                apply: |configuration, value| {
                    configuration.$field = ($parser)(value)?;
                    Ok(())
                },
            },)+
        ];
    };
}

settings! {
    trayicon: bool => ("", "trayicon", "yes", boolean, "App::start_services");
    auto_restart: bool => ("", "auto_restart", "yes", boolean, "App::start_services");
    restart_delay_ms: u32 => ("", "restart_delay_ms", "1000", |v| integer(v, 200, 10000), "ConfigWatcher::start");
    switch_windows_enable: bool => ("switch-windows", "enable", "yes", boolean, "Config::to_hotkeys");
    switch_windows_hotkey: Vec<Hotkey> => ("switch-windows", "hotkey", "alt+`", |v| hotkeys(v, SWITCH_WINDOWS_HOTKEY_ID, "switch windows", "alt+`"), "KeyboardListener");
    switch_windows_blacklist: HashSet<String> => ("switch-windows", "blacklist", "", blacklist, "ForegroundWatcher::init");
    switch_windows_ignore_minimal: bool => ("switch-windows", "ignore_minimal", "no", boolean, "App::switch_windows");
    switch_windows_only_current_desktop: Option<bool> => ("switch-windows", "only_current_desktop", "auto", automatic_bool, "Config::switch_windows_only_current_desktop");
    switch_apps_enable: bool => ("switch-apps", "enable", "no", boolean, "Config::to_hotkeys");
    switch_apps_hotkey: Vec<Hotkey> => ("switch-apps", "hotkey", "alt+tab", |v| hotkeys(v, SWITCH_APPS_HOTKEY_ID, "switch apps", "alt+tab"), "KeyboardListener");
    switch_apps_ignore_minimal: bool => ("switch-apps", "ignore_minimal", "no", boolean, "App::switch_apps");
    switch_apps_override_icons: IndexMap<String, String> => ("switch-apps", "override_icons", "", overrides, "get_app_icon");
    switch_apps_show_badge: bool => ("switch-apps", "show_badge", "yes", boolean, "App::switch_apps");
    switch_apps_badge_max: u32 => ("switch-apps", "badge_max", "99", |v| integer(v, 2, 9999), "App::switch_apps");
    switch_apps_badge_color: u32 => ("switch-apps", "badge_color", "#4C7094", color, "BadgeStyle::from_config");
    switch_apps_badge_text_color: u32 => ("switch-apps", "badge_text_color", "#FFFFFF", color, "BadgeStyle::from_config");
    switch_apps_badge_font_size: u32 => ("switch-apps", "badge_font_size", "12", |v| integer(v, 8, 24), "BadgeStyle::from_config");
    switch_apps_only_current_desktop: Option<bool> => ("switch-apps", "only_current_desktop", "auto", automatic_bool, "Config::switch_apps_only_current_desktop");
    startup_enabled: StartupEnabled => ("startup", "enabled", "auto", str::parse::<StartupEnabled>, "Startup::start");
    startup_run_level: RunLevel => ("startup", "run_level", "inherit", str::parse::<RunLevel>, "Startup::start");
    startup_battery_policy: BatteryPolicy => ("startup", "battery_policy", "inherit", str::parse::<BatteryPolicy>, "Startup::start");
    startup_command_timeout_ms: u32 => ("startup", "command_timeout_ms", "5000", |v| integer(v, 500, 30000), "Startup::start");
    config_watch_mode: WatchMode => ("config", "watch_mode", "auto", str::parse::<WatchMode>, "ConfigWatcher::start");
    config_poll_interval_ms: u32 => ("config", "poll_interval_ms", "250", |v| integer(v, 100, 60000), "ConfigWatcher::start");
    config_restart_timeout_ms: u32 => ("config", "restart_timeout_ms", "10000", |v| integer(v, 1000, 60000), "RestartController::start");
    config_retry_delay_ms: u32 => ("config", "retry_delay_ms", "1000", |v| integer(v, 200, 30000), "ConfigWatcher::start");
    config_retry_limit: u32 => ("config", "retry_limit", "3", |v| integer(v, 0, 10), "ConfigWatcher::start");
    language: Language => ("localization", "language", "zh-CN", str::parse::<Language>, "Text::new");
    log_level: LevelFilter => ("log", "level", "info", level, "initialize_logging");
    log_file: Option<PathBuf> => ("log", "path", "", log_path, "initialize_logging");
    log_max_file_mb: u32 => ("log", "max_file_mb", "10", |v| integer(v, 1, 100), "RotatingLog::new");
    log_retained_files: u32 => ("log", "retained_files", "3", |v| integer(v, 0, 20), "RotatingLog::new");
    metrics_enabled: bool => ("performance", "metrics_enabled", "no", boolean, "Diagnostics::new");
    metrics_interval_s: u32 => ("performance", "metrics_interval_s", "60", |v| integer(v, 1, 3600), "Diagnostics::tick");
}

impl Setting {
    fn set(&self, configuration: &mut Config, value: &str) -> Result<()> {
        (self.apply)(configuration, value).with_context(|| {
            format!(
                "[{}] {} 无效；默认值 {:?}，原值未修改",
                self.section, self.key, self.default
            )
        })
    }
}

pub(super) fn load(ini: &Ini) -> Result<Config> {
    let mut configuration = Config::default();
    // Validate every duplicate, then use Ini's existing first-value precedence.
    for (section, properties) in ini {
        for (key, value) in properties {
            if let Some(setting) = SETTINGS.iter().find(|setting| {
                setting.section == section.unwrap_or_default() && setting.key == key
            }) {
                setting.set(&mut configuration, value)?;
            }
        }
    }
    for setting in SETTINGS {
        let section = (!setting.section.is_empty()).then_some(setting.section);
        if let Some(value) = ini.get_from(section, setting.key) {
            setting.set(&mut configuration, value)?;
        }
    }
    Ok(configuration)
}
