//! The tray emits typed changes, never arbitrary INI keys or values.
use anyhow::Result;

use super::{
    AppNameMode, Config, MonitorFilter, RunLevel, SearchField, SearchMatch, SwitchOrder, Theme,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuickSetting {
    CloseEnabled,
    Theme(Theme),
    Names,
    Badges,
    Preview,
    Minimized,
    Topmost,
    HiddenMinimized,
    Scope(MonitorFilter),
    Order(SwitchOrder),
    SearchEnabled,
    Match(SearchMatch),
    Field(SearchField),
    StartupLevel(RunLevel),
}

impl QuickSetting {
    pub(crate) fn key(self) -> (&'static str, &'static str) {
        match self {
            Self::CloseEnabled => ("window-actions", "close_enable"),
            Self::Theme(_) => ("appearance", "theme"),
            Self::Names => ("appearance", "app_name_mode"),
            Self::Badges => ("switch-apps", "show_badge"),
            Self::Preview => ("preview", "enable"),
            Self::Minimized => ("switch-apps", "ignore_minimal"),
            Self::Topmost => ("switch-apps", "include_topmost"),
            Self::HiddenMinimized => ("switch-apps", "include_hidden_minimized"),
            Self::Scope(_) => ("switch-apps", "monitor_filter"),
            Self::Order(_) => ("switch-apps", "order"),
            Self::SearchEnabled => ("search", "enable"),
            Self::Match(_) => ("search", "match"),
            Self::Field(_) => ("search", "fields"),
            Self::StartupLevel(_) => ("startup", "run_level"),
        }
    }

    pub(crate) fn value(self, config: &Config) -> String {
        let boolean = |value| if value { "yes" } else { "no" }.to_owned();
        match self {
            Self::CloseEnabled => boolean(config.close_enable),
            Self::Theme(_) => config.theme.to_string(),
            Self::Names => config.app_name_mode.to_string(),
            Self::Badges => boolean(config.switch_apps_show_badge),
            Self::Preview => boolean(config.preview_enable),
            Self::Minimized => boolean(config.switch_apps_ignore_minimal),
            Self::Topmost => boolean(config.switch_apps_include_topmost),
            Self::HiddenMinimized => boolean(config.switch_apps_include_hidden_minimized),
            Self::Scope(_) => config.switch_apps_monitor_filter.to_string(),
            Self::Order(_) => config.switch_apps_order.to_string(),
            Self::SearchEnabled => boolean(config.search_enable),
            Self::Match(_) => config.search_match.to_string(),
            Self::Field(_) => config.search_fields.to_string(),
            Self::StartupLevel(_) => config.startup_run_level.to_string(),
        }
    }

    pub(crate) fn changed_value(self, config: &Config) -> Result<String> {
        let mut changed = config.clone();
        match self {
            Self::CloseEnabled => changed.close_enable = !config.close_enable,
            Self::Theme(value) => changed.theme = value,
            Self::Names => {
                changed.app_name_mode = if config.app_name_mode == AppNameMode::Off {
                    AppNameMode::Selected
                } else {
                    AppNameMode::Off
                }
            }
            Self::Badges => changed.switch_apps_show_badge = !config.switch_apps_show_badge,
            Self::Preview => changed.preview_enable = !config.preview_enable,
            Self::Minimized => {
                changed.switch_apps_ignore_minimal = !config.switch_apps_ignore_minimal
            }
            Self::Topmost => {
                changed.switch_apps_include_topmost = !config.switch_apps_include_topmost
            }
            Self::HiddenMinimized => {
                changed.switch_apps_include_hidden_minimized =
                    !config.switch_apps_include_hidden_minimized
            }
            Self::Scope(value) => changed.switch_apps_monitor_filter = value,
            Self::Order(value) => changed.switch_apps_order = value,
            Self::SearchEnabled => changed.search_enable = !config.search_enable,
            Self::Match(value) => changed.search_match = value,
            Self::Field(value) => changed.search_fields = config.search_fields.toggle(value)?,
            Self::StartupLevel(value) => changed.startup_run_level = value,
        }
        Ok(self.value(&changed))
    }

    pub(crate) fn checked(self, config: &Config) -> bool {
        match self {
            Self::CloseEnabled => config.close_enable,
            Self::Theme(value) => config.theme == value,
            Self::Names => config.app_name_mode == AppNameMode::Selected,
            Self::Badges => config.switch_apps_show_badge,
            Self::Preview => config.preview_enable,
            Self::Minimized => !config.switch_apps_ignore_minimal,
            Self::Topmost => config.switch_apps_include_topmost,
            Self::HiddenMinimized => config.switch_apps_include_hidden_minimized,
            Self::Scope(value) => config.switch_apps_monitor_filter == value,
            Self::Order(value) => config.switch_apps_order == value,
            Self::SearchEnabled => config.search_enable,
            Self::Match(value) => config.search_match == value,
            Self::Field(value) => config.search_fields.contains(value),
            Self::StartupLevel(value) => config.startup_run_level == value,
        }
    }
}
