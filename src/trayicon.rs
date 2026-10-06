use crate::{
    app::{NAME, WM_USER_TRAYICON},
    config::Config,
    localization::Text,
    startup::StartupState,
};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{GetLastError, SetLastError, ERROR_SUCCESS, HWND, LPARAM, POINT, WPARAM},
    UI::{
        Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO,
            NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
        },
        WindowsAndMessaging::{
            CreateIconFromResourceEx, DestroyIcon, DestroyMenu, GetCursorPos,
            LookupIconIdFromDirectoryEx, PostMessageW, SetForegroundWindow, TrackPopupMenu, HMENU,
            LR_DEFAULTCOLOR, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, WM_NULL,
        },
    },
};

pub(crate) mod quick_settings;

const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.ico");

#[derive(Clone, Copy)]
pub(crate) struct TrayMenuState<'a> {
    pub(crate) startup: StartupState,
    pub(crate) startup_busy: bool,
    pub(crate) paused: bool,
    pub(crate) pause_busy: bool,
    pub(crate) elevated: bool,
    pub(crate) restarting: bool,
    pub(crate) settings_busy: bool,
    pub(crate) pending_settings: bool,
    pub(crate) report_busy: bool,
    pub(crate) report_available: bool,
    pub(crate) configuration: &'a Config,
}

pub(crate) struct TrayIcon {
    data: NOTIFYICONDATAW,
    #[cfg(test)]
    reject_registration: bool,
}

impl TrayIcon {
    pub(crate) fn create() -> Result<Self> {
        let offset = unsafe {
            LookupIconIdFromDirectoryEx(ICON_BYTES.as_ptr(), true, 0, 0, LR_DEFAULTCOLOR)
        };
        if offset <= 0 || offset as usize >= ICON_BYTES.len() {
            bail!("trayicon stage=icon invalid resource");
        }
        let icon = unsafe {
            CreateIconFromResourceEx(
                &ICON_BYTES[offset as usize..],
                true,
                0x30000,
                0,
                0,
                LR_DEFAULTCOLOR,
            )
        }
        .context("trayicon stage=icon-create")?;
        Ok(Self {
            #[cfg(test)]
            reject_registration: false,
            data: NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                uID: WM_USER_TRAYICON,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_USER_TRAYICON,
                hIcon: icon,
                szTip: notification_text(&String::from_utf16_lossy(unsafe { NAME.as_wide() })),
                ..Default::default()
            },
        })
    }

    #[cfg(test)]
    pub(crate) fn failing_registration_for_test() -> Result<Self> {
        let mut icon = Self::create()?;
        icon.reject_registration = true;
        Ok(icon)
    }

    pub(crate) fn register(&mut self, hwnd: HWND) -> Result<()> {
        self.data.hWnd = hwnd;
        #[cfg(test)]
        if self.reject_registration {
            bail!("test-only tray registration failure");
        }
        unsafe { Shell_NotifyIconW(NIM_ADD, &self.data) }
            .ok()
            .context("trayicon stage=register")
    }

    pub(crate) fn exist(&mut self) -> bool {
        #[cfg(test)]
        if self.reject_registration {
            return false;
        }
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data) }.as_bool()
    }

    pub(crate) fn notify(&self, title: &str, message: &str, error: bool) -> Result<()> {
        let mut notification = self.data;
        notification.uFlags = NIF_INFO;
        notification.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
        notification.szInfoTitle = notification_text(title);
        notification.szInfo = notification_text(message);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &notification) }
            .ok()
            .context("trayicon stage=notification")
    }

    pub(crate) fn show(&mut self, state: TrayMenuState<'_>, text: Text) -> Result<Option<u32>> {
        let hwnd = self.data.hWnd;
        let mut cursor = POINT::default();
        unsafe { SetForegroundWindow(hwnd) }
            .ok()
            .context("trayicon stage=foreground")?;
        unsafe { GetCursorPos(&mut cursor) }?;
        let menu = self.create_menu(state, text)?;
        unsafe { SetLastError(ERROR_SUCCESS) };
        let command = unsafe {
            TrackPopupMenu(
                menu.0,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD | TPM_NONOTIFY,
                cursor.x,
                cursor.y,
                None,
                hwnd,
                None,
            )
        }
        .0 as u32;
        let error = unsafe { GetLastError() };
        let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
        if command == 0 && error != ERROR_SUCCESS {
            return Err(windows::core::Error::from(error)).context("trayicon stage=menu");
        }
        Ok((command != 0).then_some(command))
    }

    fn create_menu(&self, state: TrayMenuState<'_>, text: Text) -> Result<Menu> {
        quick_settings::build(state, text)
    }
}

struct Menu(HMENU);
impl Drop for Menu {
    fn drop(&mut self) {
        if let Err(error) = unsafe { DestroyMenu(self.0) } {
            warn!("trayicon stage=menu-release code={:#x}", error.code().0);
        }
    }
}

fn notification_text<const N: usize>(text: &str) -> [u16; N] {
    let mut result = [0; N];
    let mut length = 0;
    for character in text.chars() {
        let mut units = [0; 2];
        let encoded = character.encode_utf16(&mut units);
        if length + encoded.len() >= N {
            break;
        }
        result[length..length + encoded.len()].copy_from_slice(encoded);
        length += encoded.len();
    }
    result
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
            if let Err(error) = DestroyIcon(self.data.hIcon) {
                warn!("trayicon stage=icon-release code={:#x}", error.code().0);
            }
        }
    }
}

#[cfg(test)]
mod tests;
