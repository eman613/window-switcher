use std::mem::size_of;

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
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
            match msg {
                WM_MOUSEMOVE => {
                    super::scroll_visibility::reveal(state);
                    let hover = item_at(hwnd, lparam);
                    if state.hover.replace(hover) != hover {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    track(hwnd);
                }
                WM_MOUSELEAVE => {
                    state.hover.set(None);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                WM_LBUTTONDOWN if !state.busy.get() => {
                    state
                        .pressed
                        .set(item_at(hwnd, lparam).map(|index| (state.epoch.get(), index)));
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                WM_LBUTTONUP => {
                    let pressed = state.pressed.take();
                    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                    if pressed.is_some_and(|(epoch, index)| {
                        epoch == state.epoch.get() && Some(index) == item_at(hwnd, lparam)
                    }) {
                        state.accept();
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return result;
                }
                WM_CAPTURECHANGED | WM_CANCELMODE => {
                    state.pressed.set(None);
                    let _ = InvalidateRect(Some(hwnd), None, false);
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
        if matches!(msg, WM_CHAR | WM_SYSCHAR) && matches!(wparam.0, 0x01 | 0x09 | 0x0d | 0x1b) {
            return LRESULT(0);
        }
    }
    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
    if hwnd == state.list.get() && msg == WM_KEYDOWN {
        super::scroll_visibility::reveal(state);
        state.signal(0);
    }
    result
}
