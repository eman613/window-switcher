use super::*;
use windows::Win32::{
    Foundation::{LPARAM, POINT, RECT, WPARAM},
    Graphics::Gdi::ScreenToClient,
    UI::{Input::KeyboardAndMouse::GetFocus, WindowsAndMessaging::*},
};

pub(super) fn hovered(state: &ViewState) -> Option<CloseEvent> {
    if unsafe { GetFocus() } == state.close.close.get() {
        if let Some(target) = state.close.offered.get() {
            return (target_at(state, target.1) == Some(target)).then_some(target);
        }
    }
    let list = state.list.get();
    let mut point = POINT::default();
    let mut bounds = RECT::default();
    unsafe {
        GetCursorPos(&mut point).ok()?;
        GetClientRect(list, &mut bounds).ok()?;
        if !ScreenToClient(list, &mut point).as_bool() || point.y < 0 || point.y >= bounds.bottom {
            return None;
        }
        let packed = LPARAM(((point.y as u32) << 16) as isize);
        let index = SendMessageW(list, LB_ITEMFROMPOINT, None, Some(packed)).0 as usize;
        if index >> 16 != 0 {
            return None;
        }
        let mut row = RECT::default();
        if SendMessageW(
            list,
            LB_GETITEMRECT,
            Some(WPARAM(index)),
            Some(LPARAM(&mut row as *mut _ as isize)),
        )
        .0 < 0
        {
            return None;
        }
        let layout = crate::picker::row_actions::layout(state, row, false);
        (point.x >= row.left
            && point.x < layout.right
            && point.y >= row.top
            && point.y < row.bottom)
            .then(|| target_at(state, index))
            .flatten()
    }
}

pub(super) fn select_offered(state: &ViewState, target: Option<CloseEvent>) -> bool {
    if state.close.pending.get().is_some() || state.close.request.get().is_some() {
        return false;
    }
    let Some(target) = target else {
        return false;
    };
    if target_at(state, target.1) != Some(target) || state.close.offered.get() != Some(target) {
        debug!("close stage=pointer-target rejected=stale");
        return false;
    }
    let previous = selected(state);
    if unsafe { SendMessageW(state.list.get(), LB_SETCURSEL, Some(WPARAM(target.1)), None) }.0 < 0 {
        warn!("close stage=pointer-selection failed");
        return false;
    }
    crate::picker::repaint::row(state, previous.map(|item| item.1));
    crate::picker::repaint::row(state, Some(target.1));
    debug!("close stage=pointer-target selected=true");
    selected(state) == Some(target)
}
