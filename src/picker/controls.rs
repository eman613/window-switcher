use super::{
    messages::{self, ViewState, BACK_ID, EDIT_ID, LIST_ID},
    ViewKind,
};
use crate::{
    appearance::Appearance,
    config::Config,
    localization::Text,
    utils::gdi::{message_font, OwnedGdiObject},
};
use anyhow::{ensure, Context, Result};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::Gdi::{CreateSolidBrush, HBRUSH, HGDIOBJ},
        System::LibraryLoader::GetModuleHandleW,
        UI::{Shell::SetWindowSubclass, WindowsAndMessaging::*},
    },
};

pub(super) struct Controls {
    pub edit: HWND,
    pub list: HWND,
    pub status: HWND,
    back: HWND,
    label: HWND,
    results_label: HWND,
    font: Option<OwnedGdiObject>,
    background: Option<OwnedGdiObject>,
}

fn child(parent: HWND, class: PCWSTR, name: &str, style: WINDOW_STYLE, id: usize) -> Result<HWND> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &HSTRING::from(name),
            WS_CHILD | WS_VISIBLE | style,
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
        let label = child(
            parent,
            w!("STATIC"),
            if state.kind == ViewKind::Search {
                text.search_label()
            } else {
                text.details_label()
            },
            WINDOW_STYLE(0),
            100,
        )?;
        let edit = if state.kind == ViewKind::Search {
            child(
                parent,
                w!("EDIT"),
                "",
                WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                EDIT_ID,
            )?
        } else {
            HWND::default()
        };
        state.edit.set(edit);
        let results_label = if state.kind == ViewKind::Search {
            child(
                parent,
                w!("STATIC"),
                text.search_results_label(),
                WINDOW_STYLE(0),
                103,
            )?
        } else {
            HWND::default()
        };
        let list = child(
            parent,
            w!("LISTBOX"),
            "",
            WS_BORDER
                | WS_VSCROLL
                | WS_TABSTOP
                | WINDOW_STYLE((LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
            LIST_ID,
        )?;
        state.list.set(list);
        let back = if state.kind == ViewKind::Details {
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
            WINDOW_STYLE(0),
            104,
        )?;
        unsafe {
            if !edit.is_invalid() {
                SendMessageW(
                    edit,
                    windows::Win32::UI::Controls::EM_SETLIMITTEXT,
                    Some(WPARAM(super::MAX_QUERY_UNITS)),
                    None,
                );
            }
            for hwnd in [edit, list, back]
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
            font: None,
            background: None,
        })
    }

    pub(super) fn style(
        &mut self,
        parent: HWND,
        state: &ViewState,
        config: &Config,
        dpi: u32,
    ) -> Result<()> {
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
        state.brush.set(HBRUSH(brush.0 .0));
        self.background = Some(brush);
        let font = message_font(dpi)?;
        for hwnd in [
            self.label,
            self.edit,
            self.results_label,
            self.list,
            self.status,
            self.back,
        ]
        .into_iter()
        .filter(|hwnd| !hwnd.is_invalid())
        {
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETFONT,
                    Some(WPARAM(font.0 .0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
        self.font = Some(font);
        self.layout(parent, dpi)
    }

    pub(super) fn layout(&self, parent: HWND, dpi: u32) -> Result<()> {
        let mut rect = RECT::default();
        unsafe { GetClientRect(parent, &mut rect) }.context("search stage=client-bounds")?;
        let px = |dip: i32| (i64::from(dip) * i64::from(dpi) / 96) as i32;
        let (margin, width, height) = (px(16), rect.right, rect.bottom);
        ensure!(
            width >= px(200) && height >= px(180),
            "search stage=layout insufficient-work-area"
        );
        let details = !self.back.is_invalid();
        let list_top = if details { 30 } else { 92 };
        let rows = [
            (self.label, margin, margin, width - 2 * margin, px(22)),
            (
                self.edit,
                margin,
                margin + px(26),
                width - 2 * margin,
                px(32),
            ),
            (
                self.results_label,
                margin,
                margin + px(66),
                width - 2 * margin,
                px(22),
            ),
            (
                self.list,
                margin,
                margin + px(list_top),
                width - 2 * margin,
                height - margin - px(list_top + 52),
            ),
            (
                self.status,
                margin,
                height - px(42),
                width - 2 * margin - if details { px(158) } else { 0 },
                px(32),
            ),
            (
                self.back,
                width - margin - px(148),
                height - px(42),
                px(148),
                px(32),
            ),
        ];
        for (hwnd, x, y, w, h) in rows.into_iter().filter(|(hwnd, ..)| !hwnd.is_invalid()) {
            unsafe { MoveWindow(hwnd, x, y, w, h, true) }.context("search stage=control-layout")?;
        }
        let result = unsafe {
            SendMessageW(
                self.list,
                LB_SETITEMHEIGHT,
                Some(WPARAM(0)),
                Some(LPARAM(px(30) as isize)),
            )
        }
        .0;
        ensure!(result != LB_ERR as isize, "search stage=row-height failed");
        Ok(())
    }
}
