use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use anyhow::{Context, Result};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP},
};

pub(super) const WM_INPUT_ACTIVATION: u32 = WM_APP + 0x41;

/// The input thread acknowledges changes between callbacks. An atomic flag alone
/// cannot prove that a callback which read the old flag has already returned.
pub(crate) struct InputActivation {
    requested: AtomicU64,
    applied: AtomicU64,
    thread: AtomicU32,
}

impl InputActivation {
    pub(super) fn new(active: bool) -> Self {
        Self {
            requested: AtomicU64::new(u64::from(active)),
            applied: AtomicU64::new(u64::from(active)),
            thread: AtomicU32::new(0),
        }
    }

    pub(super) fn register(&self, thread: u32) {
        self.thread.store(thread, Ordering::Release);
    }

    pub(crate) fn request(&self, active: bool) -> Result<()> {
        let previous = self.requested.load(Ordering::Acquire);
        if previous & 1 != u64::from(active) {
            self.requested.store(
                ((previous >> 1) + 1) * 2 + u64::from(active),
                Ordering::Release,
            );
        }
        unsafe {
            PostThreadMessageW(
                self.thread.load(Ordering::Acquire),
                WM_INPUT_ACTIVATION,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .context("input stage=activation-notify")
    }

    pub(crate) fn acknowledged(&self, active: bool) -> bool {
        let requested = self.requested.load(Ordering::Acquire);
        requested & 1 == u64::from(active) && self.applied.load(Ordering::Acquire) == requested
    }

    pub(super) fn active(&self) -> bool {
        self.applied.load(Ordering::Acquire) & 1 != 0
    }

    pub(super) fn pending(&self) -> u64 {
        self.requested.load(Ordering::Acquire)
    }

    pub(super) fn acknowledge(&self, generation: u64) {
        self.applied.store(generation, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pausing_is_not_quiescent_until_the_input_thread_acknowledges() {
        let activation = InputActivation::new(true);
        activation.requested.store(2, Ordering::Release);
        assert!(activation.active());
        assert!(!activation.acknowledged(false));
        activation.acknowledge(2);
        assert!(!activation.active());
        assert!(activation.acknowledged(false));
        activation.requested.store(5, Ordering::Release);
        assert!(!activation.acknowledged(true));
        activation.acknowledge(5);
        assert!(activation.acknowledged(true));
    }
}
