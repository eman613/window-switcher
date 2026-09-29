use super::{
    layout::PickerLayout,
    messages::{
        self, ViewState, BACK_ID, CLEAR_ID, DISMISS_ID, EDIT_ID, HELP_ID, LABEL_ID, LIST_ID,
        NOTICE_ID, RESULTS_ID, STATUS_ID,
    },
    skin::{px, SearchSkin},
    ViewKind,
};
use crate::{
    appearance::Appearance,
    config::Config,
    localization::Text,
    utils::gdi::{message_font, OwnedGdiObject},
};
use anyhow::{ensure, Context, Result};
use std::cell::Cell;
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::Gdi::{
            CreateRoundRectRgn, CreateSolidBrush, DeleteObject, InvalidateRect, SetWindowRgn,
            HBRUSH, HGDIOBJ,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            SystemServices::{SS_CENTER, SS_NOPREFIX, SS_RIGHT},
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
    region: Cell<Option<(i32, i32, u32, bool)>>,
}

fn child(parent: HWND, class: PCWSTR, name: &str, style: WINDOW_STYLE, id: usize) -> Result<HWND> {
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
        let clear = if search {
            child(
                parent,
                w!("BUTTON"),
                text.search_clear(),
                WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                CLEAR_ID,
            )?
        } else {
            HWND::default()
        };
        let dismiss = if search {
            child(
                parent,
                w!("BUTTON"),
                text.search_close(),
                WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                DISMISS_ID,
            )?
        } else {
            HWND::default()
        };
        state.clear.set(clear);
        state.dismiss.set(dismiss);
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
                WINDOW_STYLE(SS_CENTER.0 | SS_NOPREFIX.0),
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

    pub(super) fn style(
        &mut self,
        parent: HWND,
        state: &ViewState,
        config: &Config,
        dpi: u32,
    ) -> Result<()> {
        state.dpi.set(dpi);
        state.paint_error.set(false);
        if state.kind == ViewKind::Search {
            let skin = SearchSkin::new(config, dpi)?;
            state
                .background
                .set(crate::text_raster::colorref(skin.palette.surface));
            state
                .foreground
                .set(crate::text_raster::colorref(skin.palette.text));
            state
                .muted
                .set(crate::text_raster::colorref(skin.palette.muted));
            state.brush.set(skin.background_brush());
            for hwnd in [
                self.label,
                self.results_label,
                self.list,
                self.status,
                self.help,
                self.clear,
                self.dismiss,
            ] {
                Self::set_font(hwnd, &skin.normal);
            }
            Self::set_font(self.edit, &skin.input);
            Self::set_font(self.notice, &skin.title);
            state
                .visual
                .try_borrow_mut()
                .context("picker stage=style reentrant update")?
                .skin = Some(skin);
        } else {
            let appearance = Appearance::capture(config);
            let color = crate::text_raster::colorref(appearance.panel.color);
            let brush = OwnedGdiObject::new(
                HGDIOBJ(unsafe { CreateSolidBrush(color) }.0),
                "search-background",
            )?;
            state.background.set(color);
            state
                .foreground
                .set(crate::text_raster::colorref(appearance.text));
            state.muted.set(state.foreground.get());
            state.brush.set(HBRUSH(brush.0 .0));
            let font = message_font(dpi)?;
            for hwnd in [self.label, self.list, self.status, self.back] {
                Self::set_font(hwnd, &font);
            }
            self.font = Some(font);
            self.background = Some(brush);
        }
        let _ = unsafe { InvalidateRect(Some(parent), None, true) };
        Ok(())
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
            .unwrap_or((px(30, dpi), px(14, dpi)))
    }

    pub(super) fn layout(&self, parent: HWND, state: &ViewState, dpi: u32) -> Result<()> {
        let mut bounds = RECT::default();
        unsafe { GetClientRect(parent, &mut bounds) }.context("search stage=client-bounds")?;
        let (row_height, text_height) = self.metrics(state, dpi);
        let layout = PickerLayout::calculate(
            bounds.right,
            bounds.bottom,
            dpi,
            state.kind,
            row_height,
            text_height,
        )?;
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
            unsafe {
                MoveWindow(
                    hwnd,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    true,
                )
            }
            .context("search stage=control-layout")?;
        }
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
        if state.kind == ViewKind::Search {
            let rounded = state
                .visual
                .try_borrow()
                .ok()
                .and_then(|visual| visual.skin.as_ref().map(|skin| !skin.palette.high_contrast))
                .unwrap_or(false);
            let signature = (bounds.right, bounds.bottom, dpi, rounded);
            if self.region.get() == Some(signature) {
                return self.refresh_notice(state);
            }
            // SetWindowRgn can send WM_SIZE. Record only a successful assignment,
            // so that its following layout does not assign the same region again.
            if rounded {
                let radius = px(24, dpi);
                let region = unsafe {
                    CreateRoundRectRgn(0, 0, bounds.right + 1, bounds.bottom + 1, radius, radius)
                };
                ensure!(!region.is_invalid(), "picker stage=window-region failed");
                if unsafe { SetWindowRgn(parent, Some(region), true) } == 0 {
                    let _ = unsafe { DeleteObject(region.into()) };
                    anyhow::bail!("picker stage=window-region assignment failed");
                }
            } else {
                ensure!(
                    unsafe { SetWindowRgn(parent, None, true) } != 0,
                    "picker stage=window-region reset failed"
                );
            }
            self.region.set(Some(signature));
        }
        self.refresh_notice(state)
    }

    pub(super) fn refresh_notice(&self, state: &ViewState) -> Result<()> {
        if state.kind != ViewKind::Search {
            return Ok(());
        }
        let count = unsafe { SendMessageW(self.list, LB_GETCOUNT, None, None) }.0;
        let show = count <= 0 || state.failed.get();
        let text = state.text;
        let (title, help) = if state.failed.get() {
            (text.search_failed_title(), text.search_failure())
        } else if state.busy.get() {
            (text.search_loading(), text.search_cancel_hint())
        } else {
            (text.search_empty_title(), text.search_empty_hint())
        };
        unsafe {
            SetWindowTextW(self.notice, &HSTRING::from(title))?;
            SetWindowTextW(self.help, &HSTRING::from(help))?;
            let _ = ShowWindow(self.notice, if show { SW_SHOWNA } else { SW_HIDE });
            let _ = ShowWindow(self.help, if show { SW_SHOWNA } else { SW_HIDE });
            let _ = ShowWindow(self.list, if show { SW_HIDE } else { SW_SHOWNA });
        }
        Ok(())
    }
}
