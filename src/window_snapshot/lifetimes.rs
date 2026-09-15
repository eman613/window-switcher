use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

const LIFETIME_LIMIT: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LifetimeStamp {
    epoch: u64,
    sequence: u64,
}

#[derive(Default)]
pub(crate) struct WindowLifetimes {
    epoch: AtomicU64,
    revision: AtomicU64,
    entries: Mutex<HashMap<usize, u64>>,
}

impl WindowLifetimes {
    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(crate) fn event(&self, window: usize, lifetime_changed: bool) {
        let sequence = self.revision.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        if !lifetime_changed {
            return;
        }
        let Some(mut entries) = self.entries.try_lock() else {
            self.invalidate_all();
            return;
        };
        if entries.len() >= LIFETIME_LIMIT || sequence == 0 {
            entries.clear();
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
        entries.insert(window, sequence);
    }

    pub(crate) fn invalidate_all(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn stamp(&self, window: usize) -> Option<LifetimeStamp> {
        // Activation uses this path too. A paused auxiliary thread must never
        // transfer a blocking registry lock to the UI.
        let Some(entries) = self.entries.try_lock() else {
            debug!("window stage=identity registry-busy");
            return None;
        };
        Some(LifetimeStamp {
            epoch: self.epoch.load(Ordering::Acquire),
            sequence: entries.get(&window).copied().unwrap_or(0),
        })
    }

    pub(crate) fn matches(&self, window: usize, stamp: LifetimeStamp) -> bool {
        let Some(entries) = self.entries.try_lock() else {
            return false;
        };
        stamp.epoch == self.epoch.load(Ordering::Acquire)
            && stamp.sequence == entries.get(&window).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recycled_handles_and_lost_events_invalidate_old_stamps() {
        let registry = WindowLifetimes::default();
        let first = registry.stamp(7).unwrap();
        registry.event(7, false);
        assert!(registry.matches(7, first));
        registry.event(7, true);
        assert!(!registry.matches(7, first));
        let second = registry.stamp(7).unwrap();
        let guard = registry.entries.lock();
        registry.event(8, true);
        drop(guard);
        assert!(!registry.matches(7, second));
        let third = registry.stamp(7).unwrap();
        for window in 0..=LIFETIME_LIMIT {
            registry.event(window, true);
        }
        assert!(!registry.matches(7, third));
        assert!(registry.entries.lock().len() <= LIFETIME_LIMIT);
    }

    #[test]
    fn a_busy_registry_rejects_identity_reads_without_waiting() {
        let registry = WindowLifetimes::default();
        let stamp = registry.stamp(7).unwrap();
        let held = registry.entries.lock();
        assert!(registry.stamp(7).is_none());
        assert!(!registry.matches(7, stamp));
        drop(held);
        assert_eq!(registry.stamp(7), Some(stamp));
        assert!(registry.matches(7, stamp));
    }
}
