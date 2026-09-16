use super::window::PaintState;
use crate::utils::{gdi::SavedDc, get_window_user_data, set_window_user_data};
use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::*,
};

unsafe fn paint_content(hwnd: HWND, dc: HDC, state: &PaintState) -> Result<()> {
    if state.background.get().is_invalid() {
        return Ok(());
    }
    let mut bounds = RECT::default();
    GetClientRect(hwnd, &mut bounds).context("preview stage=paint-bounds")?;
    ensure!(
        FillRect(dc, &bounds, state.background.get()) != 0,
        "preview stage=paint-background failed"
    );
    ensure!(
        FrameRect(dc, &bounds, state.border.get()) != 0,
        "preview stage=paint-border failed"
    );
    if state.placeholder.get().is_none() {
        return Ok(());
    }
    let saved = SavedDc::new(dc)?;
    saved.select(state.font.get())?;
    ensure!(
        SetTextColor(dc, state.foreground.get()).0 != u32::MAX,
        "preview stage=text-color failed"
    );
    ensure!(
        SetBkMode(dc, TRANSPARENT) != 0,
        "preview stage=text-background failed"
    );
    let padding = state.padding.get();
    bounds.left += padding;
    bounds.right -= padding;
    bounds.top += padding;
    bounds.bottom -= padding;
    let mut units: Vec<_> = state.label().encode_utf16().collect();
    let mut measured = bounds;
    let flags = DT_CENTER | DT_WORDBREAK | DT_NOPREFIX;
    ensure!(
        DrawTextW(dc, &mut units, &mut measured, flags | DT_CALCRECT) > 0,
        "preview stage=measure-placeholder failed"
    );
    bounds.top += ((bounds.bottom - bounds.top - (measured.bottom - measured.top)) / 2).max(0);
    ensure!(
        DrawTextW(dc, &mut units, &mut bounds, flags) > 0,
        "preview stage=paint-placeholder failed"
    );
    Ok(())
}

pub(super) unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        _ => {}
    }
    let pointer = get_window_user_data(hwnd);
    if pointer != 0 {
        let state = &*(pointer as *const PaintState);
        match msg {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                let result = paint_content(hwnd, dc, state);
                if !EndPaint(hwnd, &paint).as_bool() {
                    warn!("preview stage=end-paint failed");
                }
                if let Err(error) = result {
                    warn!("preview stage=paint error={error:#}");
                }
                return LRESULT(0);
            }
            WM_SHOWWINDOW => {
                state.visible.set(wparam.0 != 0);
            }
            WM_CLOSE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                state
                    .target
                    .try_post(crate::keyboard::dispatch::WM_INPUT_READY);
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                state.alive.set(false);
                state.visible.set(false);
                set_window_user_data(hwnd, 0);
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_THEMECHANGED | WM_SETTINGCHANGE => {
                state.target.try_post(crate::app::WM_SCENE_INVALIDATED);
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
