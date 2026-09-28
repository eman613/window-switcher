use crate::{layout::MAX_WINDOWS, utils};
use anyhow::{ensure, Context, Result};
use windows::{
    core::BOOL,
    Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::EnumWindows,
    },
};

struct WindowEnumeration {
    windows: Vec<usize>,
    excluded: usize,
    excluded_process: u32,
    overflow: bool,
    process_queries: usize,
}

impl WindowEnumeration {
    fn admit(&mut self, window: usize, mut is_excluded: impl FnMut(usize) -> bool) -> bool {
        if window == self.excluded {
            return true;
        }
        if self.windows.len() == MAX_WINDOWS {
            if is_excluded(window) {
                return true;
            }
            // Keep the original external-window limit without querying every
            // hidden desktop window in the common, below-capacity path.
            self.windows.retain(|window| !is_excluded(*window));
            if self.windows.len() == MAX_WINDOWS {
                self.overflow = true;
                return false;
            }
        }
        self.windows.push(window);
        true
    }
}

unsafe extern "system" fn enumerate(hwnd: HWND, parameter: LPARAM) -> BOOL {
    let output = &mut *(parameter.0 as *mut WindowEnumeration);
    let excluded_process = output.excluded_process;
    let mut queries = 0;
    let accepted = output.admit(hwnd.0 as usize, |window| {
        if excluded_process == 0 {
            return false;
        }
        queries += 1;
        utils::get_window_pid(HWND(window as _)) == excluded_process
    });
    output.process_queries += queries;
    BOOL::from(accepted)
}

pub(super) fn collect(excluded: usize) -> Result<(Vec<usize>, u32)> {
    let mut output = WindowEnumeration {
        windows: Vec::new(),
        excluded,
        excluded_process: if excluded == 0 {
            0
        } else {
            utils::get_window_pid(HWND(excluded as _))
        },
        overflow: false,
        process_queries: 0,
    };
    let status = unsafe { EnumWindows(Some(enumerate), LPARAM(&mut output as *mut _ as isize)) };
    debug!(
        "snapshot stage=collect windows={} process_queries={} overflow={}",
        output.windows.len(),
        output.process_queries,
        output.overflow
    );
    ensure!(
        !output.overflow,
        "snapshot stage=enumerate window-limit-exceeded"
    );
    status.context("snapshot stage=enumerate")?;
    Ok((output.windows, output.excluded_process))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> WindowEnumeration {
        WindowEnumeration {
            windows: Vec::new(),
            excluded: usize::MAX,
            excluded_process: 1,
            overflow: false,
            process_queries: 0,
        }
    }

    #[test]
    fn below_capacity_and_explicit_exclusion_do_not_query_processes() {
        let mut output = empty();
        for window in 0..MAX_WINDOWS {
            assert!(output.admit(window, |_| panic!("unnecessary PID query")));
        }
        assert!(output.admit(usize::MAX, |_| panic!("explicit HWND needs no PID")));
        assert_eq!(output.windows, (0..MAX_WINDOWS).collect::<Vec<_>>());
        assert!(!output.overflow);
    }

    #[test]
    fn full_external_list_accepts_own_windows_but_rejects_one_more_external_window() {
        let mut output = empty();
        output.windows = (0..MAX_WINDOWS).collect();
        assert!(output.admit(MAX_WINDOWS, |window| window == MAX_WINDOWS));
        assert_eq!(output.windows.len(), MAX_WINDOWS);
        assert!(!output.overflow);
        assert!(!output.admit(MAX_WINDOWS + 1, |_| false));
        assert!(output.overflow);
        assert_eq!(output.windows, (0..MAX_WINDOWS).collect::<Vec<_>>());
    }

    #[test]
    fn capacity_cleanup_preserves_external_order_and_bounded_storage() {
        let mut output = empty();
        output.windows = (0..MAX_WINDOWS).collect();
        assert!(output.admit(MAX_WINDOWS + 1, |window| window % 2 == 0));
        let expected: Vec<_> = (0..MAX_WINDOWS)
            .filter(|window| window % 2 != 0)
            .chain([MAX_WINDOWS + 1])
            .collect();
        assert_eq!(output.windows, expected);
        for window in MAX_WINDOWS + 2..MAX_WINDOWS * 3 {
            if !output.admit(window, |window| window % 2 == 0) {
                break;
            }
            assert!(output.windows.len() <= MAX_WINDOWS);
        }
        assert!(output.overflow);
        assert_eq!(output.windows.len(), MAX_WINDOWS);
        assert!(output.windows.iter().all(|window| window % 2 != 0));
    }
}
