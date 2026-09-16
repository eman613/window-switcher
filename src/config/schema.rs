//! One declaration owns each setting's typed field, default, parser and consumer.
//! The shipped Chinese template is checked against this declaration in tests.
use std::{collections::HashSet, path::PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use ini::Ini;
use log::LevelFilter;

use super::{
    parsers::*, types::*, Hotkey, SearchFields, SEARCH_HOTKEY_ID, SWITCH_APPS_HOTKEY_ID,
    SWITCH_WINDOWS_HOTKEY_ID,
};

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
    switch_windows_ignore_minimal: bool => ("switch-windows", "ignore_minimal", "no", boolean, "WindowFilter::from_config");
    switch_windows_only_current_desktop: Option<bool> => ("switch-windows", "only_current_desktop", "auto", automatic_bool, "Config::switch_windows_only_current_desktop");
    switch_windows_include_topmost: bool => ("switch-windows", "include_topmost", "no", boolean, "WindowFilter::allows");
    switch_windows_include_tool_windows: bool => ("switch-windows", "include_tool_windows", "no", boolean, "WindowFilter::allows");
    switch_windows_include_untitled: bool => ("switch-windows", "include_untitled", "no", boolean, "WindowFilter::allows");
    switch_windows_min_width: u32 => ("switch-windows", "min_width", "120", |v| integer(v, 0, 4096), "WindowFilter::allows");
    switch_windows_min_height: u32 => ("switch-windows", "min_height", "90", |v| integer(v, 0, 4096), "WindowFilter::allows");
    switch_windows_exclude_titles: HashSet<String> => ("switch-windows", "exclude_titles", "Windows Input Experience", titles, "WindowFilter::allows");
    switch_windows_exclude_processes: HashSet<String> => ("switch-windows", "exclude_processes", "", blacklist, "WindowFilter::allows");
    switch_apps_enable: bool => ("switch-apps", "enable", "no", boolean, "Config::to_hotkeys");
    switch_apps_hotkey: Vec<Hotkey> => ("switch-apps", "hotkey", "alt+tab", |v| hotkeys(v, SWITCH_APPS_HOTKEY_ID, "switch apps", "alt+tab"), "KeyboardListener");
    switch_apps_ignore_minimal: bool => ("switch-apps", "ignore_minimal", "no", boolean, "WindowFilter::from_config");
    switch_apps_override_icons: IndexMap<String, String> => ("switch-apps", "override_icons", "", overrides, "IconLoader::native");
    switch_apps_show_badge: bool => ("switch-apps", "show_badge", "yes", boolean, "App::apply_app_snapshot");
    switch_apps_badge_max: u32 => ("switch-apps", "badge_max", "99", |v| integer(v, 2, 9999), "App::apply_app_snapshot");
    switch_apps_badge_color: u32 => ("switch-apps", "badge_color", "#4C7094", color, "BadgeStyle::from_config");
    switch_apps_badge_text_color: u32 => ("switch-apps", "badge_text_color", "#FFFFFF", color, "BadgeStyle::from_config");
    switch_apps_badge_font_size: u32 => ("switch-apps", "badge_font_size", "12", |v| integer(v, 8, 24), "BadgeStyle::from_config");
    badge_font_family: String => ("switch-apps", "badge_font_family", "Segoe UI", font_family, "FontResources::load");
    badge_font_file: Option<PathBuf> => ("switch-apps", "badge_font_file", "", font_file, "FontResources::load");
    switch_apps_only_current_desktop: Option<bool> => ("switch-apps", "only_current_desktop", "auto", automatic_bool, "Config::switch_apps_only_current_desktop");
    switch_apps_blacklist: HashSet<String> => ("switch-apps", "blacklist", "", blacklist, "ForegroundStatus::allows_apps");
    switch_apps_include_topmost: bool => ("switch-apps", "include_topmost", "no", boolean, "WindowFilter::allows");
    switch_apps_include_tool_windows: bool => ("switch-apps", "include_tool_windows", "no", boolean, "WindowFilter::allows");
    switch_apps_include_untitled: bool => ("switch-apps", "include_untitled", "no", boolean, "WindowFilter::allows");
    switch_apps_min_width: u32 => ("switch-apps", "min_width", "120", |v| integer(v, 0, 4096), "WindowFilter::allows");
    switch_apps_min_height: u32 => ("switch-apps", "min_height", "90", |v| integer(v, 0, 4096), "WindowFilter::allows");
    switch_apps_exclude_titles: HashSet<String> => ("switch-apps", "exclude_titles", "Windows Input Experience", titles, "WindowFilter::allows");
    switch_apps_exclude_processes: HashSet<String> => ("switch-apps", "exclude_processes", "", blacklist, "WindowFilter::allows");
    unknown_foreground: ForegroundPolicy => ("input", "unknown_foreground", "passthrough", str::parse::<ForegroundPolicy>, "ForegroundStatus::allows");
    injected_events: InjectedPolicy => ("input", "injected_events", "handle", str::parse::<InjectedPolicy>, "KeyboardListener");
    search_enable: bool => ("search", "enable", "no", boolean, "Config::to_hotkeys");
    search_hotkey: Hotkey => ("search", "hotkey", "ctrl+space", |v| Hotkey::create(SEARCH_HOTKEY_ID, "search", v), "KeyboardListener");
    search_match: SearchMatch => ("search", "match", "fuzzy", str::parse::<SearchMatch>, "SearchService");
    search_fields: SearchFields => ("search", "fields", "app,title", str::parse::<SearchFields>, "SearchService");
    search_max_results: u32 => ("search", "max_results", "50", |v| integer(v, 10, 200), "SearchService");
    monitor: MonitorPolicy => ("appearance", "monitor", "cursor", str::parse::<MonitorPolicy>, "MonitorSnapshot::capture");
    use_work_area: bool => ("appearance", "use_work_area", "yes", boolean, "MonitorSnapshot::capture");
    panel_width: u32 => ("appearance", "panel_width", "0", |v| integer(v, 0, 32768), "LayoutSnapshot::calculate");
    panel_height: u32 => ("appearance", "panel_height", "0", |v| integer(v, 0, 32768), "LayoutSnapshot::calculate");
    icon_size: u32 => ("appearance", "icon_size", "64", |v| integer(v, 24, 256), "LayoutSnapshot::calculate");
    icon_padding: u32 => ("appearance", "icon_padding", "4", |v| integer(v, 0, 64), "LayoutSnapshot::calculate");
    item_gap: u32 => ("appearance", "item_gap", "0", |v| integer(v, 0, 64), "LayoutSnapshot::calculate");
    panel_padding: u32 => ("appearance", "panel_padding", "10", |v| integer(v, 0, 128), "LayoutSnapshot::calculate");
    max_width: u32 => ("appearance", "max_width", "0", |v| integer(v, 0, 32768), "LayoutSnapshot::calculate");
    max_height: u32 => ("appearance", "max_height", "0", |v| integer(v, 0, 32768), "LayoutSnapshot::calculate");
    max_columns: u32 => ("appearance", "max_columns", "0", |v| integer(v, 0, 128), "LayoutSnapshot::calculate");
    theme: Theme => ("appearance", "theme", "auto", str::parse::<Theme>, "Appearance::resolve");
    background_color: Option<u32> => ("appearance", "background_color", "auto", automatic_color, "Appearance::resolve");
    background_opacity: u32 => ("appearance", "background_opacity", "100", |v| integer(v, 0, 100), "Appearance::resolve");
    icon_background_color: Option<u32> => ("appearance", "icon_background_color", "auto", automatic_color, "Appearance::resolve");
    icon_background_opacity: u32 => ("appearance", "icon_background_opacity", "0", |v| integer(v, 0, 100), "Appearance::resolve");
    selection_color: Option<u32> => ("appearance", "selection_color", "auto", automatic_color, "Appearance::resolve");
    selection_opacity: u32 => ("appearance", "selection_opacity", "100", |v| integer(v, 0, 100), "Appearance::resolve");
    selection_border_color: Option<u32> => ("appearance", "selection_border_color", "auto", automatic_color, "Appearance::resolve");
    selection_border_width: u32 => ("appearance", "selection_border_width", "2", |v| integer(v, 0, 6), "Appearance::resolve");
    panel_corner_radius: Option<u32> => ("appearance", "panel_corner_radius", "auto", |v| radius(v, 256), "Appearance::radii");
    icon_corner_radius: Option<u32> => ("appearance", "icon_corner_radius", "auto", |v| radius(v, 128), "Appearance::radii");
    selection_corner_radius: Option<u32> => ("appearance", "selection_corner_radius", "auto", |v| radius(v, 128), "Appearance::radii");
    app_name_mode: AppNameMode => ("appearance", "app_name_mode", "off", str::parse::<AppNameMode>, "GdiAAPainter::render_allowed");
    app_name_font_family: String => ("appearance", "app_name_font_family", "auto", font_family, "FontResources::load");
    app_name_font_file: Option<PathBuf> => ("appearance", "app_name_font_file", "", font_file, "FontResources::load");
    app_name_font_size: u32 => ("appearance", "app_name_font_size", "14", |v| integer(v, 12, 48), "FontResources::layout");
    app_name_font_weight: u32 => ("appearance", "app_name_font_weight", "400", font_weight, "FontResources::load");
    app_name_font_italic: bool => ("appearance", "app_name_font_italic", "no", boolean, "FontResources::load");
    app_name_text_color: Option<u32> => ("appearance", "app_name_text_color", "auto", automatic_color, "Appearance::resolve");
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
    icon_cache_limit: u32 => ("performance", "icon_cache_limit", "256", |v| integer(v, 16, 1024), "IconCache::new");
    icon_cache_mb: u32 => ("performance", "icon_cache_mb", "32", |v| integer(v, 4, 256), "IconCache::new");
    icon_failure_ttl_ms: u32 => ("performance", "icon_failure_ttl_ms", "3000", |v| integer(v, 100, 60000), "IconCache::new");
    metadata_cache_limit: u32 => ("performance", "metadata_cache_limit", "512", |v| integer(v, 16, 4096), "ProcessMetadataCache::new");
    metadata_ttl_ms: u32 => ("performance", "metadata_ttl_ms", "5000", |v| integer(v, 100, 60000), "ProcessMetadataCache::new");
    icon_query_timeout_ms: u32 => ("performance", "icon_query_timeout_ms", "100", |v| integer(v, 10, 500), "IconLoader::native");
    snapshot_budget_ms: u32 => ("performance", "snapshot_budget_ms", "50", |v| integer(v, 5, 500), "SnapshotService::start");
    render_scale: RenderScale => ("performance", "render_scale", "auto", str::parse::<RenderScale>, "RenderPlan::new");
    render_budget_mb: u32 => ("performance", "render_budget_mb", "64", |v| integer(v, 8, 256), "RenderPlan::new");
    chrome_user_data_dir: Option<PathBuf> => ("browser", "chrome_user_data_dir", "auto", directory, "BrowserPaths::new");
    edge_user_data_dir: Option<PathBuf> => ("browser", "edge_user_data_dir", "auto", directory, "BrowserPaths::new");
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
    crate::layout::LayoutOptions::from_config(&configuration).validate()?;
    super::search::validate_hotkeys(&configuration)?;
    Ok(configuration)
}
