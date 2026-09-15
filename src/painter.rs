use crate::app::SwitchAppsState;
use crate::utils::{
    gdi::{as_bitmap, BitmapSurface, WindowDc},
    gdiplus::{check_status, GdiPlusRuntime, OwnedGp},
    get_moinitor_rect, is_light_theme, is_win11,
};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{COLORREF, HWND, POINT, SIZE},
    Graphics::{
        Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, HPALETTE},
        GdiPlus::{
            FlushIntentionSync, GdipCreateBitmapFromHBITMAP, GdipCreateFromHDC,
            GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDisposeImage,
            GdipDrawImageRect, GdipFillRectangle, GdipFlush, GdipSetInterpolationMode,
            GdipSetSmoothingMode, InterpolationModeHighQualityBicubic, SmoothingModeAntiAlias,
        },
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{
            GetCursorPos, ShowWindow, UpdateLayeredWindow, SW_HIDE, SW_SHOW, ULW_ALPHA,
        },
    },
};

mod drawing;
use drawing::{draw_icons, draw_round_rect};

#[cfg(test)]
#[path = "painter_tests.rs"]
mod tests;

pub const BG_DARK_COLOR: u32 = 0x4c4c4c;
pub const FG_DARK_COLOR: u32 = 0x3b3b3b;
pub const BG_LIGHT_COLOR: u32 = 0xe0e0e0;
pub const FG_LIGHT_COLOR: u32 = 0xf2f2f2;
pub const ALPHA_MASK: u32 = 0xff000000;
pub const ICON_SIZE_BASE: i32 = 64;
pub const WINDOW_BORDER_SIZE_BASE: i32 = 10;
pub const ICON_BORDER_SIZE_BASE: i32 = 4;
pub const SCALE_FACTOR: i32 = 6;

// GDI Antialiasing Painter
pub struct GdiAAPainter {
    hwnd: HWND,
    screen: WindowDc,
    _runtime: GdiPlusRuntime,
    rounded_corner: bool,
    show: bool,
}

impl GdiAAPainter {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let runtime = GdiPlusRuntime::new()?;
        let screen = WindowDc::new(Some(hwnd))?;
        let rounded_corner = is_win11();

        Ok(Self {
            hwnd,
            screen,
            _runtime: runtime,
            rounded_corner,
            show: false,
        })
    }

    pub fn paint(&mut self, state: &SwitchAppsState, allowed: impl Fn() -> bool) -> Result<()> {
        if !allowed() {
            return Ok(());
        }
        self.render(state)?;
        if !self.show && allowed() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                if allowed() {
                    let _ = SetFocus(Some(self.hwnd));
                } else {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                    return Ok(());
                }
            }
            self.show = true;
        }
        Ok(())
    }

    fn render(&self, state: &SwitchAppsState) -> Result<()> {
        let dpi_scale = get_dpi_scale(self.hwnd);
        let icon_size_max = (ICON_SIZE_BASE as f64 * dpi_scale) as i32;
        let border_size = (WINDOW_BORDER_SIZE_BASE as f64 * dpi_scale) as i32;
        let icon_border = (ICON_BORDER_SIZE_BASE as f64 * dpi_scale) as i32;

        let Coordinate {
            x,
            y,
            width,
            height,
            icon_size,
            item_size,
        } = Coordinate::new(
            state
                .apps
                .len()
                .try_into()
                .context("paint stage=item-count overflow")?,
            icon_size_max,
            border_size,
            icon_border,
        )?;

        let corner_radius = if self.rounded_corner {
            item_size / 4
        } else {
            0
        };

        let hwnd = self.hwnd;
        let hdc_screen = self.screen.dc;

        let (fg_color, bg_color) = theme_color(is_light_theme());

        unsafe {
            let surface = BitmapSurface::new(hdc_screen, width, height)?;
            let graphics = OwnedGp::create(
                "create-graphics",
                |out| GdipCreateFromHDC(surface.dc(), out),
                GdipDeleteGraphics,
            )?;
            check_status(
                GdipSetSmoothingMode(graphics.ptr, SmoothingModeAntiAlias),
                "smoothing",
            )?;
            check_status(
                GdipSetInterpolationMode(graphics.ptr, InterpolationModeHighQualityBicubic),
                "interpolation",
            )?;
            let bg_brush = OwnedGp::create(
                "create-brush",
                |out| GdipCreateSolidFill(ALPHA_MASK | bg_color, out),
                |brush| GdipDeleteBrush(brush.cast()),
            )?;

            if self.rounded_corner {
                draw_round_rect(
                    graphics.ptr,
                    bg_brush.ptr.cast(),
                    0.0,
                    0.0,
                    width as f32,
                    height as f32,
                    corner_radius as f32,
                )?;
            } else {
                check_status(
                    GdipFillRectangle(
                        graphics.ptr,
                        bg_brush.ptr.cast(),
                        0.0,
                        0.0,
                        width as f32,
                        height as f32,
                    ),
                    "fill-background",
                )?;
            }

            let icons_width = item_size * state.apps.len() as i32;
            let icons_height = item_size;
            let bitmap_icons = draw_icons(
                state,
                hdc_screen,
                icon_size,
                icon_border,
                icons_width,
                icons_height,
                corner_radius,
                fg_color,
                bg_color,
            )?;

            let image = OwnedGp::create(
                "create-image",
                |out| {
                    GdipCreateBitmapFromHBITMAP(as_bitmap(&bitmap_icons), HPALETTE::default(), out)
                },
                |bitmap| GdipDisposeImage(bitmap.cast()),
            )?;
            check_status(
                GdipDrawImageRect(
                    graphics.ptr,
                    image.ptr.cast(),
                    border_size as f32,
                    border_size as f32,
                    icons_width as f32,
                    icons_height as f32,
                ),
                "draw-icons",
            )?;
            check_status(GdipFlush(graphics.ptr, FlushIntentionSync), "flush")?;

            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as _,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as _,
                ..Default::default()
            };
            UpdateLayeredWindow(
                hwnd,
                Some(hdc_screen),
                Some(&POINT { x, y }),
                Some(&SIZE {
                    cx: width,
                    cy: height,
                }),
                Some(surface.dc()),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .context("paint stage=layered-window")?;
        }

        Ok(())
    }

    pub fn unpaint(&mut self, _state: SwitchAppsState) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.show = false;
    }

    pub fn find_clicked_app_index(&self, state: &SwitchAppsState) -> Option<usize> {
        let cursor_pos = unsafe {
            let mut pos = POINT::default();
            GetCursorPos(&mut pos).ok()?;
            pos
        };

        let dpi_scale = get_dpi_scale(self.hwnd);
        let icon_size_max = (ICON_SIZE_BASE as f64 * dpi_scale) as i32;
        let border_size = (WINDOW_BORDER_SIZE_BASE as f64 * dpi_scale) as i32;
        let icon_border = (ICON_BORDER_SIZE_BASE as f64 * dpi_scale) as i32;

        let Coordinate {
            x, y, item_size, ..
        } = Coordinate::new(
            state.apps.len() as i32,
            icon_size_max,
            border_size,
            icon_border,
        )
        .ok()?;

        let xpos = cursor_pos.x - x;
        let ypos = cursor_pos.y - y;

        let cy = border_size;
        for (i, _) in state.apps.iter().enumerate() {
            let cx = border_size + item_size * (i as i32);
            if xpos >= cx && xpos < cx + item_size && ypos >= cy && ypos < cy + item_size {
                return Some(i);
            }
        }
        None
    }
}

const fn theme_color(light_theme: bool) -> (u32, u32) {
    match light_theme {
        true => (FG_LIGHT_COLOR, BG_LIGHT_COLOR),
        false => (FG_DARK_COLOR, BG_DARK_COLOR),
    }
}

fn get_dpi_scale(hwnd: HWND) -> f64 {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        if dpi == 0 {
            1.0
        } else {
            dpi as f64 / 96.0
        }
    }
}

struct Coordinate {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    icon_size: i32,
    item_size: i32,
}

impl Coordinate {
    fn new(num_apps: i32, icon_size_max: i32, border_size: i32, icon_border: i32) -> Result<Self> {
        if num_apps <= 0 {
            bail!("paint stage=layout empty list");
        }
        let monitor_rect = get_moinitor_rect();
        let monitor_width = monitor_rect
            .right
            .checked_sub(monitor_rect.left)
            .context("paint monitor width overflow")?;
        let monitor_height = monitor_rect
            .bottom
            .checked_sub(monitor_rect.top)
            .context("paint monitor height overflow")?;

        let icon_size =
            ((monitor_width - 2 * border_size) / num_apps - icon_border * 2).min(icon_size_max);
        if icon_size <= 0 || monitor_height <= 0 {
            bail!("paint stage=layout insufficient space");
        }

        let item_size = icon_size + icon_border * 2;
        let width = item_size * num_apps + border_size * 2;
        let height = item_size + border_size * 2;
        let x = monitor_rect.left + (monitor_width - width) / 2;
        let y = monitor_rect.top + (monitor_height - height) / 2;

        Ok(Self {
            x,
            y,
            width,
            height,
            icon_size,
            item_size,
        })
    }
}
