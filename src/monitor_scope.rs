//! Candidate monitors are captured once, independently of later cursor movement.
use crate::config::MonitorFilter;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST},
};

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MonitorScope {
    panel: usize,
    foreground: usize,
}

impl MonitorScope {
    pub(crate) fn capture(panel: usize, foreground: HWND) -> Self {
        Self {
            panel,
            foreground: Self::window_monitor(foreground),
        }
    }

    fn window_monitor(hwnd: HWND) -> usize {
        unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }.0 as usize
    }

    fn allows_monitor(self, filter: MonitorFilter, monitor: usize) -> bool {
        let expected = match filter {
            MonitorFilter::All => return true,
            MonitorFilter::Panel => self.panel,
            MonitorFilter::Foreground => self.foreground,
        };
        expected != 0 && monitor == expected
    }

    pub(crate) fn allows(self, filter: MonitorFilter, hwnd: HWND) -> bool {
        filter == MonitorFilter::All || self.allows_monitor(filter, Self::window_monitor(hwnd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_is_fixed_and_panel_and_foreground_are_independent() {
        let scope = MonitorScope {
            panel: 1,
            foreground: 2,
        };
        assert!(scope.allows_monitor(MonitorFilter::All, 3));
        assert!(scope.allows_monitor(MonitorFilter::Panel, 1));
        assert!(!scope.allows_monitor(MonitorFilter::Panel, 2));
        assert!(scope.allows_monitor(MonitorFilter::Foreground, 2));
        assert!(!scope.allows_monitor(MonitorFilter::Foreground, 1));
        assert!(!MonitorScope::default().allows_monitor(MonitorFilter::Panel, 0));
        // Repeated queries cannot update the captured monitor.
        assert!(!scope.allows_monitor(MonitorFilter::Panel, 3));
        assert!(scope.allows_monitor(MonitorFilter::Panel, 1));
    }
}
