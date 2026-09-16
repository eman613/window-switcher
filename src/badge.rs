use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{COLORREF, RECT, SIZE},
    Graphics::Gdi::{
        CreateEllipticRgn, CreateFontW, CreateSolidBrush, DrawTextW, FillRgn,
        GetTextExtentPoint32W, SetBkMode, SetTextColor, ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS,
        CLR_INVALID, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_SINGLELINE, DT_VCENTER,
        FF_DONTCARE, FW_SEMIBOLD, HDC, OUT_DEFAULT_PRECIS, TRANSPARENT,
    },
};

use crate::utils::gdi::{OwnedGdiObject, SavedDc};
use crate::{config::Config, painter::ICON_SIZE_BASE};

mod raster;
pub(crate) use raster::compose;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeStyle {
    pub background: u32,
    pub foreground: u32,
    pub font_size: u32,
}

impl BadgeStyle {
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            background: config.switch_apps_badge_color,
            foreground: config.switch_apps_badge_text_color,
            font_size: config.switch_apps_badge_font_size,
        }
    }
}

pub(crate) fn format_badge_count(window_count: usize, badge_max: u32) -> Option<String> {
    if window_count <= 1 {
        return None;
    }
    let badge_max = badge_max.clamp(2, 9999) as usize;
    Some(if window_count > badge_max {
        format!("{badge_max}+")
    } else {
        window_count.to_string()
    })
}

pub(crate) fn draw_badge(
    hdc: HDC,
    window_count: usize,
    badge_max: u32,
    icon: RECT,
    style: BadgeStyle,
) -> Result<()> {
    let Some(text) = format_badge_count(window_count, badge_max) else {
        return Ok(());
    };
    let icon_size = (icon.right - icon.left).min(icon.bottom - icon.top);
    if icon_size < 4 {
        return Ok(());
    }
    let mut font_height = (i64::from(style.font_size.clamp(8, 24)) * i64::from(icon_size)
        / i64::from(ICON_SIZE_BASE))
    .max(1) as i32;
    let mut text_utf16: Vec<u16> = text.encode_utf16().collect();

    for _ in 0..4 {
        let font = unsafe {
            CreateFontW(
                -font_height,
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
                windows::core::w!("Segoe UI"),
            )
        };
        let font = OwnedGdiObject::new(font.into(), "CreateFontW")?;
        // Restore the font and text/DC state before deleting the font on every path.
        let state = SavedDc::new(hdc)?;
        state.select(font.0)?;
        let mut measured = SIZE::default();
        unsafe { GetTextExtentPoint32W(hdc, &text_utf16, &mut measured) }
            .ok()
            .context("Badge text measurement failed")?;
        let diameter = required_diameter(icon_size, measured);
        let inset = (icon_size / 32).max(1);
        let available = icon_size - inset * 2;
        if diameter > available {
            font_height =
                ((i64::from(font_height) * i64::from(available) / i64::from(diameter)) as i32 - 1)
                    .max(1);
            continue;
        }
        let mut bounds = RECT {
            left: icon.right - inset - diameter,
            top: icon.top + inset,
            right: icon.right - inset,
            bottom: icon.top + inset + diameter,
        };
        let brush = unsafe { CreateSolidBrush(rgb_colorref(style.background)) };
        let brush_guard = OwnedGdiObject::new(brush.into(), "CreateSolidBrush")?;
        let region =
            unsafe { CreateEllipticRgn(bounds.left, bounds.top, bounds.right, bounds.bottom) };
        let region_guard = OwnedGdiObject::new(region.into(), "CreateEllipticRgn")?;
        unsafe {
            FillRgn(hdc, region, brush)
                .ok()
                .context("Badge circle fill failed")?;
            if SetBkMode(hdc, TRANSPARENT) == 0
                || SetTextColor(hdc, rgb_colorref(style.foreground)).0 == CLR_INVALID
            {
                bail!("Badge text color setup failed");
            }
            if DrawTextW(
                hdc,
                &mut text_utf16,
                &mut bounds,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            ) == 0
            {
                bail!("Badge text drawing failed");
            }
        }
        drop((region_guard, brush_guard));
        return Ok(());
    }
    bail!("Badge text does not fit the icon; skipped count {text}")
}

fn required_diameter(icon_size: i32, measured: SIZE) -> i32 {
    let padding = (icon_size / ICON_SIZE_BASE).max(1);
    let width = f64::from(measured.cx) + f64::from(padding) * 2.0;
    let height = f64::from(measured.cy) + f64::from(padding) * 2.0;
    // The text rectangle's diagonal must fit inside the circle, including its corners.
    (width.hypot(height).ceil() as i32).max((i64::from(icon_size) * 3 / 8) as i32)
}

fn rgb_colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff))
}

#[cfg(test)]
#[path = "badge_tests.rs"]
mod tests;
