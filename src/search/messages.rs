//! Callbacks retain only scalar state; they never borrow App or run a search.
use crate::{
    keyboard::dispatch::WM_INPUT_READY, utils::get_window_user_data, window_target::WindowTarget,
};
use std::{cell::Cell, sync::Arc};
use windows::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{FillRect, SetBkColor, SetTextColor, HBRUSH, HDC},
    UI::{
        Input::KeyboardAndMouse::{EnableWindow, GetKeyState, SetFocus, VK_RETURN},
        Shell::{DefSubclassProc, RemoveWindowSubclass},
        WindowsAndMessaging::*,
    },
};

pub(super) const EDIT_ID: usize = 101;
pub(super) const LIST_ID: usize = 102;
pub(super) const CHANGED: u32 = 1;
pub(super) const CANCEL: u32 = 2;
pub(super) const RELAYOUT: u32 = 4;

pub(super) struct ViewState {
    pub target: Arc<WindowTarget>,
    pub edit: Cell<HWND>,
    pub list: Cell<HWND>,
    pub visible: Cell<bool>,
    pub busy: Cell<bool>,
    pub composing: Cell<bool>,
    pub suppress_enter: Cell<bool>,
    pub flags: Cell<u32>,
    pub epoch: Cell<u64>,
    pub accept: Cell<Option<(u64, usize)>>,
    pub background: Cell<COLORREF>,
    pub foreground: Cell<COLORREF>,
    pub brush: Cell<HBRUSH>,
}

impl ViewState {
    pub(super) fn signal(&self, flags: u32) {
        if self.visible.get() {
            self.flags.set(self.flags.get() | flags);
            self.target.try_post(WM_INPUT_READY);
        }
    }
    pub(super) fn changed(&self) {
        self.busy.set(true);
        self.accept.set(None);
        unsafe {
            let _ = EnableWindow(self.list.get(), false);
        }
        if !self.composing.get() {
            self.signal(CHANGED);
        }
    }
    fn accept(&self) {
        if self.visible.get() && !self.busy.get() && !self.composing.get() {
            let index = unsafe { SendMessageW(self.list.get(), LB_GETCURSEL, None, None) }.0;
            if index >= 0 {
                self.accept.set(Some((self.epoch.get(), index as usize)));
                self.target.try_post(WM_INPUT_READY);
            }
        }
    }
}

pub(super) unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = get_window_user_data(hwnd);
    if pointer != 0 {
        let state = &*(pointer as *const ViewState);
        match msg {
            WM_CLOSE => {
                state.signal(CANCEL);
                return LRESULT(0);
            }
            WM_ACTIVATE if wparam.0 & 0xffff == WA_INACTIVE as usize => state.signal(CANCEL),
            WM_SETFOCUS if state.visible.get() => {
                let _ = SetFocus(Some(state.edit.get()));
            }
            WM_SIZE | WM_DPICHANGED | WM_THEMECHANGED => state.signal(RELAYOUT),
            WM_COMMAND => {
                let (id, notification) = (wparam.0 & 0xffff, (wparam.0 >> 16) as u32);
                if id == EDIT_ID && notification == EN_CHANGE {
                    state.changed();
                }
                if id == LIST_ID && notification == LBN_DBLCLK {
                    state.accept();
                }
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let dc = HDC(wparam.0 as _);
                SetTextColor(dc, state.foreground.get());
                SetBkColor(dc, state.background.get());
                return LRESULT(state.brush.get().0 as isize);
            }
            WM_ERASEBKGND if !state.brush.get().is_invalid() => {
                let mut rect = RECT::default();
                if GetClientRect(hwnd, &mut rect).is_ok() {
                    FillRect(HDC(wparam.0 as _), &rect, state.brush.get());
                    return LRESULT(1);
                }
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
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
            WM_KEYUP if wparam.0 == VK_RETURN.0 as usize => state.suppress_enter.set(false),
            _ => {}
        }
    }
    if state.visible.get() && !state.composing.get() {
        if msg == WM_KEYDOWN {
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
                    state.accept();
                    return LRESULT(0);
                }
                0x1b => {
                    state.signal(CANCEL);
                    return LRESULT(0);
                }
                0x09 => {
                    let target = if hwnd == state.edit.get() && !state.busy.get() {
                        state.list.get()
                    } else {
                        state.edit.get()
                    };
                    let _ = SetFocus(Some(target));
                    return LRESULT(0);
                }
                0x21 | 0x22 | 0x26 | 0x28 if hwnd == state.edit.get() && !state.busy.get() => {
                    SendMessageW(state.list.get(), msg, Some(wparam), Some(lparam));
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        // TranslateMessage runs before WM_KEYDOWN is dispatched. Consume its
        // corresponding control character too; printable/IME text stays native.
        if msg == WM_CHAR && matches!(wparam.0, 0x01 | 0x09 | 0x0d | 0x1b) {
            return LRESULT(0);
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}
