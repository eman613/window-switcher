use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::Mutex;
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::PostMessageW,
};

static NEXT_GENERATION: AtomicUsize = AtomicUsize::new(1);

/// Only a numeric notification endpoint crosses threads. No App reference,
/// borrowed pointer or owned Win32 UI resource is sent to another thread.
pub(crate) struct WindowTarget {
    window: isize,
    generation: usize,
    live: AtomicBool,
    posting: Mutex<()>,
}

impl WindowTarget {
    pub(crate) fn new(hwnd: HWND) -> Self {
        Self {
            window: hwnd.0 as isize,
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            live: AtomicBool::new(true),
            posting: Mutex::new(()),
        }
    }

    pub(crate) fn try_post(&self, message: u32) -> bool {
        let Some(_posting) = self.posting.try_lock() else {
            return false;
        };
        if !self.is_live() {
            return false;
        }
        unsafe {
            PostMessageW(
                Some(HWND(self.window as _)),
                message,
                WPARAM(0),
                LPARAM(self.generation as isize),
            )
        }
        .is_ok()
    }

    pub(crate) fn accepts(&self, generation: LPARAM) -> bool {
        self.is_live() && generation.0 as usize == self.generation
    }

    pub(crate) fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    pub(crate) fn window_id(&self) -> usize {
        self.window as usize
    }

    pub(crate) fn close(&self) {
        // Posters use try_lock and never wait. Once close returns no worker can
        // post to this HWND again, even if Windows later reuses its numeric value.
        let _posting = self.posting.lock();
        self.live.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retired_targets_reject_stale_generations_and_posts() {
        let target = WindowTarget::new(HWND::default());
        assert!(target.accepts(LPARAM(target.generation as isize)));
        assert!(!target.accepts(LPARAM(target.generation.wrapping_add(1) as isize)));
        target.close();
        assert!(!target.accepts(LPARAM(target.generation as isize)));
        assert!(!target.try_post(6003));
    }
}
