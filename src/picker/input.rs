use std::mem::size_of;

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::InvalidateRect,
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::KeyboardAndMouse::{
            GetKeyState, IsWindowEnabled, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
            VK_RETURN,
        },
        Shell::{DefSubclassProc, RemoveWindowSubclass},
        WindowsAndMessaging::*,
    },
};

use super::{
    messages::{ViewState, BACK, BACK_ID, CANCEL, CLEAR_ID, DISMISS_ID},
    ViewKind,
};

unsafe fn item_at(hwnd: HWND, point: LPARAM) -> Option<usize> {
    let value = SendMessageW(hwnd, LB_ITEMFROMPOINT, None, Some(point)).0 as usize;
    let count = SendMessageW(hwnd, LB_GETCOUNT, None, None).0;
    if value >> 16 != 0 || (value & 0xffff) >= count.max(0) as usize {
        return None;
    }
    let index = value & 0xffff;
    let mut bounds = RECT::default();
    if SendMessageW(
        hwnd,
        LB_GETITEMRECT,
        Some(WPARAM(index)),
        Some(LPARAM((&mut bounds as *mut RECT) as isize)),
    )
    .0 < 0
    {
        return None;
    }
    let x = (point.0 as u16 as i16) as i32;
    let y = ((point.0 >> 16) as u16 as i16) as i32;
    (x >= bounds.left && x < bounds.right && y >= bounds.top && y < bounds.bottom).then_some(index)
}

unsafe fn track(hwnd: HWND) {
    let mut tracking = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        ..Default::default()
    };
    if let Err(error) = TrackMouseEvent(&mut tracking) {
        debug!("picker stage=track-leave code={:#x}", error.code().0);
    }
}

unsafe fn tab(state: &ViewState, hwnd: HWND) {
    let mut controls = if state.kind == ViewKind::Details {
        vec![state.list.get(), state.back.get()]
    } else {
        vec![
            state.edit.get(),
            state.clear.get(),
            state.dismiss.get(),
            state.list.get(),
        ]
    };
    controls.extend(super::close_confirmation::buttons(state));
    controls.retain(|control| {
        !control.is_invalid()
            && IsWindowEnabled(*control).as_bool()
            && IsWindowVisible(*control).as_bool()
    });
    if controls.is_empty() {
        return;
    }
    let current = controls
        .iter()
        .position(|control| *control == hwnd)
        .unwrap_or(0);
    let index = if GetKeyState(0x10) < 0 {
        (current + controls.len() - 1) % controls.len()
    } else {
        (current + 1) % controls.len()
    };
    let _ = SetFocus(Some(controls[index]));
}

pub(super) unsafe extern "system" fn control_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let state = &*(data as *const ViewState);
    if msg == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(control_proc), id);
        return DefSubclassProc(hwnd, msg, wparam, lparam);
    }
    if msg == WM_KEYUP && wparam.0 == VK_RETURN.0 as usize {
        state.suppress_enter.set(false);
    }
    if state.visible.get() && super::close_confirmation::buttons(state).contains(&hwnd) {
        if msg == WM_LBUTTONDOWN || (msg == WM_KEYDOWN && wparam.0 == 0x20) {
            super::close_confirmation::press(state, hwnd);
        }
        match msg {
            WM_MOUSEMOVE => {
                if state.hot_control.replace(hwnd) != hwnd {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                track(hwnd);
            }
            WM_MOUSELEAVE => {
                state.hot_control.set(HWND::default());
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            _ => {}
        }
    }
    if hwnd == state.edit.get() {
        match msg {
            WM_IME_STARTCOMPOSITION => {
                state.composing.set(true);
                state.changed();
            }
            WM_IME_ENDCOMPOSITION => {
                state.composing.set(false);
                state
                    .suppress_enter
                    .set(GetKeyState(VK_RETURN.0 as i32) < 0);
                state.changed();
            }
            _ => {}
        }
    }
    if state.kind == ViewKind::Search && state.visible.get() {
        if hwnd == state.dismiss.get() && msg == WM_LBUTTONDBLCLK {
            // An owner-drawn BUTTON otherwise sends BN_DOUBLECLICKED and drops
            // the second release. Help is a toggle with ordinary button presses.
            debug!("search stage=help double-click-as-press");
            return DefSubclassProc(hwnd, WM_LBUTTONDOWN, wparam, lparam);
        }
        if msg == WM_MOUSEWHEEL && super::scrollbar::wheel(state, wparam) {
            return LRESULT(0);
        }
        if hwnd == state.list.get() {
            if let Some(result) = super::scrollbar::handle_list(state, msg, lparam) {
                return result;
            }
            match msg {
                WM_MOUSEMOVE => {
                    let mut point = POINT::default();
                    if GetCursorPos(&mut point).is_ok()
                        && state.pointer.replace(Some((point.x, point.y)))
                            != Some((point.x, point.y))
                    {
                        super::scroll_visibility::reveal(state);
                    }
                    if let Ok(parent) = GetParent(hwnd) {
                        let hot = super::scrollbar::pointer_on_track(parent, state);
                        if state.scroll_hot.replace(hot) != hot {
                            super::scrollbar::invalidate(state);
                        }
                    }
                    let hover = item_at(hwnd, lparam);
                    super::repaint::hover(state, hover);
                    track(hwnd);
                }
                WM_MOUSELEAVE => {
                    state.pointer.set(None);
                    super::repaint::hover(state, None);
                    if state.scroll_hot.replace(false) {
                        super::scrollbar::invalidate(state);
                    }
                }
                WM_LBUTTONDOWN if !state.busy.get() => {
                    super::repaint::pressed(
                        state,
                        item_at(hwnd, lparam)
                            .and_then(|index| super::repaint::identity(state, index)),
                    );
                }
                WM_LBUTTONUP => {
                    let pressed = state.pressed.get();
                    super::repaint::pressed(state, None);
                    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                    if let Some(index) = item_at(hwnd, lparam) {
                        if pressed.is_some() && pressed == super::repaint::identity(state, index) {
                            SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(index)), None);
                            state.accept();
                        }
                    }
                    return result;
                }
                WM_CAPTURECHANGED | WM_CANCELMODE => {
                    super::repaint::pressed(state, None);
                }
                WM_VSCROLL => {
                    state.hover.set(None);
                    state.pressed.set(None);
                    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                    state.signal(0);
                    return result;
                }
                _ => {}
            }
        } else if hwnd == state.clear.get() || hwnd == state.dismiss.get() {
            match msg {
                WM_MOUSEMOVE => {
                    if state.hot_control.replace(hwnd) != hwnd {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    track(hwnd);
                }
                WM_MOUSELEAVE => {
                    state.hot_control.set(HWND::default());
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                _ => {}
            }
        }
    }
    if state.visible.get() && !state.composing.get() {
        if msg == WM_KEYDOWN || (msg == WM_SYSKEYDOWN && state.kind == ViewKind::Details) {
            match wparam.0 {
                0x57 if GetKeyState(0x11) < 0 && GetKeyState(0x12) >= 0 => {
                    if lparam.0 & (1 << 30) == 0 {
                        super::close_confirmation::command(state, super::close_confirmation::CLOSE);
                    }
                    return LRESULT(0);
                }
                0x0d if super::close_confirmation::confirming(state) => {
                    if lparam.0 & (1 << 30) == 0 {
                        state.suppress_enter.set(true);
                        super::close_confirmation::keyboard_confirm(state, hwnd);
                    }
                    return LRESULT(0);
                }
                0x1b if super::close_confirmation::escape(state) => return LRESULT(0),
                0x41 if hwnd == state.edit.get() && GetKeyState(0x11) < 0 => {
                    SendMessageW(
                        hwnd,
                        windows::Win32::UI::Controls::EM_SETSEL,
                        Some(WPARAM(0)),
                        Some(LPARAM(-1)),
                    );
                    return LRESULT(0);
                }
                0x0d if !state.suppress_enter.get() => {
                    if hwnd == state.back.get() {
                        state.command(BACK_ID);
                    } else if hwnd == state.clear.get() {
                        state.command(CLEAR_ID);
                    } else if hwnd == state.dismiss.get() {
                        state.command(DISMISS_ID);
                    } else if super::close_confirmation::buttons(state).contains(&hwnd) {
                        if lparam.0 & (1 << 30) == 0 {
                            state.suppress_enter.set(true);
                            super::close_confirmation::native_command(
                                state,
                                super::close_confirmation::CLOSE,
                            );
                        }
                    } else {
                        state.accept();
                    }
                    return LRESULT(0);
                }
                0x1b => {
                    state.signal(if state.kind == ViewKind::Details {
                        BACK
                    } else {
                        CANCEL
                    });
                    return LRESULT(0);
                }
                0x09 => {
                    if state.kind == ViewKind::Search
                        && hwnd == state.edit.get()
                        && !state.busy.get()
                    {
                        SendMessageW(
                            state.list.get(),
                            WM_KEYDOWN,
                            Some(WPARAM(if GetKeyState(0x10) < 0 { 0x26 } else { 0x28 })),
                            None,
                        );
                        state.signal(0);
                        return LRESULT(0);
                    }
                    tab(state, hwnd);
                    return LRESULT(0);
                }
                0x21 | 0x22 | 0x26 | 0x28 if hwnd == state.edit.get() && !state.busy.get() => {
                    SendMessageW(state.list.get(), msg, Some(wparam), Some(lparam));
                    state.signal(0);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        if matches!(msg, WM_CHAR | WM_SYSCHAR)
            && matches!(wparam.0, 0x01 | 0x09 | 0x0d | 0x1b | 0x17)
        {
            return LRESULT(0);
        }
    }
    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
    if matches!(
        msg,
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_SETFOCUS | WM_KILLFOCUS
    ) && (hwnd == state.list.get() || super::close_confirmation::buttons(state).contains(&hwnd))
    {
        if msg == WM_MOUSEMOVE {
            track(hwnd);
        }
        super::close_confirmation::refresh(state);
    }
    if hwnd == state.list.get() && matches!(msg, WM_MOUSEWHEEL | WM_VSCROLL) {
        super::close_confirmation::refresh(state);
    }
    if hwnd == state.list.get() && msg == WM_KEYDOWN {
        super::scroll_visibility::reveal(state);
        state.signal(0);
    }
    result
}
