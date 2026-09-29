//! The HWND owner also owns retirement of every auxiliary notification sender.
use super::window_proc;
use crate::{app::NAME, utils::check_error, window_target::WindowTarget};
use anyhow::{Context, Result};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::Ime::{ImmAssociateContextEx, HIMC},
        WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetWindowLongPtrW, IsWindow, LoadCursorW,
            RegisterClassW, SetWindowLongPtrW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWL_STYLE,
            IDC_ARROW, WINDOW_STYLE, WNDCLASSW, WS_CAPTION, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST,
        },
    },
};

static WINDOW_CLASS: OnceCell<u16> = OnceCell::new();
pub(super) struct ApplicationWindow(pub(super) HWND, Arc<WindowTarget>);
impl ApplicationWindow {
    pub(super) fn target(&self) -> Arc<WindowTarget> {
        self.1.clone()
    }

    pub(super) fn from_handle(hwnd: HWND) -> Self {
        Self(hwnd, Arc::new(WindowTarget::new(hwnd)))
    }

    pub(super) fn create() -> Result<Self> {
        let module = unsafe { GetModuleHandleW(None) }.context("ui stage=module")?;
        WINDOW_CLASS.get_or_try_init(|| -> Result<u16> {
            let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }?;
            let class = WNDCLASSW {
                hCursor: cursor,
                hInstance: HINSTANCE(module.0),
                lpszClassName: NAME,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&class) };
            if atom == 0 {
                return Err(windows::core::Error::from_win32()).context("ui stage=register-class");
            }
            Ok(atom)
        })?;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                NAME,
                NAME,
                WINDOW_STYLE(0),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(module.into()),
                None,
            )
        }
        .context("ui stage=create-window")?;
        let window = Self::from_handle(hwnd);
        // This surface never edits text. Keep its modifier/navigation messages
        // out of IME composition. Text-capable UI threads retain their input
        // services and Search owns a separate native EDIT context.
        if !unsafe { ImmAssociateContextEx(hwnd, HIMC::default(), 0) }.as_bool() {
            debug!("ui stage=panel-ime-detach unavailable; retaining system association");
        }
        let style = check_error(|| unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) })? as u32;
        check_error(|| unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, (style & !WS_CAPTION.0) as isize)
        })?;
        Ok(window)
    }
}
impl Drop for ApplicationWindow {
    fn drop(&mut self) {
        if self.1.is_live() {
            debug!("ui stage=retire-window before-destroy");
        }
        self.1.close();
        if unsafe { IsWindow(Some(self.0)) }.as_bool() {
            if let Err(err) = unsafe { DestroyWindow(self.0) } {
                warn!("ui stage=destroy-window code={:#x}", err.code().0);
            }
        }
    }
}
