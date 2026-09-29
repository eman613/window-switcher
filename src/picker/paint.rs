use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::*,
    UI::{
        Controls::{
            DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, ODT_BUTTON, ODT_LISTBOX,
        },
        WindowsAndMessaging::{GetClientRect, GetWindowTextLengthW},
    },
};

use super::{
    drawing::{line, rounded, text},
    layout::PickerLayout,
    messages::{ViewState, CLEAR_ID, DISMISS_ID, LIST_ID},
    rows::PickerRow,
    skin::{px, SearchSkin},
    ViewKind,
};
use crate::{render_surface::RenderSurface, text_raster::colorref, utils::gdi::SavedDc};

fn draw_icon(
    dc: HDC,
    row: &PickerRow,
    x: i32,
    y: i32,
    size: i32,
    background: u32,
    foreground: u32,
) -> Result<()> {
    if let Some(icon) = row.image() {
        let scaled = icon.image.resize(size, size)?;
        let mut surface = RenderSurface::new(size, size)?;
        surface.fill_rgb(background)?;
        surface.compose(&scaled, 0, 0)?;
        unsafe { BitBlt(dc, x, y, size, size, Some(surface.dc()), 0, 0, SRCCOPY) }
            .context("picker stage=icon-copy")?;
    } else {
        let inset = size / 6;
        rounded(
            dc,
            RECT {
                left: x + inset,
                top: y + inset,
                right: x + size - inset,
                bottom: y + size - inset,
            },
            (size / 10).max(1),
            background,
            Some(foreground),
        )?;
        line(
            dc,
            x + inset,
            y + inset * 2,
            x + size - inset,
            y + inset * 2,
            foreground,
        )?;
    }
    Ok(())
}

struct RowState {
    selected: bool,
    focused: bool,
    hovered: bool,
    pressed: bool,
    disabled: bool,
}

fn draw_row(
    dc: HDC,
    bounds: RECT,
    row: &PickerRow,
    skin: &SearchSkin,
    state: RowState,
) -> Result<()> {
    let p = |value| px(value, skin.dpi);
    let palette = skin.palette;
    let background = if state.pressed {
        palette.pressed
    } else if state.selected {
        palette.selected
    } else if state.hovered && !state.disabled {
        palette.hover
    } else {
        palette.surface
    };
    let selected = state.selected || (state.pressed && palette.high_contrast);
    let foreground = if selected {
        palette.selected_text
    } else if state.disabled {
        palette.muted
    } else {
        palette.text
    };
    let secondary = if selected && palette.high_contrast {
        palette.selected_text
    } else {
        palette.muted
    };
    rounded(
        dc,
        bounds,
        if palette.high_contrast { 0 } else { p(6) },
        background,
        None,
    )?;
    let height = bounds.bottom - bounds.top;
    if state.selected {
        rounded(
            dc,
            RECT {
                left: bounds.left,
                right: bounds.left + p(3).max(2),
                top: bounds.top + (height - p(28)) / 2,
                bottom: bounds.top + (height + p(28)) / 2,
            },
            p(1),
            if palette.high_contrast {
                palette.selected_text
            } else {
                palette.accent
            },
            None,
        )?;
    }
    let icon_size = p(24).max(1);
    let icon_left = bounds.left + p(12);
    draw_icon(
        dc,
        row,
        icon_left,
        bounds.top + (height - icon_size) / 2,
        icon_size,
        background,
        secondary,
    )?;
    let left = icon_left + icon_size + p(12);
    let right = bounds.right - p(12);
    let app_width = ((right - left) / 3).min(p(190));
    text(
        dc,
        &skin.title,
        &row.primary,
        RECT {
            left,
            right: right - app_width - p(16),
            ..bounds
        },
        foreground,
        DT_LEFT,
    )?;
    let app = if row.meta.is_empty() {
        row.secondary.clone()
    } else {
        format!("{} · {}", row.secondary, row.meta)
    };
    text(
        dc,
        &skin.normal,
        &app,
        RECT {
            left: right - app_width,
            right,
            ..bounds
        },
        secondary,
        DT_RIGHT,
    )?;
    if state.focused {
        let rect = RECT {
            left: bounds.left + p(4),
            top: bounds.top + p(3),
            right: bounds.right - p(4),
            bottom: bounds.bottom - p(3),
        };
        ensure!(
            unsafe { DrawFocusRect(dc, &rect) }.as_bool(),
            "picker stage=focus failed"
        );
    }
    Ok(())
}

pub(super) fn item(state: &ViewState, item: &DRAWITEMSTRUCT) -> Result<bool> {
    if state.kind != ViewKind::Search {
        return Ok(false);
    }
    let visual = state
        .visual
        .try_borrow()
        .context("picker stage=paint reentrant state")?;
    let Some(skin) = &visual.skin else {
        return Ok(false);
    };
    if item.CtlType == ODT_LISTBOX && item.CtlID as usize == LIST_ID {
        if let Some(row) = visual.rows.get(item.itemID as usize) {
            draw_row(
                item.hDC,
                item.rcItem,
                row,
                skin,
                RowState {
                    selected: item.itemState.0 & ODS_SELECTED.0 != 0,
                    focused: item.itemState.0 & ODS_FOCUS.0 != 0,
                    hovered: state.hover.get() == Some(item.itemID as usize),
                    pressed: state.pressed.get() == Some((state.epoch.get(), item.itemID as usize)),
                    disabled: state.busy.get() || item.itemState.0 & ODS_DISABLED.0 != 0,
                },
            )?;
        } else {
            rounded(item.hDC, item.rcItem, 0, skin.palette.surface, None)?;
        }
        return Ok(true);
    }
    if item.CtlType == ODT_BUTTON && matches!(item.CtlID as usize, CLEAR_ID | DISMISS_ID) {
        let p = |value| px(value, skin.dpi);
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let hovered = state.hot_control.get() == item.hwndItem && !disabled;
        let background = if pressed {
            skin.palette.pressed
        } else if hovered {
            skin.palette.hover
        } else {
            skin.palette.surface
        };
        let foreground = if pressed && skin.palette.high_contrast {
            skin.palette.selected_text
        } else {
            skin.palette.muted
        };
        // Owner-drawn buttons must also erase the pixels outside their rounded face.
        ensure!(
            unsafe { FillRect(item.hDC, &item.rcItem, skin.background_brush()) } != 0,
            "picker stage=button-background failed"
        );
        rounded(
            item.hDC,
            item.rcItem,
            p(5),
            background,
            (item.CtlID as usize == DISMISS_ID).then_some(skin.palette.border),
        )?;
        if item.CtlID as usize == CLEAR_ID {
            let (x, y) = (
                (item.rcItem.left + item.rcItem.right) / 2,
                (item.rcItem.top + item.rcItem.bottom) / 2,
            );
            let color = if disabled {
                skin.palette.border
            } else {
                foreground
            };
            line(item.hDC, x - p(4), y - p(4), x + p(4), y + p(4), color)?;
            line(item.hDC, x + p(4), y - p(4), x - p(4), y + p(4), color)?;
        } else {
            text(
                item.hDC,
                &skin.normal,
                "Esc",
                item.rcItem,
                foreground,
                DT_CENTER,
            )?;
        }
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            ensure!(
                unsafe { DrawFocusRect(item.hDC, &item.rcItem) }.as_bool(),
                "picker stage=button-focus failed"
            );
        }
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn window(hwnd: HWND, state: &ViewState) -> Result<()> {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let result = (|| -> Result<()> {
        ensure!(!dc.is_invalid(), "picker stage=paint invalid dc");
        let visual = state
            .visual
            .try_borrow()
            .context("picker stage=chrome reentrant state")?;
        let Some(skin) = &visual.skin else {
            return Ok(());
        };
        let mut bounds = RECT::default();
        unsafe { GetClientRect(hwnd, &mut bounds) }?;
        let layout = PickerLayout::calculate(
            bounds.right,
            bounds.bottom,
            skin.dpi,
            ViewKind::Search,
            skin.row_height,
            skin.secondary_height,
        )?;
        let p = |value| px(value, skin.dpi);
        rounded(
            dc,
            bounds,
            if skin.palette.high_contrast { 0 } else { p(12) },
            skin.palette.surface,
            Some(skin.palette.border),
        )?;
        rounded(
            dc,
            RECT {
                left: p(12),
                top: p(10),
                right: layout.clear.right + p(6),
                bottom: layout.query_bottom - p(10),
            },
            p(8),
            skin.palette.surface,
            Some(skin.palette.border),
        )?;
        line(
            dc,
            1,
            layout.footer_top,
            bounds.right - 1,
            layout.footer_top,
            skin.palette.divider,
        )?;
        let (x, y) = (p(28), (layout.edit.top + layout.edit.bottom) / 2);
        {
            let saved = SavedDc::new(dc)?;
            saved.select(unsafe { GetStockObject(DC_PEN) })?;
            saved.select(unsafe { GetStockObject(NULL_BRUSH) })?;
            unsafe {
                SetDCPenColor(dc, colorref(skin.palette.accent));
                ensure!(
                    Ellipse(dc, x - p(7), y - p(7), x + p(7), y + p(7)).as_bool(),
                    "picker stage=search-icon failed"
                );
            }
        }
        line(
            dc,
            x + p(5),
            y + p(5),
            x + p(11),
            y + p(11),
            skin.palette.accent,
        )?;
        let shortcut_width = if bounds.right >= p(650) && skin.secondary_height <= p(18) {
            p(170)
        } else {
            0
        };
        let footer = RECT {
            left: p(20),
            top: layout.footer_top,
            right: bounds.right - p(20) - shortcut_width,
            bottom: bounds.bottom,
        };
        text(
            dc,
            &skin.normal,
            state.text.search_keyboard_help(),
            footer,
            skin.palette.muted,
            DT_LEFT,
        )?;
        if shortcut_width > 0 {
            text(
                dc,
                &skin.normal,
                &skin.shortcut,
                RECT {
                    left: footer.right,
                    right: bounds.right - p(20),
                    ..footer
                },
                skin.palette.muted,
                DT_RIGHT,
            )?;
        }
        super::scrollbar::paint(hwnd, state, dc)?;
        Ok(())
    })();
    let ended = unsafe { EndPaint(hwnd, &paint) }.as_bool();
    result?;
    ensure!(ended, "picker stage=end-paint failed");
    Ok(())
}

pub(super) fn edit_hint(hwnd: HWND, state: &ViewState) -> Result<()> {
    if state.composing.get() || unsafe { GetWindowTextLengthW(hwnd) } != 0 {
        return Ok(());
    }
    let visual = state
        .visual
        .try_borrow()
        .context("picker stage=hint reentrant state")?;
    let Some(skin) = &visual.skin else {
        return Ok(());
    };
    let mut bounds = RECT::default();
    unsafe { GetClientRect(hwnd, &mut bounds) }.context("picker stage=hint-bounds")?;
    let dc = unsafe { GetDC(Some(hwnd)) };
    ensure!(!dc.is_invalid(), "picker stage=hint invalid dc");
    let result = text(
        dc,
        &skin.input,
        state.text.search_input_hint(),
        bounds,
        skin.palette.muted,
        DT_LEFT,
    );
    let released = unsafe { ReleaseDC(Some(hwnd), dc) };
    result?;
    ensure!(released != 0, "picker stage=hint release dc failed");
    Ok(())
}
