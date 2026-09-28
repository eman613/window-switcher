use crate::{
    config::{Config, MonitorFilter},
    keyboard::state::SwitchKind,
    monitor_scope::MonitorScope,
    utils,
};
use std::collections::HashSet;
use windows::Win32::Foundation::HWND;

#[derive(Clone, Copy, Debug)]
pub(super) enum FilterRejection {
    Scope,
    Invisible,
    Minimized,
    Tool,
    Topmost,
    Cloaked,
    Geometry,
    Title,
    Process,
    Elevated,
    Metadata,
    Identity,
}

pub(super) const FILTER_REJECTION_COUNT: usize = FilterRejection::Identity as usize + 1;

#[derive(Clone)]
pub(crate) struct WindowFilter {
    pub(crate) ignore_minimal: bool,
    pub(crate) only_current_desktop: bool,
    hidden_minimized: bool,
    topmost: bool,
    tool: bool,
    untitled: bool,
    min_width: u32,
    min_height: u32,
    titles: HashSet<String>,
    processes: HashSet<String>,
    monitor: MonitorFilter,
    scope: MonitorScope,
}

impl WindowFilter {
    pub(crate) fn from_config(config: &Config, kind: SwitchKind) -> Self {
        let (
            ignore_minimal,
            only_current_desktop,
            topmost,
            tool,
            untitled,
            min_width,
            min_height,
            titles,
            processes,
        ) = match kind {
            SwitchKind::Apps | SwitchKind::Search => (
                config.switch_apps_ignore_minimal,
                config.switch_apps_only_current_desktop(),
                config.switch_apps_include_topmost,
                config.switch_apps_include_tool_windows,
                config.switch_apps_include_untitled,
                config.switch_apps_min_width,
                config.switch_apps_min_height,
                &config.switch_apps_exclude_titles,
                &config.switch_apps_exclude_processes,
            ),
            SwitchKind::Windows => (
                config.switch_windows_ignore_minimal,
                config.switch_windows_only_current_desktop(),
                config.switch_windows_include_topmost,
                config.switch_windows_include_tool_windows,
                config.switch_windows_include_untitled,
                config.switch_windows_min_width,
                config.switch_windows_min_height,
                &config.switch_windows_exclude_titles,
                &config.switch_windows_exclude_processes,
            ),
        };
        Self {
            ignore_minimal,
            only_current_desktop,
            hidden_minimized: match kind {
                SwitchKind::Windows => config.switch_windows_include_hidden_minimized,
                SwitchKind::Apps | SwitchKind::Search => {
                    config.switch_apps_include_hidden_minimized
                }
            },
            topmost,
            tool,
            untitled,
            min_width,
            min_height,
            titles: titles.clone(),
            processes: processes.iter().map(|p| p.to_lowercase()).collect(),
            monitor: match kind {
                SwitchKind::Windows => config.switch_windows_monitor_filter,
                SwitchKind::Apps | SwitchKind::Search => config.switch_apps_monitor_filter,
            },
            scope: MonitorScope::default(),
        }
    }

    pub(crate) fn with_scope(mut self, scope: MonitorScope) -> Self {
        self.scope = scope;
        self
    }

    pub(crate) fn allows_process(&self, executable: &str) -> bool {
        !self.processes.contains(&executable.to_lowercase())
    }

    fn inspect_style(
        &self,
        (visible, iconic, tool, topmost): (bool, bool, bool, bool),
    ) -> Result<(), FilterRejection> {
        if !visible && !(self.hidden_minimized && iconic) {
            Err(FilterRejection::Invisible)
        } else if self.ignore_minimal && iconic {
            Err(FilterRejection::Minimized)
        } else if !self.tool && tool {
            Err(FilterRejection::Tool)
        } else if !self.topmost && topmost {
            Err(FilterRejection::Topmost)
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    fn allows_style(&self, state: (bool, bool, bool, bool)) -> bool {
        self.inspect_style(state).is_ok()
    }

    fn allows_size(&self, width: i32, height: i32, dpi: u32) -> bool {
        i64::from(width) * 96 >= i64::from(self.min_width) * i64::from(dpi)
            && i64::from(height) * 96 >= i64::from(self.min_height) * i64::from(dpi)
    }

    pub(crate) fn allows(&self, hwnd: HWND) -> Option<(String, bool)> {
        self.inspect(hwnd).ok()
    }

    pub(super) fn inspect(&self, hwnd: HWND) -> Result<(String, bool), FilterRejection> {
        self.inspect_content(hwnd, self.inspect_window_style(hwnd)?)
    }

    pub(super) fn inspect_window_style(
        &self,
        hwnd: HWND,
    ) -> Result<(bool, bool, bool, bool), FilterRejection> {
        if !self.scope.allows(self.monitor, hwnd) {
            return Err(FilterRejection::Scope);
        }
        let state = utils::get_window_state(hwnd);
        self.inspect_style(state)?;
        Ok(state)
    }

    pub(super) fn inspect_content(
        &self,
        hwnd: HWND,
        state: (bool, bool, bool, bool),
    ) -> Result<(String, bool), FilterRejection> {
        if utils::is_cloaked_window(hwnd, self.only_current_desktop) {
            return Err(FilterRejection::Cloaked);
        }
        let (width, height) =
            utils::get_window_size(hwnd).map_err(|_| FilterRejection::Geometry)?;
        let dpi = crate::layout::window_monitor_dpi(hwnd).map_err(|_| FilterRejection::Geometry)?;
        if (!state.0 && (width <= 0 || height <= 0)) || !self.allows_size(width, height, dpi) {
            return Err(FilterRejection::Geometry);
        }
        let title = utils::get_window_title(hwnd);
        if (!self.untitled && title.is_empty()) || self.titles.contains(&title) {
            return Err(FilterRejection::Title);
        }
        Ok((title, state.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_minimized_is_opt_in_and_keeps_other_filters_and_modes_independent() {
        let mut config = Config::default();
        assert!(!WindowFilter::from_config(&config, SwitchKind::Apps)
            .allows_style((false, true, false, false)));
        config.switch_apps_include_hidden_minimized = true;
        for kind in [SwitchKind::Apps, SwitchKind::Search] {
            let filter = WindowFilter::from_config(&config, kind);
            assert!(filter.allows_style((false, true, false, false)));
            assert!(!filter.allows_style((false, false, false, false)));
            assert!(!filter.allows_style((false, true, true, false)));
            assert!(!filter.allows_style((false, true, false, true)));
        }
        assert!(!WindowFilter::from_config(&config, SwitchKind::Windows)
            .allows_style((false, true, false, false)));
        config.switch_apps_ignore_minimal = true;
        assert!(!WindowFilter::from_config(&config, SwitchKind::Apps)
            .allows_style((false, true, false, false)));
    }

    #[test]
    fn both_switch_modes_have_independent_filters_and_default_exclusions() {
        let mut config = Config {
            switch_apps_include_topmost: true,
            switch_apps_include_tool_windows: true,
            ..Default::default()
        };
        config
            .switch_apps_exclude_processes
            .insert("EXAMPLE.EXE".into());
        let apps = WindowFilter::from_config(&config, SwitchKind::Apps);
        let windows = WindowFilter::from_config(&config, SwitchKind::Windows);
        assert!(apps.allows_style((true, false, true, true)));
        assert!(!windows.allows_style((true, false, true, true)));
        assert!(!apps.allows_style((false, false, false, false)));
        assert!(!apps.allows_process("example.exe"));
        assert!(windows.allows_process("example.exe"));
        assert!(windows.titles.contains("Windows Input Experience"));
        assert!(!windows.untitled);
        config.switch_apps_monitor_filter = MonitorFilter::Panel;
        config.switch_windows_monitor_filter = MonitorFilter::Foreground;
        assert_eq!(
            WindowFilter::from_config(&config, SwitchKind::Apps).monitor,
            MonitorFilter::Panel
        );
        assert_eq!(
            WindowFilter::from_config(&config, SwitchKind::Search).monitor,
            MonitorFilter::Panel
        );
        assert_eq!(
            WindowFilter::from_config(&config, SwitchKind::Windows).monitor,
            MonitorFilter::Foreground
        );
        for dpi in [96, 144, 192] {
            assert!(windows.allows_size((120 * dpi / 96) as i32, (90 * dpi / 96) as i32, dpi));
            assert!(!windows.allows_size((120 * dpi / 96) as i32 - 1, (90 * dpi / 96) as i32, dpi));
        }
    }
}
