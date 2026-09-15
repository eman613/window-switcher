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
        lifetimes.matches(self.window, self.lifetime)
            && Self::capture(self.hwnd(), lifetimes) == Some(self)
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

    #[test]
    fn destroyed_window_and_unknown_identity_are_rejected() {
        let lifetimes = WindowLifetimes::default();
        assert!(WindowIdentity::capture(HWND::default(), &lifetimes).is_none());
        let hwnd = unsafe {
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
        .unwrap();
        let identity = WindowIdentity::capture(hwnd, &lifetimes).unwrap();
        assert!(identity.is_current(&lifetimes));
        let mut stale = identity;
        stale.process.created ^= 1;
        assert!(!stale.is_current(&lifetimes));
        lifetimes.event(hwnd.0 as usize, true);
        assert!(!identity.is_current(&lifetimes));
        assert_eq!(super::super::get_window_size(hwnd).unwrap(), (320, 200));
        unsafe { DestroyWindow(hwnd) }.unwrap();
        assert!(!identity.is_current(&lifetimes));
        assert!(super::super::get_window_size(hwnd).is_err());
    }
}
