//! Search viewport chrome. The list remains the source of scroll position.
use super::{drawing::rounded, layout::PickerLayout, messages::ViewState, skin::px, ViewKind};
use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{ClientToScreen, IntersectClipRect, InvalidateRect, ScreenToClient, HDC},
    UI::{
        Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture},
        WindowsAndMessaging::*,
    },
};

struct Geometry {
    viewport: RECT,
    track: RECT,
    thumb: RECT,
    maximum: i32,
    page: i32,
    top: i32,
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
        state.help_open.get(),
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
        left: layout.list.right - px(14, skin.dpi),
        right: layout.list.right - px(2, skin.dpi),
        top: layout.list.top,
        bottom: layout.list.bottom,
    };
    let height = track.bottom - track.top;
    let thumb_height = ((i64::from(height) * i64::from(page) / i64::from(count)) as i32)
        .max(px(28, skin.dpi))
        .min(height);
    let offset = (i64::from(height - thumb_height) * i64::from(top) / i64::from(maximum)) as i32;
    Some(Geometry {
        viewport: layout.list,
        track,
        thumb: RECT {
            top: track.top + offset,
            bottom: track.top + offset + thumb_height,
            ..track
        },
        maximum,
        page,
        top,
    })
}

fn wheel_rows(remainder: i32, delta: i32, lines: u32, page: i32) -> (i32, i32) {
    let delta = remainder + delta;
    // SPI_GETWHEELSCROLLLINES uses UINT_MAX for one page per wheel notch.
    let lines = lines.min(page as u32) as i32;
    (
        delta / WHEEL_DELTA as i32 * lines,
        delta % WHEEL_DELTA as i32,
    )
}

pub(super) fn wheel(state: &ViewState, wparam: WPARAM) -> bool {
    if state.kind != ViewKind::Search || !state.visible.get() {
        return false;
    }
    if state.busy.get() || state.scroll_drag.get().is_some() {
        state.wheel_remainder.set(0);
        return true;
    }
    let parent = match unsafe { GetParent(state.list.get()) } {
        Ok(parent) => parent,
        Err(error) => {
            warn!("search stage=wheel-parent code={:#x}", error.code().0);
            return true;
        }
    };
    let Some(geometry) = geometry(parent, state) else {
        state.wheel_remainder.set(0);
        return true;
    };
    super::scroll_visibility::reveal(state);
    let mut lines = 3u32;
    if let Err(error) = unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some((&mut lines as *mut u32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    } {
        warn!(
            "search stage=wheel-settings code={:#x}; using 3 lines",
            error.code().0
        );
        lines = 3;
    }
    let delta = (wparam.0 >> 16) as u16 as i16 as i32;
    let (rows, remainder) = wheel_rows(state.wheel_remainder.get(), delta, lines, geometry.page);
    state
        .wheel_remainder
        .set(if lines == 0 { 0 } else { remainder });
    let top = (geometry.top - rows).clamp(0, geometry.maximum);
    if top != geometry.top {
        let result = unsafe {
            SendMessageW(
                state.list.get(),
                LB_SETTOPINDEX,
                Some(WPARAM(top as usize)),
                None,
            )
        };
        if result.0 == LB_ERR as isize {
            warn!("search stage=wheel-position rejected top={top}");
            return true;
        }
        state.hover.set(None);
        state.pressed.set(None);
        let _ = unsafe { InvalidateRect(Some(state.list.get()), None, false) };
        state.signal(0);
    }
    debug!(
        "search stage=wheel delta={delta} lines={lines} from={} top={top}",
        geometry.top
    );
    true
}

pub(super) fn invalidate(state: &ViewState) {
    if state.kind == ViewKind::Search {
        let _ = unsafe { InvalidateRect(Some(state.list.get()), None, false) };
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

pub(super) fn paint_row(state: &ViewState, dc: HDC, row: RECT) -> Result<()> {
    if !super::scroll_visibility::visible(state) {
        return Ok(());
    }
    let hwnd =
        unsafe { GetParent(state.list.get()) }.context("search stage=scroll-paint-parent")?;
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
    let active = state.scroll_drag.get().is_some() || pointer_on_track(hwnd, state);
    let width = px(if active { 6 } else { 4 }, skin.dpi).max(2);
    let center = (geometry.thumb.left + geometry.thumb.right) / 2 - geometry.viewport.left;
    let _saved = crate::utils::gdi::SavedDc::new(dc)?;
    ensure!(
        unsafe { IntersectClipRect(dc, row.left, row.top, row.right, row.bottom) }.0 != 0,
        "search stage=scroll-clip failed"
    );
    rounded(
        dc,
        RECT {
            left: center - width / 2,
            right: center + (width + 1) / 2,
            top: geometry.thumb.top - geometry.viewport.top,
            bottom: geometry.thumb.bottom - geometry.viewport.top,
        },
        (width + 1) / 2,
        if active {
            skin.palette.accent
        } else {
            skin.palette.border
        },
        None,
    )
}

/// The overlay belongs to the list visually, while the parent retains capture
/// so dragging beyond the list still reaches both ends of the scroll range.
pub(super) unsafe fn handle_list(state: &ViewState, msg: u32, point: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_MOUSEMOVE) {
        return None;
    }
    let parent = match GetParent(state.list.get()) {
        Ok(parent) => parent,
        Err(error) => {
            warn!(
                "search stage=scroll-input-parent code={:#x}",
                error.code().0
            );
            return None;
        }
    };
    let mut position = POINT {
        x: point.0 as u16 as i16 as i32,
        y: (point.0 >> 16) as u16 as i16 as i32,
    };
    if !ClientToScreen(state.list.get(), &mut position).as_bool()
        || !ScreenToClient(parent, &mut position).as_bool()
    {
        warn!("search stage=scroll-input-coordinates failed");
        return None;
    }
    let translated =
        LPARAM(((position.x as u16 as u32) | ((position.y as u16 as u32) << 16)) as isize);
    handle(
        parent,
        state,
        if msg == WM_LBUTTONDBLCLK {
            WM_LBUTTONDOWN
        } else {
            msg
        },
        translated,
    )
}

pub(super) fn pointer_on_track(hwnd: HWND, state: &ViewState) -> bool {
    let Some(g) = geometry(hwnd, state) else {
        return false;
    };
    let mut point = POINT::default();
    (unsafe { GetCursorPos(&mut point).is_ok() && ScreenToClient(hwnd, &mut point).as_bool() })
        && point.x >= g.track.left
        && point.x < g.track.right
        && point.y >= g.track.top
        && point.y < g.track.bottom
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
    if state.scroll_mode.get() == crate::config::ScrollBarMode::Hidden {
        return None;
    }
    if x >= g.track.left && x < g.track.right && y >= g.track.top && y < g.track.bottom {
        super::scroll_visibility::reveal(state);
    }
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
            viewport: RECT::default(),
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
            page: 10,
            top: 0,
        };
        assert_eq!(position(-100, 20, &g), 0);
        assert_eq!(position(400, 20, &g), 90);
        assert_eq!(position(1000, 20, &g), 90);
        assert_eq!(position(220, 20, &g), 45);
    }

    #[test]
    fn wheel_respects_partial_deltas_system_lines_and_page_mode() {
        assert_eq!(wheel_rows(0, -30, 3, 10), (0, -30));
        assert_eq!(wheel_rows(-30, -90, 3, 10), (-3, 0));
        assert_eq!(wheel_rows(-30, 30, 3, 10), (0, 0));
        assert_eq!(wheel_rows(0, 240, 3, 10), (6, 0));
        assert_eq!(wheel_rows(0, -120, u32::MAX, 10), (-10, 0));
        assert_eq!(wheel_rows(0, 120, 1000, 10), (10, 0));
        assert_eq!(wheel_rows(0, -120, 0, 10), (0, 0));
    }
}
