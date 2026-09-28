use crate::{
    text_raster::colorref,
    utils::gdi::{OwnedGdiObject, SavedDc},
};
use anyhow::{ensure, Result};
use windows::Win32::{Foundation::RECT, Graphics::Gdi::*};

pub(super) fn rounded(
    dc: HDC,
    bounds: RECT,
    radius: i32,
    color: u32,
    border: Option<u32>,
) -> Result<()> {
    let saved = SavedDc::new(dc)?;
    saved.select(unsafe { GetStockObject(DC_BRUSH) })?;
    saved.select(unsafe { GetStockObject(if border.is_some() { DC_PEN } else { NULL_PEN }) })?;
    unsafe {
        SetDCBrushColor(dc, colorref(color));
        if let Some(border) = border {
            SetDCPenColor(dc, colorref(border));
        }
        ensure!(
            RoundRect(
                dc,
                bounds.left,
                bounds.top,
                bounds.right,
                bounds.bottom,
                radius * 2,
                radius * 2
            )
            .as_bool(),
            "picker stage=fill failed"
        );
    }
    Ok(())
}

pub(super) fn text(
    dc: HDC,
    font: &OwnedGdiObject,
    value: &str,
    mut bounds: RECT,
    color: u32,
    alignment: DRAW_TEXT_FORMAT,
) -> Result<()> {
    if value.is_empty() || bounds.right <= bounds.left || bounds.bottom <= bounds.top {
        return Ok(());
    }
    let saved = SavedDc::new(dc)?;
    saved.select(font.0)?;
    let mut units: Vec<u16> = value.encode_utf16().collect();
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, colorref(color));
        ensure!(
            DrawTextW(
                dc,
                &mut units,
                &mut bounds,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | alignment
            ) != 0,
            "picker stage=text failed"
        );
    }
    Ok(())
}

pub(super) fn line(dc: HDC, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) -> Result<()> {
    let saved = SavedDc::new(dc)?;
    saved.select(unsafe { GetStockObject(DC_PEN) })?;
    unsafe {
        SetDCPenColor(dc, colorref(color));
        ensure!(
            MoveToEx(dc, x1, y1, None).as_bool() && LineTo(dc, x2, y2).as_bool(),
            "picker stage=line failed"
        );
    }
    Ok(())
}
