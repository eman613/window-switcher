//! Local invalidation and a single reusable row buffer, owned by the UI thread.
use super::messages::ViewState;
use crate::{
    render_surface::RenderSurface,
    utils::{gdi::SavedDc, window_identity::WindowIdentity},
};
use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{LPARAM, RECT, WPARAM},
    Graphics::Gdi::{BitBlt, InvalidateRect, SetWindowOrgEx, HDC, SRCCOPY},
    UI::WindowsAndMessaging::{SendMessageW, LB_GETITEMRECT},
};

pub(super) fn identity(state: &ViewState, index: usize) -> Option<WindowIdentity> {
    state
        .visual
        .try_borrow()
        .ok()?
        .rows
        .get(index)
        .map(|row| row.key.identity)
}

pub(super) fn row(state: &ViewState, index: Option<usize>) {
    let Some(index) = index else {
        return;
    };
    let mut rect = RECT::default();
    let list = state.list.get();
    if unsafe {
        SendMessageW(
            list,
            LB_GETITEMRECT,
            Some(WPARAM(index)),
            Some(LPARAM((&mut rect as *mut RECT) as isize)),
        )
    }
    .0 >= 0
    {
        let _ = unsafe { InvalidateRect(Some(list), Some(&rect), false) };
    }
}

pub(super) fn hover(state: &ViewState, next: Option<usize>) {
    let previous = state.hover.replace(next);
    if previous != next {
        row(state, previous);
        row(state, next);
    }
}

pub(super) fn pressed(state: &ViewState, next: Option<WindowIdentity>) {
    let previous = state.pressed.replace(next);
    if previous == next {
        return;
    }
    if let Ok(visual) = state.visual.try_borrow() {
        for (index, item) in visual.rows.iter().enumerate() {
            if Some(item.key.identity) == previous || Some(item.key.identity) == next {
                row(state, Some(index));
            }
        }
    }
}

pub(super) fn buffered(
    state: &ViewState,
    target: HDC,
    bounds: RECT,
    draw: impl FnOnce(HDC) -> Result<()>,
) -> Result<()> {
    let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
    if width <= 0 || height <= 0 {
        return Ok(());
    }
    let mut buffer = state
        .row_buffer
        .try_borrow_mut()
        .context("picker stage=row-buffer reentrant")?;
    if buffer
        .as_ref()
        .is_none_or(|surface| surface.width != width || surface.height != height)
    {
        *buffer = Some(RenderSurface::new(width, height)?);
    }
    let surface = buffer.as_ref().unwrap();
    let saved = SavedDc::new(surface.dc())?;
    ensure!(
        unsafe { SetWindowOrgEx(surface.dc(), bounds.left, bounds.top, None) }.as_bool(),
        "picker stage=row-origin failed"
    );
    draw(surface.dc())?;
    drop(saved);
    unsafe {
        BitBlt(
            target,
            bounds.left,
            bounds.top,
            width,
            height,
            Some(surface.dc()),
            0,
            0,
            SRCCOPY,
        )
    }
    .context("picker stage=row-present")?;
    Ok(())
}
