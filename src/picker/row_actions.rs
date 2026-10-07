//! Shared text, action and scrollbar geometry for a single row.
use super::{messages::ViewState, skin::px, ViewKind};
use windows::Win32::{Foundation::RECT, UI::WindowsAndMessaging::*};

pub(super) struct RowActions {
    pub left: i32,
    pub right: i32,
    pub width: i32,
}

pub(super) fn layout(state: &ViewState, row: RECT, confirming: bool) -> RowActions {
    let dpi = state.dpi.get();
    let height = (row.bottom - row.top).max(1);
    let width = px(28, dpi).min(height);
    let right = if state.kind == ViewKind::Details {
        row.right + px(56, dpi)
    } else {
        let mut bounds = RECT::default();
        let count = unsafe { SendMessageW(state.list.get(), LB_GETCOUNT, None, None) }.0;
        let overflow = unsafe { GetClientRect(state.list.get(), &mut bounds) }.is_ok()
            && count > i64::from((bounds.bottom / height).max(1)) as isize;
        row.right - px(if overflow { 17 } else { 2 }, dpi)
    };
    RowActions {
        left: right - width * if confirming { 2 } else { 1 },
        right,
        width,
    }
}
