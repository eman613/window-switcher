use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use std::{ffi::c_void, mem::size_of, os::windows::ffi::OsStrExt, path::PathBuf};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWM_CLOAKED_SHELL},
        Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    },
    UI::{
        Input::KeyboardAndMouse::{GetFocus, SendInput, SetFocus, INPUT, INPUT_MOUSE},
        WindowsAndMessaging::{
            GetCursorPos, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowPlacement,
            GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
            SetForegroundWindow, ShowWindowAsync, GWL_EXSTYLE, GWL_STYLE, GWL_USERDATA, GW_OWNER,
            SW_RESTORE, WINDOWPLACEMENT, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_ICONIC, WS_VISIBLE,
        },
    },
};

pub fn get_window_state(hwnd: HWND) -> (bool, bool, bool, bool) {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let exstyle = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    (
        style & WS_VISIBLE.0 != 0,
        style & WS_ICONIC.0 != 0,
        exstyle & WS_EX_TOOLWINDOW.0 != 0,
        exstyle & WS_EX_TOPMOST.0 != 0,
    )
}

pub fn is_iconic_window(hwnd: HWND) -> bool {
    unsafe { IsIconic(hwnd) }.as_bool()
}

pub fn get_window_cloak_type(hwnd: HWND) -> u32 {
    let mut cloak = 0u32;
    if let Err(error) = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloak as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
    } {
        debug!("window stage=cloak code={:#x}", error.code().0);
    }
    cloak
}

pub(crate) fn is_cloaked_window(hwnd: HWND, only_current_desktop: bool) -> bool {
    let cloak = get_window_cloak_type(hwnd);
    if only_current_desktop {
        cloak != 0
    } else {
        cloak & !DWM_CLOAKED_SHELL != 0
    }
}

pub fn is_small_window(hwnd: HWND) -> bool {
    get_window_size(hwnd).map_or(true, |(w, h)| w < 120 || h < 90)
}

pub fn get_moinitor_rect() -> RECT {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut cursor = POINT::default();
    unsafe {
        if GetCursorPos(&mut cursor).is_ok()
            && GetMonitorInfoW(
                MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST),
                &mut info,
            )
            .as_bool()
        {
            return info.rcMonitor;
        }
    }
    RECT::default()
}

pub fn get_window_size(hwnd: HWND) -> windows::core::Result<(i32, i32)> {
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { GetWindowPlacement(hwnd, &mut placement) }?;
    let rect = placement.rcNormalPosition;
    Ok((
        rect.right.saturating_sub(rect.left),
        rect.bottom.saturating_sub(rect.top),
    ))
}

pub fn get_exe_folder() -> Result<PathBuf> {
    let path =
        std::env::current_exe().map_err(|err| anyhow!("Failed to get binary path, {err}"))?;
    path.parent()
        .ok_or_else(|| anyhow!("Failed to get binary folder"))
        .map(PathBuf::from)
}

pub fn get_exe_path() -> Vec<u16> {
    match std::env::current_exe() {
        Ok(path) => path.as_os_str().encode_wide().collect(),
        Err(error) => {
            warn!("window stage=executable-path kind={:?}", error.kind());
            Vec::new()
        }
    }
}

pub fn get_window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

pub fn get_module_path(pid: u32) -> Option<String> {
    let (handle, _) = crate::process_metadata::open_identity(pid)?;
    crate::process_metadata::image_path(&handle)
}

pub fn get_window_exe(hwnd: HWND) -> Option<String> {
    get_module_path(get_window_pid(hwnd)).map(|path| super::browser::exe_name(&path).to_owned())
}

pub fn set_foreground_window(hwnd: HWND, allowed: impl Fn() -> bool) -> bool {
    if !allowed() {
        return false;
    }
    unsafe {
        if is_iconic_window(hwnd) && !ShowWindowAsync(hwnd, SW_RESTORE).as_bool() {
            debug!("window stage=restore rejected");
            return false;
        }
        if !allowed() {
            return false;
        }
        let input = INPUT {
            r#type: INPUT_MOUSE,
            ..Default::default()
        };
        if SendInput(&[input], size_of::<INPUT>() as i32) != 1 {
            debug!("window stage=activation-input rejected");
        }
        // Restore/input can dispatch native callbacks. Validate again before activation.
        if !allowed() {
            return false;
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            debug!("window stage=activation rejected");
            return false;
        }
    }
    true
}

pub fn get_foreground_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

pub(crate) fn focus_window(hwnd: HWND, allowed: impl Fn() -> bool) -> bool {
    if !set_foreground_window(hwnd, &allowed) || !allowed() {
        return false;
    }
    if unsafe { GetFocus() } != hwnd {
        let result = unsafe { SetFocus(Some(hwnd)) };
        // SetFocus returns the previous HWND, which can legitimately be null.
        // Verify the new focus instead of interpreting that null as failure.
        if unsafe { GetFocus() } != hwnd {
            debug!(
                "window stage=keyboard-focus rejected code={:#x}",
                result.err().map_or(0, |error| error.code().0)
            );
            return false;
        }
    }
    allowed()
}

pub fn get_window_title(hwnd: HWND) -> String {
    const LIMIT: usize = 32768;
    let mut capacity = (unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize + 1).clamp(2, LIMIT);
    for _ in 0..3 {
        let mut buffer = vec![0u16; capacity];
        let length = unsafe { GetWindowTextW(hwnd, &mut buffer) }.max(0) as usize;
        if length < capacity - 1 {
            return String::from_utf16_lossy(&buffer[..length]);
        }
        if capacity == LIMIT {
            break;
        }
        capacity = (capacity * 2).min(LIMIT);
    }
    debug!("window stage=title truncated-or-changing");
    String::new()
}

pub fn get_owner_window(hwnd: HWND) -> HWND {
    unsafe { GetWindow(hwnd, GW_OWNER) }.unwrap_or_default()
}

#[cfg(target_arch = "x86")]
pub fn get_window_user_data(hwnd: HWND) -> i32 {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(hwnd, GWL_USERDATA) }
}
#[cfg(not(target_arch = "x86"))]
pub fn get_window_user_data(hwnd: HWND) -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_USERDATA) }
}
#[cfg(target_arch = "x86")]
pub fn set_window_user_data(hwnd: HWND, ptr: i32) -> i32 {
    unsafe { windows::Win32::UI::WindowsAndMessaging::SetWindowLongW(hwnd, GWL_USERDATA, ptr) }
}
#[cfg(not(target_arch = "x86"))]
pub fn set_window_user_data(hwnd: HWND, ptr: isize) -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(hwnd, GWL_USERDATA, ptr) }
}

/// Synchronous compatibility entry for inspection tools; the UI uses SnapshotService.
pub fn list_windows(
    ignore_minimal: bool,
    only_current_desktop: bool,
    is_admin: bool,
) -> Result<IndexMap<String, Vec<(HWND, String)>>> {
    crate::window_snapshot::scan_for_tools(ignore_minimal, only_current_desktop, is_admin)
}
