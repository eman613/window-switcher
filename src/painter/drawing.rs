use super::SCALE_FACTOR;
use crate::{
    app::SwitchAppsState,
    badge::draw_badge,
    utils::{
        gdi::{BitmapSurface, OwnedGdiObject},
        gdiplus::{check_status, OwnedGp},
    },
};
use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{COLORREF, RECT},
    Graphics::{
        Gdi::{
            CreateRoundRectRgn, CreateSolidBrush, FillRect, FillRgn, SetStretchBltMode, StretchBlt,
            HALFTONE, HDC, SRCCOPY,
        },
        GdiPlus::{
            FillModeAlternate, GdipAddPathArc, GdipClosePathFigure, GdipCreatePath, GdipDeletePath,
            GdipFillPath, GpBrush, GpGraphics,
        },
    },
    UI::WindowsAndMessaging::{DrawIconEx, DI_NORMAL},
};

pub(super) unsafe fn draw_round_rect(
    graphic_ptr: *mut GpGraphics,
    brush_ptr: *mut GpBrush,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    corner_radius: f32,
) -> Result<()> {
    unsafe {
        let path = OwnedGp::create(
            "create-path",
            |out| GdipCreatePath(FillModeAlternate, out),
            GdipDeletePath,
        )?;
        check_status(
            GdipAddPathArc(
                path.ptr,
                left,
                top,
                corner_radius,
                corner_radius,
                180.0,
                90.0,
            ),
            "path-arc",
        )?;
        check_status(
            GdipAddPathArc(
                path.ptr,
                right - corner_radius,
                top,
                corner_radius,
                corner_radius,
                270.0,
                90.0,
            ),
            "path-arc",
        )?;
        check_status(
            GdipAddPathArc(
                path.ptr,
                right - corner_radius,
                bottom - corner_radius,
                corner_radius,
                corner_radius,
                0.0,
                90.0,
            ),
            "path-arc",
        )?;
        check_status(
            GdipAddPathArc(
                path.ptr,
                left,
                bottom - corner_radius,
                corner_radius,
                corner_radius,
                90.0,
                90.0,
            ),
            "path-arc",
        )?;
        check_status(GdipClosePathFigure(path.ptr), "close-path")?;
        check_status(GdipFillPath(graphic_ptr, brush_ptr, path.ptr), "fill-path")?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_icons(
    state: &SwitchAppsState,
    hdc_screen: HDC,
    icon_size: i32,
    icon_border: i32,
    width: i32,
    height: i32,
    corner_radius: i32,
    fg_color: u32,
    bg_color: u32,
) -> Result<OwnedGdiObject> {
    let scaled_width = width
        .checked_mul(SCALE_FACTOR)
        .context("paint width overflow")?;
    let scaled_height = height
        .checked_mul(SCALE_FACTOR)
        .context("paint height overflow")?;
    let scaled_corner_radius = corner_radius * SCALE_FACTOR;
    let scaled_border_size = icon_border * SCALE_FACTOR;
    let scaled_icon_inner_size = icon_size * SCALE_FACTOR;
    let scaled_icon_outer_size = scaled_icon_inner_size + scaled_border_size * 2;

    unsafe {
        let surface = BitmapSurface::new(hdc_screen, width, height)?;
        let scaled = BitmapSurface::new(hdc_screen, scaled_width, scaled_height)?;
        let hdc_tmp = surface.dc();
        let hdc_scaled = scaled.dc();

        let fg_brush = CreateSolidBrush(COLORREF(fg_color));
        let _foreground = OwnedGdiObject::new(fg_brush.into(), "foreground-brush")?;
        let bg_brush = CreateSolidBrush(COLORREF(bg_color));
        let _background = OwnedGdiObject::new(bg_brush.into(), "background-brush")?;

        let rect = RECT {
            left: 0,
            top: 0,
            right: scaled_width,
            bottom: scaled_height,
        };

        if FillRect(hdc_scaled, &rect, bg_brush) == 0 {
            bail!("paint stage=fill-rect failed");
        }

        for (i, entry) in state.apps.iter().enumerate() {
            // draw the box for selected icon
            if i == state.index {
                let left = scaled_icon_outer_size * (i as i32);
                let top = 0;
                let right = left + scaled_icon_outer_size;
                let bottom = top + scaled_icon_outer_size;
                let rgn = CreateRoundRectRgn(
                    left,
                    top,
                    right,
                    bottom,
                    scaled_corner_radius,
                    scaled_corner_radius,
                );
                let _region = OwnedGdiObject::new(rgn.into(), "selected-region")?;
                FillRgn(hdc_scaled, rgn, fg_brush)
                    .ok()
                    .context("paint stage=fill-region")?;
            }

            let cx = scaled_border_size + scaled_icon_outer_size * (i as i32);
            DrawIconEx(
                hdc_scaled,
                cx,
                scaled_border_size,
                entry.icon,
                scaled_icon_inner_size,
                scaled_icon_inner_size,
                0,
                None,
                DI_NORMAL,
            )
            .context("paint stage=draw-icon")?;

            if state.show_badge {
                if let Err(err) = draw_badge(
                    hdc_scaled,
                    entry.window_count,
                    state.badge_max,
                    RECT {
                        left: cx,
                        top: scaled_border_size,
                        right: cx + scaled_icon_inner_size,
                        bottom: scaled_border_size + scaled_icon_inner_size,
                    },
                    state.badge_style,
                ) {
                    warn!(
                        "Badge render failed count={} error={err:#}",
                        entry.window_count
                    );
                }
            }
        }

        if SetStretchBltMode(hdc_tmp, HALFTONE) == 0 {
            bail!("paint stage=stretch-mode failed");
        }
        StretchBlt(
            hdc_tmp,
            0,
            0,
            width,
            height,
            Some(hdc_scaled),
            0,
            0,
            scaled_width,
            scaled_height,
            SRCCOPY,
        )
        .ok()
        .context("paint stage=stretch-blit")?;
        Ok(surface.into_bitmap())
    }
}
