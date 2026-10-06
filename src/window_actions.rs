//! One explicit close request; a posted message is not proof of window closure.
use crate::{utils::window_identity::WindowIdentity, window_snapshot::lifetimes::WindowLifetimes};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE},
};
pub(crate) fn request_close(
    identity: WindowIdentity,
    lifetimes: &WindowLifetimes,
) -> Result<(), &'static str> {
    if identity.process.pid == std::process::id() {
        return Err("self-window");
    }
    if !identity.is_current(lifetimes) {
        return Err("stale-window");
    }
    unsafe { PostMessageW(Some(identity.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0)) }
        .map_err(|_| "close-post-rejected")
}

#[cfg(test)]
mod tests;
