mod style;

use super::{
    layout::PickerLayout,
    messages::{
        self, ViewState, BACK_ID, DISMISS_ID, EDIT_ID, HELP_ID, LABEL_ID, LIST_ID, NOTICE_ID,
        RESULTS_ID, STATUS_ID,
    },
    skin::{px, SearchSkin},
    ViewKind,
};
use crate::{
    appearance::Appearance,
    config::Config,
    localization::Text,
    utils::gdi::{content_font, OwnedGdiObject},
};
use anyhow::{ensure, Context, Result};
use std::cell::Cell;
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::Gdi::{CreateSolidBrush, GetObjectW, InvalidateRect, HBRUSH, HGDIOBJ, LOGFONTW},
        System::{
            LibraryLoader::GetModuleHandleW,
            SystemServices::{SS_CENTER, SS_CENTERIMAGE, SS_ENDELLIPSIS, SS_NOPREFIX, SS_RIGHT},
        },
        UI::{
            Input::KeyboardAndMouse::EnableWindow, Shell::SetWindowSubclass, WindowsAndMessaging::*,
        },
    },
};

pub(super) struct Controls {
    pub edit: HWND,
    pub list: HWND,
    pub status: HWND,
    back: HWND,
    label: HWND,
    results_label: HWND,
    clear: HWND,
    dismiss: HWND,
    notice: HWND,
    help: HWND,
    font: Option<OwnedGdiObject>,
    background: Option<OwnedGdiObject>,
    region: Cell<Option<(i32, i32, u32, i32)>>,
    text_height: Cell<i32>,
    styled: Option<(Config, u32)>,
}

pub(super) fn child(
    parent: HWND,
    class: PCWSTR,
    name: &str,
    style: WINDOW_STYLE,
    id: usize,
) -> Result<HWND> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &HSTRING::from(name),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | style,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as _)),
            Some(GetModuleHandleW(None)?.into()),
            None,
        )
    }
    .context("search stage=create-control")
}

impl Controls {
    pub(super) fn create(parent: HWND, state: &ViewState, text: Text) -> Result<Self> {
        let search = state.kind == ViewKind::Search;
        let label = child(
            parent,
            w!("STATIC"),
            if search {
                text.search_label()
            } else {
                text.details_label()
            },
            WINDOW_STYLE(SS_NOPREFIX.0),
            LABEL_ID,
        )?;
        let edit = if search {
            child(
                parent,
                w!("EDIT"),
                "",
                WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                EDIT_ID,
            )?
        } else {
            HWND::default()
        };
        state.edit.set(edit);
        let results_label = if search {
            child(
                parent,
                w!("STATIC"),
                text.search_results_label(),
                WINDOW_STYLE(SS_NOPREFIX.0),
                RESULTS_ID,
            )?
        } else {
            HWND::default()
        };
        let list_style = if search {
            WINDOW_STYLE((LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32)
        } else {
            WS_BORDER
        };
        let list = child(
            parent,
            w!("LISTBOX"),
            "",
            (if search { WINDOW_STYLE(0) } else { WS_VSCROLL })
                | WS_TABSTOP
                | list_style
                | WINDOW_STYLE((LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
            LIST_ID,
        )?;
        state.list.set(list);
        let back = if !search {
            child(
                parent,
                w!("BUTTON"),
                text.details_back(),
                WS_TABSTOP,
                BACK_ID,
            )?
        } else {
            HWND::default()
        };
        state.back.set(back);
        let status = child(
            parent,
            w!("STATIC"),
            text.search_loading(),
            WINDOW_STYLE(SS_NOPREFIX.0 | if search { SS_RIGHT.0 } else { 0 }),
            STATUS_ID,
        )?;
        let clear = HWND::default();
        let dismiss = if search {
            child(
                parent,
                w!("BUTTON"),
                text.search_help_toggle(),
                WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                DISMISS_ID,
            )?
        } else {
            HWND::default()
        };
        state.clear.set(clear);
        state.dismiss.set(dismiss);
        super::close_confirmation::create(parent, state)?;
        let notice = if search {
            child(
                parent,
                w!("STATIC"),
                "",
                WINDOW_STYLE(SS_CENTER.0 | SS_NOPREFIX.0),
                NOTICE_ID,
            )?
        } else {
            HWND::default()
        };
        let help = if search {
            child(
                parent,
                w!("STATIC"),
                "",
                WINDOW_STYLE(SS_RIGHT.0 | SS_CENTERIMAGE.0 | SS_ENDELLIPSIS.0 | SS_NOPREFIX.0),
                HELP_ID,
            )?
        } else {
            HWND::default()
        };
        unsafe {
            if search {
                SendMessageW(
                    edit,
                    windows::Win32::UI::Controls::EM_SETLIMITTEXT,
                    Some(WPARAM(super::MAX_QUERY_UNITS)),
                    None,
                );
                let _ = EnableWindow(clear, false);
            }
            for hwnd in [edit, list, back, clear, dismiss]
                .into_iter()
                .filter(|hwnd| !hwnd.is_invalid())
            {
                ensure!(
                    SetWindowSubclass(
                        hwnd,
                        Some(messages::control_proc),
                        1,
                        state as *const _ as usize
                    )
                    .as_bool(),
                    "search stage=subclass failed"
                );
            }
        }
        Ok(Self {
            edit,
            list,
            status,
            back,
            label,
            results_label,
            clear,
            dismiss,
            notice,
            help,
            font: None,
            background: None,
            region: Cell::new(None),
            text_height: Cell::new(14),
            styled: None,
        })
    }

    fn set_font(hwnd: HWND, font: &OwnedGdiObject) {
        if !hwnd.is_invalid() {
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETFONT,
                    Some(WPARAM(font.0 .0 as usize)),
                    Some(LPARAM(1)),
                )
            };
        }
    }

    pub(super) fn metrics(&self, state: &ViewState, dpi: u32) -> (i32, i32) {
        state
            .visual
            .try_borrow()
            .ok()
            .and_then(|visual| {
                visual
                    .skin
                    .as_ref()
                    .map(|skin| (skin.row_height, skin.secondary_height))
            })
            .unwrap_or((
                (self.text_height.get() * 127 / 100 + px(12, dpi)).max(px(30, dpi)),
                self.text_height.get(),
            ))
    }

    pub(super) fn layout(&self, parent: HWND, state: &ViewState, dpi: u32) -> Result<()> {
        let mut bounds = RECT::default();
        unsafe { GetClientRect(parent, &mut bounds) }.context("search stage=client-bounds")?;
        let top = unsafe { SendMessageW(self.list, LB_GETTOPINDEX, None, None) };
        let (row_height, text_height) = self.metrics(state, dpi);
        let mut layout = PickerLayout::calculate(
            bounds.right,
            bounds.bottom,
            dpi,
            state.kind,
            row_height,
            text_height,
            state.help_open.get() || state.truncated.get(),
        )?;
        if state.kind == ViewKind::Details && super::close_confirmation::enabled(state) {
            layout.list.right -= px(59, dpi);
        }
        state.query_bottom.set(layout.query_bottom);
        for (hwnd, rect) in [
            (self.label, layout.label),
            (self.edit, layout.edit),
            (self.results_label, layout.results_label),
            (self.list, layout.list),
            (self.status, layout.status),
            (self.back, layout.back),
            (self.clear, layout.clear),
            (self.dismiss, layout.dismiss),
            (self.notice, layout.notice),
            (self.help, layout.help),
        ]
        .into_iter()
        .filter(|(hwnd, _)| !hwnd.is_invalid())
        {
            super::placement::move_child(parent, hwnd, rect)?;
        }
        if state.kind == ViewKind::Search {
            unsafe {
                for control in [self.results_label, self.status, self.clear] {
                    if !control.is_invalid() {
                        let _ = ShowWindow(control, SW_HIDE);
                    }
                }
                let _ = ShowWindow(self.label, SW_HIDE);
            }
        }
        if unsafe { SendMessageW(self.list, LB_GETITEMHEIGHT, Some(WPARAM(0)), None) }.0
            != layout.row_height as isize
        {
            let result = unsafe {
                SendMessageW(
                    self.list,
                    LB_SETITEMHEIGHT,
                    Some(WPARAM(0)),
                    Some(LPARAM(layout.row_height as isize)),
                )
            }
            .0;
            ensure!(result != LB_ERR as isize, "search stage=row-height failed");
        }
        if unsafe { SendMessageW(self.list, LB_GETCOUNT, None, None) }.0 > 0
            && unsafe { SendMessageW(self.list, LB_GETTOPINDEX, None, None) } != top
        {
            unsafe {
                SendMessageW(
                    self.list,
                    LB_SETTOPINDEX,
                    Some(WPARAM(top.0.max(0) as usize)),
                    None,
                );
            }
        }
        if state.kind == ViewKind::Search {
            let radius = state
                .visual
                .try_borrow()
                .context("picker stage=region reentrant state")?
                .skin
                .as_ref()
                .map_or(0, |skin| skin.panel_radius)
                .min(bounds.right.min(bounds.bottom) / 2);
            let signature = (bounds.right, bounds.bottom, dpi, radius);
            if self.region.get() == Some(signature) {
                return self.refresh_notice(state);
            }
            // SetWindowRgn can send WM_SIZE. Record only a successful assignment,
            // so that its following layout does not assign the same region again.
            super::placement::region(parent, bounds, radius)?;
            self.region.set(Some(signature));
        }
        self.refresh_notice(state)
    }

    pub(super) fn refresh_notice(&self, state: &ViewState) -> Result<()> {
        super::close_confirmation::refresh(state);
        if state.kind != ViewKind::Search {
            return Ok(());
        }
        let count = unsafe { SendMessageW(self.list, LB_GETCOUNT, None, None) }.0;
        let show = count <= 0 || state.failed.get();
        let text = state.text;
        let title = if state.failed.get() {
            text.search_failed_title()
        } else if state.busy.get() {
            text.search_loading()
        } else {
            text.search_empty_title()
        };
        unsafe {
            SetWindowTextW(self.notice, &HSTRING::from(title))?;
            SetWindowTextW(
                self.help,
                &HSTRING::from(if state.truncated.get() && !state.help_open.get() {
                    text.search_truncated_hint()
                } else {
                    text.search_keyboard_help()
                }),
            )?;
        }
        super::placement::visible(self.notice, show);
        super::placement::visible(self.help, state.help_open.get() || state.truncated.get());
        super::placement::visible(self.list, !show);
        state.announce();
        Ok(())
    }
}
