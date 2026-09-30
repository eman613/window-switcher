//! Avoid repainting unchanged native children during inline help updates.
use crate::utils::gdi::OwnedGdiObject;
use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{CreateRoundRectRgn, ScreenToClient, SetWindowRgn, HGDIOBJ},
    UI::WindowsAndMessaging::*,
};

pub(super) fn move_child(parent: HWND, child: HWND, bounds: RECT) -> Result<()> {
    let mut current = RECT::default();
    unsafe { GetWindowRect(child, &mut current) }.context("picker stage=child-bounds")?;
    let mut origin = POINT {
        x: current.left,
        y: current.top,
    };
    ensure!(
        unsafe { ScreenToClient(parent, &mut origin) }.as_bool(),
        "picker stage=child-origin failed"
    );
    if origin.x == bounds.left
        && origin.y == bounds.top
        && current.right - current.left == bounds.right - bounds.left
        && current.bottom - current.top == bounds.bottom - bounds.top
    {
        return Ok(());
    }
    unsafe {
        MoveWindow(
            child,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            true,
        )
    }
    .context("picker stage=control-layout")
}

pub(super) fn region(parent: HWND, bounds: RECT, radius: i32) -> Result<()> {
    if radius > 0 {
        let region = unsafe {
            CreateRoundRectRgn(
                0,
                0,
                bounds.right + 1,
                bounds.bottom + 1,
                radius * 2,
                radius * 2,
            )
        };
        let owned = OwnedGdiObject::new(HGDIOBJ(region.0), "picker-region")?;
        ensure!(
            unsafe { SetWindowRgn(parent, Some(region), true) } != 0,
            "picker stage=window-region failed"
        );
        std::mem::forget(owned); // USER32 now owns the region.
    } else {
        ensure!(
            unsafe { SetWindowRgn(parent, None, true) } != 0,
            "picker stage=window-region reset failed"
        );
    }
    Ok(())
}

pub(super) fn visible(hwnd: HWND, visible: bool) {
    let shown = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32 & WS_VISIBLE.0 != 0;
    if shown != visible {
        let _ = unsafe { ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE }) };
    }
}
