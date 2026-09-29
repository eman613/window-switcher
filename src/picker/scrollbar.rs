//! Search viewport chrome. The list remains the source of scroll position.
use super::{drawing::rounded, layout::PickerLayout, messages::ViewState, skin::px, ViewKind};
use anyhow::{Context, Result};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{InvalidateRect, HDC},
    UI::{
        Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture},
        WindowsAndMessaging::*,
    },
};

struct Geometry {
    track: RECT,
    thumb: RECT,
    maximum: i32,
}

fn geometry(hwnd: HWND, state: &ViewState) -> Option<Geometry> {
    if state.kind != ViewKind::Search {
        return None;
    }
    let visual = state.visual.try_borrow().ok()?;
    let skin = visual.skin.as_ref()?;
    let mut bounds = RECT::default();
    unsafe { GetClientRect(hwnd, &mut bounds) }.ok()?;
    let layout = PickerLayout::calculate(
        bounds.right,
        bounds.bottom,
        skin.dpi,
        ViewKind::Search,
        skin.row_height,
        skin.secondary_height,
    )
    .ok()?;
    let count = unsafe { SendMessageW(state.list.get(), LB_GETCOUNT, None, None) }
        .0
        .max(0) as i32;
    let page = ((layout.list.bottom - layout.list.top) / skin.row_height.max(1)).max(1);
    let maximum = (count - page).max(0);
    if maximum == 0 {
        return None;
    }
    let top = (unsafe { SendMessageW(state.list.get(), LB_GETTOPINDEX, None, None) }.0 as i32)
        .clamp(0, maximum);
    let track = RECT {
        left: layout.list.right + px(2, skin.dpi),
        right: bounds.right - px(6, skin.dpi),
        top: layout.list.top,
        bottom: layout.list.bottom,
    };
    let height = track.bottom - track.top;
    let thumb_height = ((i64::from(height) * i64::from(page) / i64::from(count)) as i32)
        .max(px(28, skin.dpi))
        .min(height);
    let offset = (i64::from(height - thumb_height) * i64::from(top) / i64::from(maximum)) as i32;
    Some(Geometry {
        track,
        thumb: RECT {
            top: track.top + offset,
            bottom: track.top + offset + thumb_height,
            ..track
        },
        maximum,
    })
}

pub(super) fn invalidate(state: &ViewState) {
    if state.kind == ViewKind::Search {
        if let Ok(parent) = unsafe { GetParent(state.list.get()) } {
            let _ = unsafe { InvalidateRect(Some(parent), None, false) };
        }
    }
}

pub(super) fn cancel(hwnd: HWND, state: &ViewState) {
    state.scroll_drag.set(None);
    if unsafe { GetCapture() } == hwnd {
        if let Err(error) = unsafe { ReleaseCapture() } {
            warn!("search stage=scroll-release code={:#x}", error.code().0);
        }
    }
}

pub(super) fn paint(hwnd: HWND, state: &ViewState, dc: HDC) -> Result<()> {
    let Some(geometry) = geometry(hwnd, state) else {
        return Ok(());
    };
    let visual = state
        .visual
        .try_borrow()
        .context("search stage=scroll-paint reentrant state")?;
    let Some(skin) = &visual.skin else {
        return Ok(());
    };
    let inset = px(3, skin.dpi);
    rounded(
        dc,
        RECT {
            left: geometry.thumb.left + inset,
            right: geometry.thumb.right - inset,
            ..geometry.thumb
        },
        px(3, skin.dpi),
        if state.scroll_drag.get().is_some() {
            skin.palette.accent
        } else {
            skin.palette.border
        },
        None,
    )
}

fn position(y: i32, grab: i32, geometry: &Geometry) -> i32 {
    let travel =
        geometry.track.bottom - geometry.track.top - (geometry.thumb.bottom - geometry.thumb.top);
    if travel <= 0 {
        return 0;
    }
    let offset = (y - grab - geometry.track.top).clamp(0, travel);
    ((i64::from(offset) * i64::from(geometry.maximum) + i64::from(travel / 2)) / i64::from(travel))
        as i32
}

pub(super) unsafe fn handle(
    hwnd: HWND,
    state: &ViewState,
    msg: u32,
    point: LPARAM,
) -> Option<LRESULT> {
    if state.kind != ViewKind::Search {
        return None;
    }
    if msg == WM_CANCELMODE {
        cancel(hwnd, state);
        invalidate(state);
        return None;
    }
    if msg == WM_CAPTURECHANGED {
        state.scroll_drag.set(None);
        invalidate(state);
        return None;
    }
    if msg == WM_LBUTTONUP && state.scroll_drag.take().is_some() {
        if GetCapture() == hwnd {
            if let Err(error) = ReleaseCapture() {
                warn!("search stage=scroll-release code={:#x}", error.code().0);
            }
        }
        invalidate(state);
        return Some(LRESULT(0));
    }
    if !matches!(msg, WM_LBUTTONDOWN | WM_MOUSEMOVE) {
        return None;
    }
    let g = geometry(hwnd, state)?;
    let x = point.0 as u16 as i16 as i32;
    let y = (point.0 >> 16) as u16 as i16 as i32;
    if msg == WM_LBUTTONDOWN {
        if state.busy.get()
            || x < g.track.left
            || x >= g.track.right
            || y < g.track.top
            || y >= g.track.bottom
        {
            return None;
        }
        let grab = if y >= g.thumb.top && y < g.thumb.bottom {
            y - g.thumb.top
        } else {
            (g.thumb.bottom - g.thumb.top) / 2
        };
        SetCapture(hwnd);
        if GetCapture() != hwnd {
            warn!("search stage=scroll-capture failed");
            return Some(LRESULT(0));
        }
        state.scroll_drag.set(Some(grab));
        state.pressed.set(None);
    }
    if let Some(grab) = state.scroll_drag.get() {
        let top = position(y, grab, &g);
        SendMessageW(
            state.list.get(),
            LB_SETTOPINDEX,
            Some(WPARAM(top as usize)),
            None,
        );
        state.signal(0);
        return Some(LRESULT(0));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dragging_to_both_ends_reaches_the_full_range_without_wrapping() {
        let g = Geometry {
            track: RECT {
                top: 20,
                bottom: 420,
                ..Default::default()
            },
            thumb: RECT {
                top: 20,
                bottom: 60,
                ..Default::default()
            },
            maximum: 90,
        };
        assert_eq!(position(-100, 20, &g), 0);
        assert_eq!(position(400, 20, &g), 90);
        assert_eq!(position(1000, 20, &g), 90);
        assert_eq!(position(220, 20, &g), 45);
    }
}
