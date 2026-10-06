//! Window callbacks own only picker state, never an App borrow or a search job.
use super::{rows::PickerVisual, skin::px, ViewKind};
use crate::{
    keyboard::dispatch::WM_INPUT_READY,
    localization::Text,
    utils::{get_window_user_data, set_window_user_data},
    window_target::WindowTarget,
};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
use windows::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        FillRect, InvalidateRect, ScreenToClient, SetBkColor, SetTextColor, HBRUSH, HDC,
    },
    UI::{
        Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODT_LISTBOX},
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::*,
    },
};

pub(super) use super::input::control_proc;
pub(super) const LABEL_ID: usize = 100;
pub(super) const EDIT_ID: usize = 101;
pub(super) const LIST_ID: usize = 102;
pub(super) const RESULTS_ID: usize = 103;
pub(super) const STATUS_ID: usize = 104;
pub(super) const BACK_ID: usize = 105;
pub(super) const CLEAR_ID: usize = 106;
pub(super) const DISMISS_ID: usize = 110;
pub(super) const NOTICE_ID: usize = 108;
pub(super) const HELP_ID: usize = 109;
pub(crate) const CHANGED: u32 = 1;
pub(crate) const CANCEL: u32 = 2;
pub(crate) const RELAYOUT: u32 = 4;
pub(crate) const BACK: u32 = 8;

pub(super) struct ViewState {
    pub close: super::close_confirmation::CloseState,
    pub announcer: RefCell<Option<crate::accessibility::announcement::Announcer>>,
    pub alive: Cell<bool>,
    pub target: Arc<WindowTarget>,
    pub kind: ViewKind,
    pub text: Text,
    pub edit: Cell<HWND>,
    pub list: Cell<HWND>,
    pub back: Cell<HWND>,
    pub clear: Cell<HWND>,
    pub dismiss: Cell<HWND>,
    pub visible: Cell<bool>,
    pub busy: Cell<bool>,
    pub failed: Cell<bool>,
    pub composing: Cell<bool>,
    pub suppress_enter: Cell<bool>,
    pub flags: Cell<u32>,
    pub epoch: Cell<u64>,
    pub accept: Cell<Option<(u64, usize)>>,
    pub background: Cell<COLORREF>,
    pub foreground: Cell<COLORREF>,
    pub muted: Cell<COLORREF>,
    pub brush: Cell<HBRUSH>,
    pub dpi: Cell<u32>,
    pub monitor_dirty: Cell<bool>,
    pub query_bottom: Cell<i32>,
    pub visual: RefCell<PickerVisual>,
    pub hover: Cell<Option<usize>>,
    pub pressed: Cell<Option<crate::utils::window_identity::WindowIdentity>>,
    pub pointer: Cell<Option<(i32, i32)>>,
    pub scroll_hot: Cell<bool>,
    pub scroll_geometry: Cell<Option<(i32, i32, i32, i32)>>,
    pub row_buffer: RefCell<Option<crate::render_surface::RenderSurface>>,
    pub status_text: RefCell<String>,
    pub hot_control: Cell<HWND>,
    pub paint_error: Cell<bool>,
    pub reset_scroll: Cell<bool>,
    pub scroll_drag: Cell<Option<i32>>,
    pub wheel_remainder: Cell<i32>,
    pub scroll_mode: Cell<crate::config::ScrollBarMode>,
    pub scroll_hint: Cell<bool>,
    pub help_open: Cell<bool>,
    pub truncated: Cell<bool>,
    pub style_dirty: Cell<bool>,
}

impl ViewState {
    pub(super) fn focus_target(&self) -> HWND {
        if self.kind == ViewKind::Search {
            self.edit.get()
        } else if self.busy.get()
            || unsafe { SendMessageW(self.list.get(), LB_GETCOUNT, None, None) }.0 <= 0
        {
            self.back.get()
        } else {
            self.list.get()
        }
    }

    pub(super) fn signal(&self, flags: u32) {
        if flags & (CHANGED | CANCEL | BACK) != 0 {
            self.close.cancel();
        }
        super::close_confirmation::refresh(self);
        if self.visible.get() {
            if flags & (CHANGED | CANCEL | BACK) != 0 {
                self.cancel_announcement(true);
            } else {
                self.announce();
            }
            super::scrollbar::refresh(self);
            self.flags.set(self.flags.get() | flags);
            self.target.try_post(WM_INPUT_READY);
        }
    }

    pub(super) fn changed(&self) {
        self.close.cancel();
        super::close_confirmation::refresh(self);
        self.cancel_announcement(true);
        self.reset_scroll.set(true);
        self.wheel_remainder.set(0);
        self.busy.set(true);
        self.failed.set(false);
        self.accept.set(None);
        self.pressed.set(None);
        unsafe {
            let _ = EnableWindow(self.list.get(), false);
            if !self.clear.get().is_invalid() {
                let _ = EnableWindow(
                    self.clear.get(),
                    !self.composing.get() && GetWindowTextLengthW(self.edit.get()) > 0,
                );
                let _ = InvalidateRect(Some(self.edit.get()), None, true);
            }
        }
        if !self.composing.get() {
            self.signal(CHANGED);
        }
    }

    pub(super) fn accept(&self) {
        if self.visible.get() && !self.busy.get() && !self.composing.get() {
            let index = unsafe { SendMessageW(self.list.get(), LB_GETCURSEL, None, None) }.0;
            if index >= 0 {
                self.accept.set(Some((self.epoch.get(), index as usize)));
                self.target.try_post(WM_INPUT_READY);
            }
        }
    }

    pub(super) fn command(&self, id: usize) {
        if matches!(
            id,
            super::close_confirmation::CLOSE
                | super::close_confirmation::YES
                | super::close_confirmation::NO
        ) {
            super::close_confirmation::native_command(self, id);
            return;
        }
        match id {
            BACK_ID => self.signal(BACK),
            DISMISS_ID => {
                self.help_open.set(!self.help_open.get());
                debug!("search stage=help expanded={}", self.help_open.get());
                let _ = unsafe { SetFocus(Some(self.edit.get())) };
                self.signal(RELAYOUT);
            }
            CLEAR_ID => unsafe {
                if let Err(error) = SetWindowTextW(self.edit.get(), windows::core::w!("")) {
                    warn!("picker stage=clear-query code={:#x}", error.code().0);
                }
                let _ = SetFocus(Some(self.edit.get()));
            },
            _ => {}
        }
    }

    pub(super) fn paint_result(&self, result: anyhow::Result<()>) {
        if let Err(error) = result {
            if !self.paint_error.replace(true) {
                error!("picker stage=paint error={error:#}");
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
        if let Some(result) = super::scrollbar::handle(hwnd, state, msg, lparam) {
            return result;
        }
        match msg {
            WM_NOTIFY => {
                if let Some(result) = super::close_confirmation::notify(state, lparam) {
                    return result;
                }
            }
            WM_DRAWITEM if lparam.0 != 0 => {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                match super::close_confirmation::draw(state, item) {
                    Ok(true) => return LRESULT(1),
                    Err(error) => {
                        state.paint_result(Err(error));
                        return LRESULT(1);
                    }
                    Ok(false) => {
                        if state.kind == ViewKind::Search {
                            match super::paint::item(state, item) {
                                Ok(true) => return LRESULT(1),
                                Err(error) => {
                                    state.paint_result(Err(error));
                                    return LRESULT(1);
                                }
                                Ok(false) => {}
                            }
                        }
                    }
                }
            }
            WM_NCDESTROY => {
                if let Ok(announcer) = state.announcer.try_borrow() {
                    if let Some(announcer) = announcer.as_ref() {
                        announcer.retire();
                    }
                }
                state.alive.set(false);
                state.visible.set(false);
                state.accept.set(None);
                state.flags.set(0);
                set_window_user_data(hwnd, 0);
                debug!("picker stage=native-destroyed");
            }
            WM_TIMER if wparam.0 == super::scroll_visibility::TIMER => {
                super::scroll_visibility::tick(hwnd, state);
                return LRESULT(0);
            }
            WM_MOUSEWHEEL if super::scrollbar::wheel(state, wparam) => return LRESULT(0),
            WM_CLOSE => {
                state.signal(CANCEL);
                return LRESULT(0);
            }
            WM_GETOBJECT
                if lparam.0 as i32 == windows::Win32::UI::Accessibility::UiaRootObjectId =>
            {
                if let Ok(announcer) = state.announcer.try_borrow() {
                    if let Some(announcer) = announcer.as_ref() {
                        return windows::Win32::UI::Accessibility::UiaReturnRawElementProvider(
                            hwnd,
                            wparam,
                            lparam,
                            &announcer.root,
                        );
                    }
                }
            }
            WM_ACTIVATE if wparam.0 & 0xffff == WA_INACTIVE as usize => state.signal(CANCEL),
            WM_SETFOCUS if state.visible.get() => {
                let _ = SetFocus(Some(state.focus_target()));
            }
            WM_DPICHANGED => {
                let dpi = (wparam.0 as u32 & 0xffff).clamp(48, 768);
                state.dpi.set(dpi);
                state.monitor_dirty.set(true);
                if lparam.0 != 0 {
                    let bounds = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        bounds.left,
                        bounds.top,
                        bounds.right - bounds.left,
                        bounds.bottom - bounds.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                state.signal(RELAYOUT);
            }
            WM_EXITSIZEMOVE => {
                state.monitor_dirty.set(true);
                state.signal(RELAYOUT);
            }
            WM_SIZE => state.signal(RELAYOUT),
            WM_THEMECHANGED | WM_SETTINGCHANGE | WM_FONTCHANGE => {
                state.style_dirty.set(true);
                state.signal(RELAYOUT);
            }
            WM_NCHITTEST if state.kind == ViewKind::Search => {
                let mut point = POINT {
                    x: (lparam.0 as u16 as i16) as i32,
                    y: ((lparam.0 >> 16) as u16 as i16) as i32,
                };
                if ScreenToClient(hwnd, &mut point).as_bool()
                    && point.y >= 0
                    && point.y < state.query_bottom.get()
                    && (point.x < px(44, state.dpi.get()) || point.y < px(8, state.dpi.get()))
                {
                    return LRESULT(HTCAPTION as isize);
                }
            }
            WM_COMMAND => {
                let (id, notification) = (wparam.0 & 0xffff, (wparam.0 >> 16) as u32);
                if id == EDIT_ID && notification == EN_CHANGE {
                    state.changed();
                }
                if id == LIST_ID && notification == LBN_DBLCLK && state.kind == ViewKind::Details {
                    state.accept();
                }
                if id == LIST_ID && notification == LBN_SELCHANGE {
                    state.signal(0);
                }
                if notification == BN_CLICKED {
                    state.command(id);
                }
            }
            WM_MEASUREITEM if state.kind == ViewKind::Search && lparam.0 != 0 => {
                let measure = &mut *(lparam.0 as *mut MEASUREITEMSTRUCT);
                if measure.CtlType == ODT_LISTBOX && measure.CtlID as usize == LIST_ID {
                    measure.itemHeight = state
                        .visual
                        .try_borrow()
                        .ok()
                        .and_then(|visual| visual.skin.as_ref().map(|skin| skin.row_height))
                        .unwrap_or(px(64, state.dpi.get()))
                        as u32;
                    return LRESULT(1);
                }
            }
            WM_PAINT if state.kind == ViewKind::Search => {
                state.paint_result(super::paint::window(hwnd, state));
                return LRESULT(0);
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN
                if msg != WM_CTLCOLORBTN || state.kind == ViewKind::Search =>
            {
                let dc = HDC(wparam.0 as _);
                let id = GetDlgCtrlID(HWND(lparam.0 as _)) as usize;
                SetTextColor(
                    dc,
                    if matches!(id, LABEL_ID | RESULTS_ID | STATUS_ID | HELP_ID) {
                        state.muted.get()
                    } else {
                        state.foreground.get()
                    },
                );
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
