use crate::utils;
use std::collections::HashMap;
use windows::Win32::Foundation::HWND;

/// Scan-local relationships in enumeration order, bounded by the input list.
pub(super) struct OwnerIndex {
    candidates: HashMap<usize, Vec<usize>>,
    pid_queries: u32,
}

impl OwnerIndex {
    pub(super) fn collect(windows: &[usize]) -> Self {
        let mut candidates: HashMap<usize, Vec<usize>> = HashMap::new();
        for &window in windows {
            let owner = utils::get_owner_window(HWND(window as _)).0 as usize;
            if owner != 0 {
                candidates.entry(owner).or_default().push(window);
            }
        }
        Self {
            candidates,
            pid_queries: 0,
        }
    }

    pub(super) fn first_external(&mut self, owner: usize, excluded_process: u32) -> Option<usize> {
        self.candidates.get(&owner)?.iter().copied().find(|window| {
            if excluded_process == 0 {
                return true;
            }
            self.pid_queries += 1;
            utils::get_window_pid(HWND(*window as _)) != excluded_process
        })
    }

    pub(super) fn report(&self) {
        debug!(
            "snapshot stage=owner-index owners={} candidates={} pid_queries={}",
            self.candidates.len(),
            self.candidates.values().map(Vec::len).sum::<usize>(),
            self.pid_queries
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_snapshot::tests::Fixture;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, GWLP_HWNDPARENT};

    #[test]
    fn relationships_keep_order_and_defer_process_exclusion_until_requested() {
        let owner = Fixture::new(false);
        let first = Fixture::new(false);
        let second = Fixture::new(false);
        for owned in [&first, &second] {
            unsafe { SetWindowLongPtrW(owned.0, GWLP_HWNDPARENT, owner.0 .0 as isize) };
        }
        let owner_id = owner.0 .0 as usize;
        let first_id = first.0 .0 as usize;
        let mut index = OwnerIndex::collect(&[first_id, second.0 .0 as usize]);
        assert_eq!(index.pid_queries, 0);
        assert_eq!(index.first_external(usize::MAX, std::process::id()), None);
        assert_eq!(index.pid_queries, 0);
        assert_eq!(index.first_external(owner_id, 0), Some(first_id));
        assert_eq!(index.first_external(owner_id, std::process::id()), None);
        assert_eq!(index.pid_queries, 2);

        // A failed PID/metadata lookup must not silently select a later window.
        drop(first);
        assert_eq!(utils::get_window_pid(HWND(first_id as _)), 0);
        assert_eq!(
            index.first_external(owner_id, std::process::id()),
            Some(first_id)
        );
    }
}
