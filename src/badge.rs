use windows::core::w;
use windows::Win32::{
    Foundation::{COLORREF, RECT},
    Graphics::Gdi::{
        CreateFontW, CreateRoundRectRgn, DeleteObject, DrawTextW, FillRgn, SelectObject, SetBkMode,
        SetTextColor, ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
        DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_BOLD, HBRUSH, HDC,
        HFONT, HGDIOBJ, HRGN, OUT_DEFAULT_PRECIS, TRANSPARENT,
    },
};

const BADGE_HEIGHT_NUMERATOR: i32 = 3;
const BADGE_HEIGHT_DENOMINATOR: i32 = 8;
const BADGE_EXTRA_WIDTH: i32 = 8;
const BADGE_OFFSET: i32 = 3;
const BADGE_BORDER_SIZE: i32 = 1;
pub(crate) const BADGE_BACKGROUND_COLOR: u32 = colorref(107, 79, 52);
pub(crate) const BADGE_BORDER_COLOR: u32 = colorref(245, 232, 215);
const BADGE_TEXT_COLOR: u32 = colorref(255, 248, 240);

const fn colorref(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BadgeGeometry {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) offset: i32,
}

pub(crate) fn badge_label(window_count: usize, max_count: usize) -> Option<String> {
    if window_count <= 1 {
        return None;
    }

    if window_count > max_count {
        Some(format!("{max_count}+"))
    } else {
        Some(window_count.to_string())
    }
}

pub(crate) fn badge_geometry(label: &str, icon_size: i32, item_size: i32) -> Option<BadgeGeometry> {
    if label.is_empty() || icon_size <= 0 || item_size <= 0 {
        return None;
    }

    let available_size = item_size.saturating_sub(BADGE_OFFSET.saturating_mul(2));
    if available_size <= 0 {
        return None;
    }

    let height = icon_size
        .saturating_mul(BADGE_HEIGHT_NUMERATOR)
        .checked_div(BADGE_HEIGHT_DENOMINATOR)
        .unwrap_or(0)
        .max(1)
        .min(available_size);
    let extra_width =
        BADGE_EXTRA_WIDTH.saturating_mul(label.chars().count().saturating_sub(1) as i32);
    let width = height.saturating_add(extra_width).min(available_size);

    (width > 0 && height > 0).then_some(BadgeGeometry {
        width,
        height,
        offset: BADGE_OFFSET,
    })
}

pub(crate) fn create_badge_font(badge_height: i32, scale_factor: i32) -> Option<HFONT> {
    if badge_height <= 0 || scale_factor <= 0 {
        return None;
    }

    let font_height = badge_height
        .saturating_mul(scale_factor)
        .saturating_mul(2)
        .checked_div(3)
        .unwrap_or(0)
        .max(1);
    let font = unsafe {
        CreateFontW(
            -font_height,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            w!("Segoe UI"),
        )
    };

    (!font.is_invalid()).then_some(font)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_badge(
    hdc: HDC,
    label: &str,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    scale_factor: i32,
    border_brush: HBRUSH,
    background_brush: HBRUSH,
    font: Option<HFONT>,
) {
    if width <= 0 || height <= 0 || scale_factor <= 0 {
        return;
    }

    let right = left.saturating_add(width);
    let bottom = top.saturating_add(height);
    if right <= left || bottom <= top {
        return;
    }

    unsafe {
        let Some(outer_region) =
            RegionGuard::new(CreateRoundRectRgn(left, top, right, bottom, height, height))
        else {
            return;
        };
        if !border_brush.is_invalid() {
            let _ = FillRgn(hdc, outer_region.get(), border_brush);
        }

        let inset = (BADGE_BORDER_SIZE * scale_factor)
            .min(width.saturating_div(4))
            .min(height.saturating_div(4));
        let inner_width = width.saturating_sub(inset.saturating_mul(2));
        let inner_height = height.saturating_sub(inset.saturating_mul(2));
        if inner_width <= 0 || inner_height <= 0 {
            return;
        }

        let inner_left = left + inset;
        let inner_top = top + inset;
        let inner_right = inner_left + inner_width;
        let inner_bottom = inner_top + inner_height;
        let inner_region = RegionGuard::new(CreateRoundRectRgn(
            inner_left,
            inner_top,
            inner_right,
            inner_bottom,
            inner_height,
            inner_height,
        ));
        if let Some(inner_region) = inner_region {
            if !background_brush.is_invalid() {
                let _ = FillRgn(hdc, inner_region.get(), background_brush);
            }
        }

        let Some(font) = font else {
            return;
        };

        let mut text = label.encode_utf16().collect::<Vec<_>>();
        let mut text_rect = RECT {
            left: inner_left,
            top: inner_top,
            right: inner_right,
            bottom: inner_bottom,
        };
        let Some(_font_selection) = SelectedObjectGuard::new(hdc, font.into()) else {
            return;
        };
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(BADGE_TEXT_COLOR));
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut text_rect,
            DT_CENTER | DT_NOPREFIX | DT_SINGLELINE | DT_VCENTER,
        );
    }
}

struct RegionGuard(HRGN);

impl RegionGuard {
    fn new(handle: HRGN) -> Option<Self> {
        (!handle.is_invalid()).then_some(Self(handle))
    }

    fn get(&self) -> HRGN {
        self.0
    }
}

impl Drop for RegionGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

struct SelectedObjectGuard {
    hdc: HDC,
    previous: HGDIOBJ,
}

impl SelectedObjectGuard {
    fn new(hdc: HDC, object: HGDIOBJ) -> Option<Self> {
        let previous = unsafe { SelectObject(hdc, object) };
        (!previous.is_invalid()).then_some(Self { hdc, previous })
    }
}

impl Drop for SelectedObjectGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.hdc, self.previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_label_only_appears_for_multiple_windows() {
        assert_eq!(badge_label(0, 99), None);
        assert_eq!(badge_label(1, 99), None);
        assert_eq!(badge_label(2, 99).as_deref(), Some("2"));
        assert_eq!(badge_label(99, 99).as_deref(), Some("99"));
    }

    #[test]
    fn badge_label_caps_large_window_counts() {
        assert_eq!(badge_label(100, 99).as_deref(), Some("99+"));
        assert_eq!(badge_label(usize::MAX, 999).as_deref(), Some("999+"));
    }

    #[test]
    fn badge_geometry_scales_with_icon_and_clamps_to_item() {
        assert_eq!(
            badge_geometry("2", 64, 72),
            Some(BadgeGeometry {
                width: 24,
                height: 24,
                offset: 3,
            })
        );
        assert_eq!(
            badge_geometry("99", 64, 72),
            Some(BadgeGeometry {
                width: 32,
                height: 24,
                offset: 3,
            })
        );
        assert_eq!(
            badge_geometry("99+", 16, 20),
            Some(BadgeGeometry {
                width: 14,
                height: 6,
                offset: 3,
            })
        );
        assert_eq!(badge_geometry("", 64, 72), None);
        assert_eq!(badge_geometry("2", 0, 72), None);
    }
}
