use crate::{config::Config, keyboard::state::SwitchKind, utils};
use std::collections::HashSet;
use windows::Win32::Foundation::HWND;

#[derive(Clone)]
pub(crate) struct WindowFilter {
    pub(crate) ignore_minimal: bool,
    pub(crate) only_current_desktop: bool,
    topmost: bool,
    tool: bool,
    untitled: bool,
    min_width: u32,
    min_height: u32,
    titles: HashSet<String>,
    processes: HashSet<String>,
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
            SwitchKind::Apps => (
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
            topmost,
            tool,
            untitled,
            min_width,
            min_height,
            titles: titles.clone(),
            processes: processes.iter().map(|p| p.to_lowercase()).collect(),
        }
    }

    pub(crate) fn allows_process(&self, executable: &str) -> bool {
        !self.processes.contains(&executable.to_lowercase())
    }

    fn allows_style(&self, (visible, iconic, tool, topmost): (bool, bool, bool, bool)) -> bool {
        visible
            && (!self.ignore_minimal || !iconic)
            && (self.tool || !tool)
            && (self.topmost || !topmost)
    }

    fn allows_size(&self, width: i32, height: i32, dpi: u32) -> bool {
        i64::from(width) * 96 >= i64::from(self.min_width) * i64::from(dpi)
            && i64::from(height) * 96 >= i64::from(self.min_height) * i64::from(dpi)
    }

    pub(crate) fn allows(&self, hwnd: HWND) -> Option<(String, bool)> {
        let state = utils::get_window_state(hwnd);
        if !self.allows_style(state) || utils::is_cloaked_window(hwnd, self.only_current_desktop) {
            return None;
        }
        let (width, height) = utils::get_window_size(hwnd).ok()?;
        let dpi = crate::layout::window_monitor_dpi(hwnd).ok()?;
        if !self.allows_size(width, height, dpi) {
            return None;
        }
        let title = utils::get_window_title(hwnd);
        if (!self.untitled && title.is_empty()) || self.titles.contains(&title) {
            return None;
        }
        Some((title, state.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        for dpi in [96, 144, 192] {
            assert!(windows.allows_size((120 * dpi / 96) as i32, (90 * dpi / 96) as i32, dpi));
            assert!(!windows.allows_size((120 * dpi / 96) as i32 - 1, (90 * dpi / 96) as i32, dpi));
        }
    }
}
