use crate::app::SwitchAppsState;
use crate::badge::{
    badge_geometry, badge_label, create_badge_font, draw_badge, BADGE_BACKGROUND_COLOR,
    BADGE_BORDER_COLOR,
};
use crate::painter_resources::{
    gdiplus_status, BitmapGuard, BrushGuard, FontGuard, GpBrushGuard, GpGraphicsGuard,
    GpImageGuard, GpPathGuard, MemoryDcGuard, RegionGuard, ScreenDcGuard, SelectedObjectGuard,
};
use crate::utils::{get_moinitor_rect, is_light_theme, is_win11};

use anyhow::{anyhow, Context, Result};
use windows::Win32::{
    Foundation::{COLORREF, HWND, POINT, RECT, SIZE},
    Graphics::{
        Gdi::{
            CreateCompatibleBitmap, CreateRoundRectRgn, CreateSolidBrush, FillRect, FillRgn,
            SetStretchBltMode, StretchBlt, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, HALFTONE, HDC,
            SRCCOPY,
        },
        GdiPlus::{
            GdipAddPathArc, GdipClosePathFigure, GdipDrawImageRect, GdipFillPath,
            GdipFillRectangle, GdipSetInterpolationMode, GdipSetSmoothingMode, GdiplusShutdown,
            GdiplusStartup, GdiplusStartupInput, GpBrush, GpGraphics,
            InterpolationModeHighQualityBicubic, SmoothingModeAntiAlias,
        },
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{
            DrawIconEx, GetCursorPos, ShowWindow, UpdateLayeredWindow, DI_NORMAL, SW_HIDE, SW_SHOW,
            ULW_ALPHA,
        },
    },
};

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
    token: usize,
    hwnd: HWND,
    hdc_screen: ScreenDcGuard,
    rounded_corner: bool,
    show: bool,
}

impl GdiAAPainter {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let startup_input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let mut token: usize = 0;
        gdiplus_status(
            unsafe { GdiplusStartup(&mut token, &startup_input, std::ptr::null_mut()) },
            "GdiplusStartup",
        )
        .context("Failed to initialize GDI+")?;

        let hdc_screen = match ScreenDcGuard::new(hwnd) {
            Ok(hdc) => hdc,
            Err(err) => {
                unsafe { GdiplusShutdown(token) };
                return Err(err);
            }
        };
        let rounded_corner = is_win11();

        Ok(Self {
            token,
            hwnd,
            hdc_screen,
            rounded_corner,
            show: false,
        })
    }

    pub fn paint(&mut self, state: &SwitchAppsState) {
        if let Err(err) = self.paint_inner(state) {
            error!("paint failed: {err:#}");
        }
    }

    fn paint_inner(&mut self, state: &SwitchAppsState) -> Result<()> {
        if state.apps.is_empty() {
            return Err(anyhow!("Cannot paint an empty app state"));
        }

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
            state.apps.len() as i32,
            icon_size_max,
            border_size,
            icon_border,
        );
        if width <= 0 || height <= 0 || icon_size <= 0 || item_size <= 0 {
            return Err(anyhow!("Invalid painter dimensions"));
        }

        let corner_radius = if self.rounded_corner {
            item_size / 4
        } else {
            0
        };

        let hwnd = self.hwnd;
        let hdc_screen = self.hdc_screen.get();

        let (fg_color, bg_color) = theme_color(is_light_theme());

        let hdc_mem = MemoryDcGuard::new(hdc_screen)?;
        let bitmap_mem =
            BitmapGuard::new(unsafe { CreateCompatibleBitmap(hdc_screen, width, height) })?;
        let _bitmap_mem_selection =
            SelectedObjectGuard::new(hdc_mem.get(), bitmap_mem.get().into())?;

        let graphics = GpGraphicsGuard::new(hdc_mem.get())?;
        gdiplus_status(
            unsafe { GdipSetSmoothingMode(graphics.get(), SmoothingModeAntiAlias) },
            "GdipSetSmoothingMode",
        )?;
        gdiplus_status(
            unsafe {
                GdipSetInterpolationMode(graphics.get(), InterpolationModeHighQualityBicubic)
            },
            "GdipSetInterpolationMode",
        )?;

        let bg_brush = GpBrushGuard::new(ALPHA_MASK | bg_color)?;

        if self.rounded_corner {
            draw_round_rect(
                graphics.get(),
                bg_brush.get(),
                0.0,
                0.0,
                width as f32,
                height as f32,
                corner_radius as f32,
            )?;
        } else {
            gdiplus_status(
                unsafe {
                    GdipFillRectangle(
                        graphics.get(),
                        bg_brush.get(),
                        0.0,
                        0.0,
                        width as f32,
                        height as f32,
                    )
                },
                "GdipFillRectangle",
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

        let image = GpImageGuard::from_hbitmap(bitmap_icons.get())?;
        gdiplus_status(
            unsafe {
                GdipDrawImageRect(
                    graphics.get(),
                    image.get(),
                    border_size as f32,
                    border_size as f32,
                    icons_width as f32,
                    icons_height as f32,
                )
            },
            "GdipDrawImageRect",
        )?;

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as _,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as _,
            ..Default::default()
        };
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                Some(hdc_screen),
                Some(&POINT { x, y }),
                Some(&SIZE {
                    cx: width,
                    cy: height,
                }),
                Some(hdc_mem.get()),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
        .context("UpdateLayeredWindow failed")?;

        if self.show {
            return Ok(());
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetFocus(Some(self.hwnd));
        }
        self.show = true;
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
            let _ = GetCursorPos(&mut pos);
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
        );

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

impl Drop for GdiAAPainter {
    fn drop(&mut self) {
        unsafe { GdiplusShutdown(self.token) }
    }
}

const fn theme_color(light_theme: bool) -> (u32, u32) {
    match light_theme {
        true => (FG_LIGHT_COLOR, BG_LIGHT_COLOR),
        false => (FG_DARK_COLOR, BG_DARK_COLOR),
    }
}

fn draw_round_rect(
    graphic_ptr: *mut GpGraphics,
    brush_ptr: *mut GpBrush,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    corner_radius: f32,
) -> Result<()> {
    let path = GpPathGuard::new()?;
    let path_ptr = path.get();
    gdiplus_status(
        unsafe {
            GdipAddPathArc(
                path_ptr,
                left,
                top,
                corner_radius,
                corner_radius,
                180.0,
                90.0,
            )
        },
        "GdipAddPathArc",
    )?;
    gdiplus_status(
        unsafe {
            GdipAddPathArc(
                path_ptr,
                right - corner_radius,
                top,
                corner_radius,
                corner_radius,
                270.0,
                90.0,
            )
        },
        "GdipAddPathArc",
    )?;
    gdiplus_status(
        unsafe {
            GdipAddPathArc(
                path_ptr,
                right - corner_radius,
                bottom - corner_radius,
                corner_radius,
                corner_radius,
                0.0,
                90.0,
            )
        },
        "GdipAddPathArc",
    )?;
    gdiplus_status(
        unsafe {
            GdipAddPathArc(
                path_ptr,
                left,
                bottom - corner_radius,
                corner_radius,
                corner_radius,
                90.0,
                90.0,
            )
        },
        "GdipAddPathArc",
    )?;
    gdiplus_status(
        unsafe { GdipClosePathFigure(path_ptr) },
        "GdipClosePathFigure",
    )?;
    gdiplus_status(
        unsafe { GdipFillPath(graphic_ptr, brush_ptr, path_ptr) },
        "GdipFillPath",
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_icons(
    state: &SwitchAppsState,
    hdc_screen: HDC,
    icon_size: i32,
    icon_border: i32,
    width: i32,
    height: i32,
    corner_radius: i32,
    fg_color: u32,
    bg_color: u32,
) -> Result<BitmapGuard> {
    let scaled_width = width * SCALE_FACTOR;
    let scaled_height = height * SCALE_FACTOR;
    let scaled_corner_radius = corner_radius * SCALE_FACTOR;
    let scaled_border_size = icon_border * SCALE_FACTOR;
    let scaled_icon_inner_size = icon_size * SCALE_FACTOR;
    let scaled_icon_outer_size = scaled_icon_inner_size + scaled_border_size * 2;

    if width <= 0 || height <= 0 || scaled_width <= 0 || scaled_height <= 0 {
        return Err(anyhow!("Invalid icon bitmap dimensions"));
    }

    let hdc_tmp = MemoryDcGuard::new(hdc_screen)?;
    let bitmap_tmp =
        BitmapGuard::new(unsafe { CreateCompatibleBitmap(hdc_screen, width, height) })?;
    let _bitmap_tmp_selection = SelectedObjectGuard::new(hdc_tmp.get(), bitmap_tmp.get().into())?;

    let hdc_scaled = MemoryDcGuard::new(hdc_screen)?;
    let bitmap_scaled = BitmapGuard::new(unsafe {
        CreateCompatibleBitmap(hdc_screen, scaled_width, scaled_height)
    })?;
    let _bitmap_scaled_selection =
        SelectedObjectGuard::new(hdc_scaled.get(), bitmap_scaled.get().into())?;

    let fg_brush = BrushGuard::new(unsafe { CreateSolidBrush(COLORREF(fg_color)) })?;
    let bg_brush = BrushGuard::new(unsafe { CreateSolidBrush(COLORREF(bg_color)) })?;
    let has_badges = state.apps.iter().any(|entry| entry.window_count > 1);
    let badge_border_brush = has_badges
        .then(|| unsafe { CreateSolidBrush(COLORREF(BADGE_BORDER_COLOR)) })
        .map(BrushGuard::new)
        .transpose()?;
    let badge_background_brush = has_badges
        .then(|| unsafe { CreateSolidBrush(COLORREF(BADGE_BACKGROUND_COLOR)) })
        .map(BrushGuard::new)
        .transpose()?;
    let badge_font = if has_badges {
        badge_geometry("2", icon_size, height)
            .and_then(|geometry| create_badge_font(geometry.height, SCALE_FACTOR))
            .map(FontGuard::new)
            .transpose()?
    } else {
        None
    };

    let rect = RECT {
        left: 0,
        top: 0,
        right: scaled_width,
        bottom: scaled_height,
    };

    if unsafe { FillRect(hdc_scaled.get(), &rect, bg_brush.get()) } == 0 {
        return Err(anyhow!("FillRect failed"));
    }

    for (i, entry) in state.apps.iter().enumerate() {
        // draw the box for selected icon
        if i == state.index {
            let left = scaled_icon_outer_size * (i as i32);
            let top = 0;
            let right = left + scaled_icon_outer_size;
            let bottom = top + scaled_icon_outer_size;
            let region = RegionGuard::new(unsafe {
                CreateRoundRectRgn(
                    left,
                    top,
                    right,
                    bottom,
                    scaled_corner_radius,
                    scaled_corner_radius,
                )
            })?;
            unsafe { FillRgn(hdc_scaled.get(), region.get(), fg_brush.get()) }
                .ok()
                .map_err(|err| anyhow!("FillRgn failed, {err}"))?;
        }

        let cx = scaled_border_size + scaled_icon_outer_size * (i as i32);
        unsafe {
            DrawIconEx(
                hdc_scaled.get(),
                cx,
                scaled_border_size,
                entry.icon,
                scaled_icon_inner_size,
                scaled_icon_inner_size,
                0,
                None,
                DI_NORMAL,
            )
        }
        .map_err(|err| anyhow!("DrawIconEx failed, {err}"))?;

        if let Some(label) = badge_label(entry.window_count) {
            if let Some(geometry) = badge_geometry(&label, icon_size, height) {
                let scaled_offset = geometry.offset * SCALE_FACTOR;
                let scaled_width = geometry.width * SCALE_FACTOR;
                let scaled_height = geometry.height * SCALE_FACTOR;
                let item_left = scaled_icon_outer_size * (i as i32);
                let right = item_left + scaled_icon_outer_size - scaled_offset;
                let left = right - scaled_width;
                draw_badge(
                    hdc_scaled.get(),
                    &label,
                    left,
                    scaled_offset,
                    scaled_width,
                    scaled_height,
                    SCALE_FACTOR,
                    badge_border_brush
                        .as_ref()
                        .map(BrushGuard::get)
                        .unwrap_or_default(),
                    badge_background_brush
                        .as_ref()
                        .map(BrushGuard::get)
                        .unwrap_or_default(),
                    badge_font.as_ref().map(FontGuard::get),
                );
            }
        }
    }

    if unsafe { SetStretchBltMode(hdc_tmp.get(), HALFTONE) } == 0 {
        return Err(anyhow!("SetStretchBltMode failed"));
    }
    unsafe {
        StretchBlt(
            hdc_tmp.get(),
            0,
            0,
            width,
            height,
            Some(hdc_scaled.get()),
            0,
            0,
            scaled_width,
            scaled_height,
            SRCCOPY,
        )
    }
    .ok()
    .map_err(|err| anyhow!("StretchBlt failed, {err}"))?;

    Ok(bitmap_tmp)
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
    fn new(num_apps: i32, icon_size_max: i32, border_size: i32, icon_border: i32) -> Self {
        let monitor_rect = get_moinitor_rect();
        let monitor_width = monitor_rect.right - monitor_rect.left;
        let monitor_height = monitor_rect.bottom - monitor_rect.top;

        let icon_size =
            ((monitor_width - 2 * border_size) / num_apps - icon_border * 2).min(icon_size_max);

        let item_size = icon_size + icon_border * 2;
        let width = item_size * num_apps + border_size * 2;
        let height = item_size + border_size * 2;
        let x = monitor_rect.left + (monitor_width - width) / 2;
        let y = monitor_rect.top + (monitor_height - height) / 2;

        Self {
            x,
            y,
            width,
            height,
            icon_size,
            item_size,
        }
    }
}
