use super::messages::{self, ViewState, EDIT_ID, LIST_ID};
use crate::{
    appearance::Appearance, config::Config, localization::Text, utils::gdi::OwnedGdiObject,
};
use anyhow::{ensure, Context, Result};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::Gdi::{CreateFontIndirectW, CreateSolidBrush, HBRUSH, HGDIOBJ},
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::SystemParametersInfoForDpi, Shell::SetWindowSubclass, WindowsAndMessaging::*},
    },
};

pub(super) struct Controls {
    pub edit: HWND,
    pub list: HWND,
    pub status: HWND,
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
            text.search_label(),
            WINDOW_STYLE(0),
            100,
        )?;
        let edit = child(
            parent,
            w!("EDIT"),
            "",
            WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            EDIT_ID,
        )?;
        state.edit.set(edit);
        let results_label = child(
            parent,
            w!("STATIC"),
            text.search_results_label(),
            WINDOW_STYLE(0),
            103,
        )?;
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
        let status = child(
            parent,
            w!("STATIC"),
            text.search_loading(),
            WINDOW_STYLE(0),
            104,
        )?;
        unsafe {
            SendMessageW(
                edit,
                windows::Win32::UI::Controls::EM_SETLIMITTEXT,
                Some(WPARAM(super::MAX_QUERY_UNITS)),
                None,
            );
            for hwnd in [edit, list] {
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
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
                0,
                dpi,
            )
        }
        .context("search stage=system-font")?;
        metrics.lfMessageFont.lfHeight = -metrics
            .lfMessageFont
            .lfHeight
            .abs()
            .max((14 * dpi / 96) as i32);
        let font = OwnedGdiObject::new(
            HGDIOBJ(unsafe { CreateFontIndirectW(&metrics.lfMessageFont) }.0),
            "search-font",
        )?;
        for hwnd in [
            self.label,
            self.edit,
            self.results_label,
            self.list,
            self.status,
        ] {
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
                margin + px(92),
                width - 2 * margin,
                height - margin - px(144),
            ),
            (
                self.status,
                margin,
                height - px(42),
                width - 2 * margin,
                px(32),
            ),
        ];
        for (hwnd, x, y, w, h) in rows {
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
