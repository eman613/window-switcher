//! Restore visible picker tips after a preview is first shown or repositioned.
use windows::{
    core::BOOL,
    Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::*,
    },
};

pub(super) fn raise(surface: HWND) {
    unsafe {
        let thread = GetWindowThreadProcessId(surface, None);
        if thread != 0
            && !EnumThreadWindows(thread, Some(raise_owned), LPARAM(surface.0 as isize)).as_bool()
        {
            debug!("preview stage=tooltip-enumeration failed");
        }
    }
}

unsafe extern "system" fn raise_owned(hwnd: HWND, surface: LPARAM) -> BOOL {
    if IsWindowVisible(hwnd).as_bool()
        && GetWindow(hwnd, GW_OWNER).ok() == Some(HWND(surface.0 as _))
    {
        let mut class = [0u16; 64];
        let length = GetClassNameW(hwnd, &mut class).max(0) as usize;
        if class[..length]
            .iter()
            .copied()
            .eq("tooltips_class32".encode_utf16())
        {
            if let Err(error) = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            ) {
                warn!("preview stage=tooltip-raise code={:#x}", error.code().0);
            }
        }
    }
    true.into()
}
