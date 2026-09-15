use std::hash::{Hash, Hasher};
use windows::Win32::{
    Foundation::{FILETIME, HWND},
    System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow},
};

use super::HandleWrapper;

/// A conservative activation identity. Unknown process identity is never
/// activated. Event-driven HWND lifetime generations are added with snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowIdentity {
    pub(crate) hwnd: HWND,
    pid: u32,
    thread: u32,
    created: u64,
}

impl Hash for WindowIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.hwnd.0 as usize, self.pid, self.thread, self.created).hash(state);
    }
}

impl WindowIdentity {
    pub(crate) fn capture(hwnd: HWND) -> Option<Self> {
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return None;
        }
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 || thread == 0 {
            return None;
        }
        let process = HandleWrapper::new(
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?,
        );
        let (mut created, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe {
            GetProcessTimes(
                process.get_handle(),
                &mut created,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        }
        .ok()?;
        let mut current_pid = 0;
        if unsafe { GetWindowThreadProcessId(hwnd, Some(&mut current_pid)) } != thread
            || current_pid != pid
        {
            return None;
        }
        Some(Self {
            hwnd,
            pid,
            thread,
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        })
    }

    pub(crate) fn is_current(self) -> bool {
        Self::capture(self.hwnd) == Some(self)
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
        assert!(WindowIdentity::capture(HWND::default()).is_none());
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
        let identity = WindowIdentity::capture(hwnd).unwrap();
        assert!(identity.is_current());
        let mut stale = identity;
        stale.created ^= 1;
        assert!(!stale.is_current());
        assert_eq!(super::super::get_window_size(hwnd).unwrap(), (320, 200));
        unsafe { DestroyWindow(hwnd) }.unwrap();
        assert!(!identity.is_current());
        assert!(super::super::get_window_size(hwnd).is_err());
    }
}
