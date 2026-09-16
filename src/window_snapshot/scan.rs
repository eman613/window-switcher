use super::{filter::WindowFilter, lifetimes::WindowLifetimes, WindowRecord, WindowSnapshot};
use crate::{
    layout::MAX_WINDOWS,
    process_metadata::ProcessMetadataCache,
    utils::{self, browser, window_identity::WindowIdentity},
};
use anyhow::{ensure, Context, Result};
use indexmap::IndexMap;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    core::BOOL,
    Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::EnumWindows,
    },
};

struct Enumeration {
    windows: Vec<usize>,
    overflow: bool,
    excluded: usize,
    excluded_process: u32,
}
const TEXT_LIMIT: usize = 16 * 1024 * 1024;
unsafe extern "system" fn enumerate(hwnd: HWND, parameter: LPARAM) -> BOOL {
    let output = &mut *(parameter.0 as *mut Enumeration);
    if hwnd.0 as usize == output.excluded
        || (output.excluded_process != 0 && utils::get_window_pid(hwnd) == output.excluded_process)
    {
        return BOOL(1);
    }
    if output.windows.len() == MAX_WINDOWS {
        output.overflow = true;
        return BOOL(0);
    }
    output.windows.push(hwnd.0 as usize);
    BOOL(1)
}

pub(super) struct Scan {
    windows: Vec<usize>,
    owners: HashMap<usize, usize>,
    next: usize,
    groups: IndexMap<Arc<str>, Vec<WindowRecord>>,
    filter: WindowFilter,
    is_admin: bool,
    revision: u64,
    text_bytes: usize,
}

impl Scan {
    #[cfg(test)]
    pub(super) fn groups_is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub(super) fn begin(
        filter: WindowFilter,
        is_admin: bool,
        revision: u64,
        excluded: usize,
    ) -> Result<Self> {
        let mut enumeration = Enumeration {
            windows: Vec::new(),
            overflow: false,
            excluded,
            excluded_process: if excluded == 0 {
                0
            } else {
                utils::get_window_pid(HWND(excluded as _))
            },
        };
        let status =
            unsafe { EnumWindows(Some(enumerate), LPARAM(&mut enumeration as *mut _ as isize)) };
        anyhow::ensure!(
            !enumeration.overflow,
            "snapshot stage=enumerate window-limit-exceeded"
        );
        status.context("snapshot stage=enumerate")?;
        let mut owners = HashMap::new();
        for &window in &enumeration.windows {
            let owner = utils::get_owner_window(HWND(window as _)).0 as usize;
            if owner != 0 {
                owners.entry(owner).or_insert(window);
            }
        }
        Ok(Self {
            windows: enumeration.windows,
            owners,
            next: 0,
            groups: IndexMap::new(),
            filter,
            is_admin,
            revision,
            text_bytes: 0,
        })
    }

    /// A slice yields between native calls. A slow system call cannot be interrupted.
    pub(super) fn step(
        &mut self,
        metadata: &mut ProcessMetadataCache,
        lifetimes: &WindowLifetimes,
        budget: Duration,
        mut cancelled: impl FnMut(&mut ProcessMetadataCache) -> bool,
    ) -> bool {
        let started = Instant::now();
        while self.next < self.windows.len() {
            if cancelled(metadata) {
                return false;
            }
            let window = self.windows[self.next];
            self.next += 1;
            self.inspect(window, metadata, lifetimes);
            if self.text_bytes > TEXT_LIMIT {
                return true;
            }
            if started.elapsed() >= budget {
                return self.next == self.windows.len();
            }
        }
        true
    }

    fn inspect(
        &mut self,
        window: usize,
        metadata: &mut ProcessMetadataCache,
        lifetimes: &WindowLifetimes,
    ) {
        let hwnd = HWND(window as _);
        let Some((title, minimized)) = self.filter.allows(hwnd) else {
            return;
        };
        let Some(native_process) = metadata.lookup(utils::get_window_pid(hwnd)) else {
            return;
        };
        let Some(identity) = WindowIdentity::from_process(hwnd, native_process.identity, lifetimes)
        else {
            return;
        };
        let process = if native_process
            .executable
            .eq_ignore_ascii_case("ApplicationFrameHost.exe")
        {
            let Some(child) = self
                .owners
                .get(&window)
                .and_then(|child| metadata.lookup(utils::get_window_pid(HWND(*child as _))))
            else {
                return;
            };
            child
        } else {
            native_process
        };
        if process
            .executable
            .eq_ignore_ascii_case("ApplicationFrameHost.exe")
            || !self.filter.allows_process(&process.executable)
            || (!self.is_admin && process.elevated == Some(true))
        {
            return;
        }
        let group = browser::group_key(&process.path, hwnd);
        // Count shared strings conservatively for every retained record. A
        // pathological desktop must not allocate 4096 maximum-length titles.
        self.text_bytes = self.text_bytes.saturating_add(
            title.len() + group.len() + process.path.len() + process.executable.len(),
        );
        if self.text_bytes > TEXT_LIMIT {
            return;
        }
        self.groups.entry(group).or_default().push(WindowRecord {
            identity,
            process,
            title,
            minimized,
        });
    }

    pub(super) fn finish(self) -> Result<WindowSnapshot> {
        ensure!(
            self.text_bytes <= TEXT_LIMIT,
            "snapshot stage=text-budget exceeded"
        );
        debug!(
            "window stage=enumerated groups={} windows={}",
            self.groups.len(),
            self.groups.values().map(Vec::len).sum::<usize>()
        );
        Ok(WindowSnapshot {
            groups: self.groups,
            revision: self.revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_text_is_rejected_instead_of_publishing_an_incomplete_snapshot() {
        let config = crate::config::Config::default();
        let mut scan = Scan::begin(
            WindowFilter::from_config(&config, crate::keyboard::state::SwitchKind::Apps),
            true,
            0,
            0,
        )
        .unwrap();
        scan.text_bytes = TEXT_LIMIT + 1;
        assert!(scan.finish().is_err());
    }
}
