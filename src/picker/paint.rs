use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::*,
    UI::{
        Controls::{
            DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, ODT_BUTTON, ODT_LISTBOX,
        },
        WindowsAndMessaging::GetClientRect,
    },
};

use super::{
    drawing::{line, rounded, text},
    layout::PickerLayout,
    messages::{ViewState, DISMISS_ID, LIST_ID},
    rows::PickerRow,
    skin::{px, SearchSkin},
    ViewKind,
};
use crate::render_surface::RenderSurface;

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
    query: &str,
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
    let foreground = if state.pressed {
        palette.pressed_text
    } else if selected {
        palette.selected_text
    } else if state.disabled {
        palette.muted
    } else {
        palette.text
    };
    let secondary = if state.pressed {
        palette.pressed_muted
    } else if selected {
        palette.selected_muted
    } else {
        palette.muted
    };
    // RoundRect with NULL_PEN does not cover every pixel of a previous border.
    // Restore the entire row, including the area outside its rounded corners.
    ensure!(
        unsafe { FillRect(dc, &bounds, skin.background_brush()) } != 0,
        "picker stage=row-background failed"
    );
    rounded(
        dc,
        bounds,
        skin.selection_radius.min((bounds.bottom - bounds.top) / 2),
        background,
        state.selected.then_some(palette.divider),
    )?;
    let height = bounds.bottom - bounds.top;
    let icon_size = p(19).max(1);
    let icon_left = bounds.left + p(3);
    draw_icon(
        dc,
        row,
        icon_left,
        bounds.top + (height - icon_size) / 2,
        icon_size,
        background,
        secondary,
    )?;
    let left = bounds.left + p(25);
    // Keep text clear of the overlay's hit area without shortening the row fill.
    let right = bounds.right - p(17);
    let app_width = ((right - left) / 3).min(p(106));
    super::highlight::draw(
        dc,
        &row.primary,
        query,
        RECT {
            left,
            right: right - app_width - p(3),
            ..bounds
        },
        foreground,
        DT_LEFT,
        skin,
    )?;
    super::highlight::draw(
        dc,
        &row.secondary,
        query,
        RECT {
            left: right - app_width,
            right,
            ..bounds
        },
        secondary,
        DT_RIGHT,
        skin,
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
        return super::repaint::buffered(state, item.hDC, item.rcItem, |dc| {
            if let Some(row) = visual.rows.get(item.itemID as usize) {
                draw_row(
                    dc,
                    item.rcItem,
                    row,
                    skin,
                    RowState {
                        selected: item.itemState.0 & ODS_SELECTED.0 != 0,
                        focused: item.itemState.0 & ODS_FOCUS.0 != 0,
                        hovered: state.hover.get() == Some(item.itemID as usize),
                        pressed: state.pressed.get() == Some(row.key.identity),
                        disabled: state.busy.get() || item.itemState.0 & ODS_DISABLED.0 != 0,
                    },
                    &visual.query,
                )?;
            } else {
                rounded(dc, item.rcItem, 0, skin.palette.surface, None)?;
            }
            super::scrollbar::paint_row(state, dc, item.rcItem)?;
            Ok(())
        })
        .map(|()| true);
    }
    if item.CtlType == ODT_BUTTON && item.CtlID as usize == DISMISS_ID {
        ensure!(
            unsafe { FillRect(item.hDC, &item.rcItem, skin.background_brush()) } != 0,
            "picker stage=help-background failed"
        );
        let active =
            state.hot_control.get() == item.hwndItem || item.itemState.0 & ODS_SELECTED.0 != 0;
        // Switcheroo's help glyph uses DarkGray at 40% opacity until hovered.
        let alpha: u32 = if active { 255 } else { 102 };
        let channel = |shift: u32| -> u32 {
            ((((skin.palette.border >> shift) & 255u32) * alpha
                + ((skin.palette.surface >> shift) & 255u32) * (255 - alpha)
                + 127)
                / 255)
                << shift
        };
        let help_color = if skin.palette.high_contrast {
            skin.palette.text
        } else {
            channel(16) | channel(8) | channel(0)
        };
        text(
            item.hDC,
            &skin.question_font,
            "?",
            item.rcItem,
            help_color,
            DT_CENTER,
        )?;
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            ensure!(
                unsafe { DrawFocusRect(item.hDC, &item.rcItem) }.as_bool(),
                "picker stage=help-focus failed"
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
            state.help_open.get(),
        )?;
        let p = |value| px(value, skin.dpi);
        rounded(
            dc,
            bounds,
            skin.panel_radius,
            skin.palette.surface,
            Some(skin.palette.border),
        )?;
        rounded(
            dc,
            RECT {
                left: p(4),
                top: layout.edit.top - p(6),
                right: bounds.right - p(4),
                bottom: layout.query_bottom,
            },
            skin.panel_radius.min(p(8)),
            skin.palette.surface,
            Some(skin.palette.accent),
        )?;
        Ok(())
    })();
    let ended = unsafe { EndPaint(hwnd, &paint) }.as_bool();
    result?;
    ensure!(ended, "picker stage=end-paint failed");
    Ok(())
}
