use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow},
};

use crate::{
    process_metadata::{open_identity, ProcessIdentity},
    window_snapshot::lifetimes::{LifetimeStamp, WindowLifetimes},
};

/// Integer HWNDs are transferable identifiers, never cross-thread owners.
/// Both the native process identity and delivered lifecycle events must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WindowIdentity {
    pub(crate) window: usize,
    pub(crate) process: ProcessIdentity,
    thread: u32,
    lifetime: LifetimeStamp,
}

impl WindowIdentity {
    #[cfg(test)]
    pub(crate) fn fixture(window: usize) -> Self {
        Self {
            window,
            process: ProcessIdentity { pid: 1, created: 1 },
            thread: 1,
            lifetime: WindowLifetimes::default().stamp(window).unwrap(),
        }
    }

    pub(crate) fn hwnd(self) -> HWND {
        HWND(self.window as _)
    }

    pub(crate) fn capture(hwnd: HWND, lifetimes: &WindowLifetimes) -> Option<Self> {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let (_, process) = open_identity(pid)?;
        Self::from_process(hwnd, process, lifetimes)
    }

    pub(crate) fn from_process(
        hwnd: HWND,
        process: ProcessIdentity,
        lifetimes: &WindowLifetimes,
    ) -> Option<Self> {
        let lifetime = lifetimes.stamp(hwnd.0 as usize)?;
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return None;
        }
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != process.pid || thread == 0 {
            return None;
        }
        if !lifetimes.matches(hwnd.0 as usize, lifetime) {
            return None;
        }
        Some(Self {
            window: hwnd.0 as usize,
            process,
            thread,
            lifetime,
        })
    }

    pub(crate) fn is_current(self, lifetimes: &WindowLifetimes) -> bool {
        self.has_current_lifetime(lifetimes) && Self::capture(self.hwnd(), lifetimes) == Some(self)
    }

    pub(crate) fn has_current_lifetime(self, lifetimes: &WindowLifetimes) -> bool {
        lifetimes.matches(self.window, self.lifetime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{
        core::w,
        Win32::{
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
            },
        },
    };

    struct TestWindow(HWND);

    impl TestWindow {
        fn new() -> Self {
            Self(
                unsafe {
                    CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("STATIC"),
                        w!("isolated identity fixture"),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        320,
                        200,
                        None,
                        None,
                        Some(GetModuleHandleW(None).unwrap().into()),
                        None,
                    )
                }
                .unwrap(),
            )
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) }.unwrap();
        }
    }

    #[test]
    fn destroyed_window_and_unknown_identity_are_rejected() {
        let lifetimes = WindowLifetimes::default();
        assert!(WindowIdentity::capture(HWND::default(), &lifetimes).is_none());
        let window = TestWindow::new();
        let hwnd = window.0;
        let identity = WindowIdentity::capture(hwnd, &lifetimes).unwrap();
        assert!(identity.is_current(&lifetimes));
        let mut stale = identity;
        stale.process.created ^= 1;
        assert!(!stale.is_current(&lifetimes));
        lifetimes.event(hwnd.0 as usize, true);
        assert!(!identity.is_current(&lifetimes));
        let current = WindowIdentity::capture(hwnd, &lifetimes).unwrap();
        assert!(current.is_current(&lifetimes));
        assert_eq!(super::super::get_window_size(hwnd).unwrap(), (320, 200));
        drop(window);
        assert!(!current.is_current(&lifetimes));
        assert!(WindowIdentity::capture(hwnd, &lifetimes).is_none());
        assert!(super::super::get_window_size(hwnd).is_err());
    }

    #[test]
    fn native_window_recreation_accepts_only_the_new_lifetime() {
        let lifetimes = WindowLifetimes::default();
        let window = TestWindow::new();
        let old = WindowIdentity::capture(window.0, &lifetimes).unwrap();
        drop(window);
        assert!(!old.is_current(&lifetimes));
        // Explicit registry delivery also protects against native HWND reuse.
        // This test does not assume that Windows reuses the numeric handle.
        lifetimes.event(old.window, true);
        let replacement = TestWindow::new();
        lifetimes.event(replacement.0 .0 as usize, true);
        let current = WindowIdentity::capture(replacement.0, &lifetimes).unwrap();
        assert_eq!(old.process, current.process);
        assert_eq!(old.thread, current.thread);
        assert_ne!(old, current);
        assert!(!old.is_current(&lifetimes));
        assert!(current.is_current(&lifetimes));
    }
}
