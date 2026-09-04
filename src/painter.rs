use crate::app::SwitchAppsState;
use crate::badge::{
    badge_geometry, badge_label, create_badge_font, draw_badge, BADGE_BACKGROUND_COLOR,
    BADGE_BORDER_COLOR,
};
use crate::config::AppearanceConfig;
use crate::layout::LayoutSnapshot;
use crate::metrics::StageTimer;
use crate::painter_resources::{
    gdiplus_status, BitmapSurface, BrushGuard, FontGuard, GpBrushGuard, GpGraphicsGuard,
    GpImageGuard, GpPathGuard, RegionGuard, ScreenDcGuard,
};
use crate::utils::{get_monitor_context, is_light_theme, is_win11};

use anyhow::{anyhow, Context, Result};
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::{COLORREF, HWND, POINT, RECT, SIZE},
    Graphics::{
        Gdi::{
            BitBlt, CreateRoundRectRgn, CreateSolidBrush, FillRect, FillRgn, SetStretchBltMode,
            StretchBlt, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, HALFTONE, HBITMAP, HDC, SRCCOPY,
        },
        GdiPlus::{
            GdipAddPathArc, GdipClosePathFigure, GdipDrawImageRect, GdipFillPath,
            GdipFillRectangle, GdipGraphicsClear, GdipSetInterpolationMode, GdipSetSmoothingMode,
            GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, GpBrush, GpGraphics,
            InterpolationModeHighQualityBicubic, SmoothingModeAntiAlias,
        },
    },
    UI::{
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
pub const SCALE_FACTOR: i32 = 6;
const THEME_CACHE_TTL: Duration = Duration::from_secs(1);

// GDI Antialiasing Painter
pub struct GdiAAPainter {
    token: usize,
    hwnd: HWND,
    hdc_screen: ScreenDcGuard,
    rounded_corner: bool,
    show: bool,
    appearance: AppearanceConfig,
    theme_cache: Option<(bool, Instant)>,
    last_layout: Option<LayoutSnapshot>,
    panel_surface: Option<BitmapSurface>,
    icon_surface: Option<BitmapSurface>,
    frame_icon_surface: Option<BitmapSurface>,
    scaled_icon_surface: Option<BitmapSurface>,
    icon_layer_key: Option<IconLayerKey>,
}

impl GdiAAPainter {
    pub fn new(hwnd: HWND, appearance: &AppearanceConfig) -> Result<Self> {
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
            appearance: appearance.clone(),
            theme_cache: None,
            last_layout: None,
            panel_surface: None,
            icon_surface: None,
            frame_icon_surface: None,
            scaled_icon_surface: None,
            icon_layer_key: None,
        })
    }

    pub fn paint(&mut self, state: &SwitchAppsState) {
        if let Err(err) = self.paint_inner(state) {
            error!("paint failed: {err:#}");
        }
    }

    fn paint_inner(&mut self, state: &SwitchAppsState) -> Result<()> {
        let _paint_timer = StageTimer::new(if self.show { "paint" } else { "first_frame" });
        if state.apps.is_empty() {
            return Err(anyhow!("Cannot paint an empty app state"));
        }

        let layout_timer = StageTimer::new("layout");
        let monitor = match self.last_layout.as_ref() {
            Some(layout) if self.show && !layout.monitor.handle.is_invalid() => layout.monitor,
            _ => get_monitor_context(self.appearance.monitor, self.appearance.use_work_area)?,
        };
        let layout = LayoutSnapshot::new(state.apps.len(), state.index, &self.appearance, monitor)?;
        let width = layout.panel_width();
        let height = layout.panel_height();
        layout_timer.finish();

        let corner_radius = if self.rounded_corner {
            layout.item_size / 4
        } else {
            0
        };

        let hwnd = self.hwnd;
        let hdc_screen = self.hdc_screen.get();

        let light_theme = match self.theme_cache {
            Some((light_theme, checked_at)) if checked_at.elapsed() < THEME_CACHE_TTL => {
                light_theme
            }
            _ => {
                let light_theme = is_light_theme();
                self.theme_cache = Some((light_theme, Instant::now()));
                light_theme
            }
        };
        let (fg_color, bg_color) = theme_color(light_theme);

        let hdc_mem = ensure_surface(&mut self.panel_surface, hdc_screen, width, height)?.dc();

        let render_timer = StageTimer::new("render");
        let graphics = GpGraphicsGuard::new(hdc_mem)?;
        gdiplus_status(
            unsafe { GdipGraphicsClear(graphics.get(), 0) },
            "GdipGraphicsClear",
        )?;
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

        let bitmap_icons = draw_icons(
            state,
            &layout,
            hdc_screen,
            corner_radius,
            fg_color,
            bg_color,
            self.appearance.show_badge,
            self.appearance.badge_max,
            &mut self.icon_surface,
            &mut self.frame_icon_surface,
            &mut self.scaled_icon_surface,
            &mut self.icon_layer_key,
        )?;

        let image = GpImageGuard::from_hbitmap(bitmap_icons)?;
        gdiplus_status(
            unsafe {
                GdipDrawImageRect(
                    graphics.get(),
                    image.get(),
                    layout.panel_padding as f32,
                    layout.panel_padding as f32,
                    layout.content_width as f32,
                    layout.content_height as f32,
                )
            },
            "GdipDrawImageRect",
        )?;
        render_timer.finish();

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as _,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as _,
            ..Default::default()
        };
        let update_timer = StageTimer::new("update_layered_window");
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                Some(hdc_screen),
                Some(&POINT {
                    x: layout.panel_rect.left,
                    y: layout.panel_rect.top,
                }),
                Some(&SIZE {
                    cx: width,
                    cy: height,
                }),
                Some(hdc_mem),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
        .context("UpdateLayeredWindow failed")?;
        update_timer.finish();
        self.last_layout = Some(layout);

        if self.show {
            return Ok(());
        }
        let show_timer = StageTimer::new("show_window");
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetFocus(Some(self.hwnd));
        }
        show_timer.finish();
        self.show = true;
        Ok(())
    }

    pub fn unpaint(&mut self, _state: SwitchAppsState) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.show = false;
        self.last_layout = None;
        self.release_surfaces();
    }

    pub fn invalidate_layout(&mut self) {
        self.last_layout = None;
        self.release_surfaces();
    }

    fn release_surfaces(&mut self) {
        self.scaled_icon_surface = None;
        self.frame_icon_surface = None;
        self.icon_surface = None;
        self.icon_layer_key = None;
        self.panel_surface = None;
    }

    pub fn find_clicked_app_index(&self, state: &SwitchAppsState) -> Option<usize> {
        let mut cursor_pos = POINT::default();
        if let Err(err) = unsafe { GetCursorPos(&mut cursor_pos) } {
            warn!("failed to get cursor position for hit testing: {err}");
            return None;
        }
        let layout = self.last_layout.as_ref()?;
        if layout.app_count != state.apps.len() {
            return None;
        }
        layout
            .hit_test(cursor_pos)
            .filter(|index| *index < state.apps.len())
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
    layout: &LayoutSnapshot,
    hdc_screen: HDC,
    corner_radius: i32,
    fg_color: u32,
    bg_color: u32,
    show_badge: bool,
    badge_max: usize,
    icon_surface: &mut Option<BitmapSurface>,
    frame_icon_surface: &mut Option<BitmapSurface>,
    scaled_icon_surface: &mut Option<BitmapSurface>,
    icon_layer_key: &mut Option<IconLayerKey>,
) -> Result<HBITMAP> {
    if layout.items.is_empty() {
        return Err(anyhow!("Layout has no visible app items"));
    }

    let width = layout.content_width;
    let height = layout.content_height;
    let item_size = layout.item_size;
    let icon_size = layout.icon_size;
    let icon_padding = layout.icon_padding;
    let scaled_item_size = layout
        .item_size
        .checked_mul(SCALE_FACTOR)
        .ok_or_else(|| anyhow!("Scaled icon item size overflow"))?;
    let scaled_corner_radius = corner_radius
        .checked_mul(SCALE_FACTOR)
        .ok_or_else(|| anyhow!("Scaled corner radius overflow"))?;
    let scaled_padding = icon_padding
        .checked_mul(SCALE_FACTOR)
        .ok_or_else(|| anyhow!("Scaled icon padding overflow"))?;
    let scaled_icon_size = icon_size
        .checked_mul(SCALE_FACTOR)
        .ok_or_else(|| anyhow!("Scaled icon size overflow"))?;

    if width <= 0 || height <= 0 || item_size <= 0 || scaled_item_size <= 0 {
        return Err(anyhow!("Invalid icon bitmap dimensions"));
    }

    let hdc_static = ensure_surface(icon_surface, hdc_screen, width, height)?.dc();
    let frame_surface = ensure_surface(frame_icon_surface, hdc_screen, width, height)?;
    let hdc_frame = frame_surface.dc();
    let bitmap_frame = frame_surface.bitmap();
    let hdc_scaled = ensure_surface(
        scaled_icon_surface,
        hdc_screen,
        scaled_item_size,
        scaled_item_size,
    )?
    .dc();

    let fg_brush = BrushGuard::new(unsafe { CreateSolidBrush(COLORREF(fg_color)) })?;
    let bg_brush = BrushGuard::new(unsafe { CreateSolidBrush(COLORREF(bg_color)) })?;
    let has_badges = show_badge
        && layout.items.iter().any(|item| {
            state
                .apps
                .get(item.app_index)
                .map(|entry| entry.window_count > 1)
                .unwrap_or(false)
        });
    let badge_border_brush = has_badges
        .then(|| unsafe { CreateSolidBrush(COLORREF(BADGE_BORDER_COLOR)) })
        .map(BrushGuard::new)
        .transpose()?;
    let badge_background_brush = has_badges
        .then(|| unsafe { CreateSolidBrush(COLORREF(BADGE_BACKGROUND_COLOR)) })
        .map(BrushGuard::new)
        .transpose()?;
    let badge_font = if has_badges {
        badge_geometry("2", icon_size, item_size)
            .and_then(|geometry| create_badge_font(geometry.height, SCALE_FACTOR))
            .map(FontGuard::new)
            .transpose()?
    } else {
        None
    };

    let rect = RECT {
        left: 0,
        top: 0,
        right: scaled_item_size,
        bottom: scaled_item_size,
    };

    let draw_item = |entry: &crate::app::AppEntry, selected: bool| -> Result<()> {
        if unsafe { FillRect(hdc_scaled, &rect, bg_brush.get()) } == 0 {
            return Err(anyhow!("FillRect failed"));
        }
        if selected {
            let region = RegionGuard::new(unsafe {
                CreateRoundRectRgn(
                    0,
                    0,
                    scaled_item_size,
                    scaled_item_size,
                    scaled_corner_radius,
                    scaled_corner_radius,
                )
            })?;
            unsafe { FillRgn(hdc_scaled, region.get(), fg_brush.get()) }
                .ok()
                .map_err(|err| anyhow!("FillRgn failed, {err}"))?;
        }

        unsafe {
            DrawIconEx(
                hdc_scaled,
                scaled_padding,
                scaled_padding,
                entry.icon,
                scaled_icon_size,
                scaled_icon_size,
                0,
                None,
                DI_NORMAL,
            )
        }
        .map_err(|err| anyhow!("DrawIconEx failed, {err}"))?;

        if show_badge {
            if let Some(label) = badge_label(entry.window_count, badge_max) {
                if let Some(geometry) = badge_geometry(&label, icon_size, item_size) {
                    let scaled_offset = geometry.offset * SCALE_FACTOR;
                    let scaled_width = geometry.width * SCALE_FACTOR;
                    let scaled_height = geometry.height * SCALE_FACTOR;
                    let right = scaled_item_size - scaled_offset;
                    let left = right - scaled_width;
                    draw_badge(
                        hdc_scaled,
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
        Ok(())
    };

    if unsafe { SetStretchBltMode(hdc_static, HALFTONE) } == 0
        || unsafe { SetStretchBltMode(hdc_frame, HALFTONE) } == 0
    {
        return Err(anyhow!("SetStretchBltMode failed"));
    }

    let next_layer_key = IconLayerKey {
        width,
        height,
        icon_size,
        icon_padding,
        bg_color,
        show_badge,
        badge_max,
        entries: layout
            .items
            .iter()
            .map(|item| {
                let entry = state
                    .apps
                    .get(item.app_index)
                    .ok_or_else(|| anyhow!("Layout app index is out of range"))?;
                Ok((
                    item.app_index,
                    entry.icon.0 as isize,
                    entry.window_count,
                    item.x,
                    item.y,
                ))
            })
            .collect::<Result<Vec<_>>>()?,
    };
    if icon_layer_key.as_ref() != Some(&next_layer_key) {
        let static_layer_timer = StageTimer::new("static_icon_layer");
        let static_rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        if unsafe { FillRect(hdc_static, &static_rect, bg_brush.get()) } == 0 {
            return Err(anyhow!("FillRect static icon layer failed"));
        }
        for item in &layout.items {
            let entry = state
                .apps
                .get(item.app_index)
                .ok_or_else(|| anyhow!("Layout app index is out of range"))?;
            draw_item(entry, false)?;
            unsafe {
                StretchBlt(
                    hdc_static,
                    item.x,
                    item.y,
                    item_size,
                    item_size,
                    Some(hdc_scaled),
                    0,
                    0,
                    scaled_item_size,
                    scaled_item_size,
                    SRCCOPY,
                )
            }
            .ok()
            .map_err(|err| anyhow!("StretchBlt static icon failed, {err}"))?;
        }
        *icon_layer_key = Some(next_layer_key);
        static_layer_timer.finish();
    }

    unsafe {
        BitBlt(
            hdc_frame,
            0,
            0,
            width,
            height,
            Some(hdc_static),
            0,
            0,
            SRCCOPY,
        )
    }
    .map_err(|err| anyhow!("BitBlt icon layer failed, {err}"))?;

    let selected_item = layout
        .items
        .iter()
        .find(|item| item.app_index == state.index)
        .ok_or_else(|| anyhow!("Selected app is not on the current layout page"))?;
    let selected = state
        .apps
        .get(state.index)
        .ok_or_else(|| anyhow!("Selected app index is out of range"))?;
    draw_item(selected, true)?;
    unsafe {
        StretchBlt(
            hdc_frame,
            selected_item.x,
            selected_item.y,
            item_size,
            item_size,
            Some(hdc_scaled),
            0,
            0,
            scaled_item_size,
            scaled_item_size,
            SRCCOPY,
        )
    }
    .ok()
    .map_err(|err| anyhow!("StretchBlt selected icon failed, {err}"))?;

    Ok(bitmap_frame)
}

fn ensure_surface(
    surface: &mut Option<BitmapSurface>,
    reference: HDC,
    width: i32,
    height: i32,
) -> Result<&BitmapSurface> {
    let recreate = surface
        .as_ref()
        .map(|surface| !surface.matches(width, height))
        .unwrap_or(true);
    if recreate {
        let replacement = BitmapSurface::new(reference, width, height)?;
        *surface = Some(replacement);
    }
    surface
        .as_ref()
        .ok_or_else(|| anyhow!("Bitmap surface was not initialized"))
}

#[derive(Debug, PartialEq, Eq)]
struct IconLayerKey {
    width: i32,
    height: i32,
    icon_size: i32,
    icon_padding: i32,
    bg_color: u32,
    show_badge: bool,
    badge_max: usize,
    entries: Vec<(usize, isize, usize, i32, i32)>,
}
