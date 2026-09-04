use crate::app::{IDM_CONFIGURE, IDM_EXIT, IDM_STARTUP, NAME, WM_USER_TRAYICON};

use anyhow::{anyhow, Result};
use std::mem::size_of;
use windows::core::{w, PCWSTR};
use windows::Win32::{
    Foundation::{HWND, POINT},
    UI::{
        Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
            NOTIFYICONDATAW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, DestroyIcon, DestroyMenu,
            GetCursorPos, SetForegroundWindow, TrackPopupMenu, HMENU, LR_DEFAULTCOLOR, MF_CHECKED,
            MF_STRING, MF_UNCHECKED, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
        },
    },
};

const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.ico");
const TEXT_CONFIGURE: PCWSTR = w!("Configure");
const TEXT_STARTUP: PCWSTR = w!("Startup");
const TEXT_EXIT: PCWSTR = w!("Exit");

pub struct TrayIcon {
    data: NOTIFYICONDATAW,
}

impl TrayIcon {
    pub fn create() -> Result<Self> {
        let data = Self::create_nid()?;
        Ok(Self { data })
    }

    pub fn register(&mut self, hwnd: HWND) -> Result<()> {
        self.data.hWnd = hwnd;
        unsafe { Shell_NotifyIconW(NIM_ADD, &self.data) }
            .ok()
            .map_err(|e| anyhow!("Fail to add trayicon, {}", e))
    }

    pub fn exist(&mut self) -> bool {
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data) }.as_bool()
    }

    pub fn show(&mut self, startup: bool) -> Result<()> {
        let hwnd = self.data.hWnd;
        let mut cursor = POINT::default();
        unsafe {
            SetForegroundWindow(hwnd)
                .ok()
                .map_err(|e| anyhow!("Fail to set foreground window, {}", e))?;
            GetCursorPos(&mut cursor).map_err(|e| anyhow!("Fail to get cursor pos, {}", e))?;
            let menu = self
                .create_menu(startup)
                .map_err(|e| anyhow!("Fail to create menu, {}", e))?;
            TrackPopupMenu(
                menu.get(),
                TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                cursor.x,
                cursor.y,
                None,
                hwnd,
                None,
            )
            .ok()
            .map_err(|e| anyhow!("Fail to show popup menu, {}", e))?
        };
        Ok(())
    }

    fn create_nid() -> Result<NOTIFYICONDATAW> {
        let icon_data = icon_image_data(ICON_BYTES)?;
        let hicon =
            unsafe { CreateIconFromResourceEx(icon_data, true, 0x30000, 0, 0, LR_DEFAULTCOLOR) }
                .map_err(|err| anyhow!("Failed to load tray icon resource, {err}"))?;
        let mut tooltip = [0u16; 128];
        let name = unsafe { NAME.as_wide() };
        let tooltip_len = name.len().min(tooltip.len() - 1);
        tooltip[..tooltip_len].copy_from_slice(&name[..tooltip_len]);
        Ok(NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            uID: WM_USER_TRAYICON,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_USER_TRAYICON,
            hIcon: hicon,
            szTip: tooltip,
            ..Default::default()
        })
    }

    fn create_menu(&mut self, startup: bool) -> Result<PopupMenuGuard> {
        let startup_flags = if startup { MF_CHECKED } else { MF_UNCHECKED };
        unsafe {
            let menu = PopupMenuGuard::new(
                CreatePopupMenu().map_err(|err| anyhow!("Failed to create menu, {err}"))?,
            )?;
            AppendMenuW(
                menu.get(),
                MF_STRING,
                IDM_CONFIGURE as usize,
                TEXT_CONFIGURE,
            )?;
            AppendMenuW(
                menu.get(),
                startup_flags,
                IDM_STARTUP as usize,
                TEXT_STARTUP,
            )?;
            AppendMenuW(menu.get(), MF_STRING, IDM_EXIT as usize, TEXT_EXIT)?;
            Ok(menu)
        }
    }
}

fn icon_image_data(bytes: &[u8]) -> Result<&[u8]> {
    const ICO_HEADER_SIZE: usize = 6;
    const ICO_ENTRY_SIZE: usize = 16;
    if bytes.len() < ICO_HEADER_SIZE + ICO_ENTRY_SIZE {
        return Err(anyhow!("Tray icon resource is truncated"));
    }
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let image_type = u16::from_le_bytes([bytes[2], bytes[3]]);
    let image_count = u16::from_le_bytes([bytes[4], bytes[5]]);
    if reserved != 0 || image_type != 1 || image_count == 0 {
        return Err(anyhow!("Tray icon resource has an invalid ICO header"));
    }
    let entry = &bytes[ICO_HEADER_SIZE..ICO_HEADER_SIZE + ICO_ENTRY_SIZE];
    let image_size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
    let image_offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
    if image_size == 0 {
        return Err(anyhow!("Tray icon resource has an empty image"));
    }
    let image_end = image_offset
        .checked_add(image_size)
        .ok_or_else(|| anyhow!("Tray icon resource offset overflow"))?;
    if image_offset < ICO_HEADER_SIZE + ICO_ENTRY_SIZE || image_end > bytes.len() {
        return Err(anyhow!("Tray icon resource image is outside the ICO data"));
    }
    Ok(&bytes[image_offset..image_end])
}

struct PopupMenuGuard(HMENU);

impl PopupMenuGuard {
    fn new(menu: HMENU) -> Result<Self> {
        if menu.is_invalid() {
            return Err(anyhow!("CreatePopupMenu returned an invalid handle"));
        }
        Ok(Self(menu))
    }

    fn get(&self) -> HMENU {
        self.0
    }
}

impl Drop for PopupMenuGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        debug!("trayicon destroyed");
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
            if !self.data.hIcon.is_invalid() {
                let _ = DestroyIcon(self.data.hIcon);
            }
        }
    }
}
