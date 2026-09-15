use crate::{
    app::{AppEntry, SwitchAppsState},
    badge::{draw_badge, BadgeStyle},
    icon_cache::IconKey,
    layout::{ItemRect, PixelRect},
    pixels::PixelImage,
    render_surface::RenderSurface,
};
use anyhow::Result;
use windows::Win32::Foundation::RECT;

#[derive(PartialEq, Eq)]
struct SpriteKey {
    icon: IconKey,
    revision: Option<u64>,
    count: usize,
    show_badge: bool,
    maximum: u32,
    style: BadgeStyle,
}

impl SpriteKey {
    fn new(entry: &AppEntry, state: &SwitchAppsState) -> Self {
        Self {
            icon: entry.key.clone(),
            revision: entry.icon.as_ref().map(|icon| icon.revision),
            count: entry.window_count,
            show_badge: state.show_badge,
            maximum: state.badge_max,
            style: state.badge_style,
        }
    }
}

pub(super) struct Sprite {
    key: SpriteKey,
    pub(super) plain: PixelImage,
    pub(super) selected: PixelImage,
}

impl Sprite {
    pub(super) fn matches(&self, entry: &AppEntry, state: &SwitchAppsState) -> bool {
        self.key == SpriteKey::new(entry, state)
    }

    pub(super) fn new(
        entry: &AppEntry,
        state: &SwitchAppsState,
        item: &ItemRect,
        scale: i32,
        rounded: bool,
        color: u32,
        metrics: bool,
    ) -> Result<Self> {
        let width = item.outer.width() * scale;
        let height = item.outer.height() * scale;
        let icon = PixelRect {
            left: (item.icon.left - item.outer.left) * scale,
            top: (item.icon.top - item.outer.top) * scale,
            right: (item.icon.right - item.outer.left) * scale,
            bottom: (item.icon.bottom - item.outer.top) * scale,
        };
        let mut content = PixelImage::new(width, height)?;
        if let Some(image) = &entry.icon {
            let resized = image.image.resize(icon.width(), icon.height())?;
            content.compose(&resized, icon.left, icon.top)?;
        } else {
            placeholder(&mut content, icon);
        }
        if state.show_badge && entry.window_count > 1 {
            let started = crate::diagnostics::sample_start(metrics);
            let mut black = RenderSurface::new(width, height)?;
            let mut white = RenderSurface::new(width, height)?;
            let rect = RECT {
                left: icon.left,
                top: icon.top,
                right: icon.right,
                bottom: icon.bottom,
            };
            for (surface, rgb) in [(&mut black, 0), (&mut white, 0xffffff)] {
                surface.fill_rgb(rgb)?;
                draw_badge(
                    surface.dc(),
                    entry.window_count,
                    state.badge_max,
                    rect,
                    state.badge_style,
                )?;
            }
            let badge = PixelImage::recover_alpha(black.pixels()?, white.pixels()?, width, height)?;
            content.compose(&badge, 0, 0)?;
            crate::diagnostics::stage_elapsed("badge-raster", started);
        }
        let mut selected = PixelImage::new(width, height)?;
        selected.rounded_fill(
            PixelRect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
            if rounded {
                width.min(height) as f32 / 8.0
            } else {
                0.0
            },
            color,
        );
        selected.compose(&content, 0, 0)?;
        Ok(Self {
            key: SpriteKey::new(entry, state),
            plain: content.downsample(scale)?,
            selected: selected.downsample(scale)?,
        })
    }
}

fn placeholder(image: &mut PixelImage, icon: PixelRect) {
    let inset = icon.width().min(icon.height()) / 5;
    let rect = PixelRect {
        left: icon.left + inset,
        top: icon.top + inset,
        right: icon.right - inset,
        bottom: icon.bottom - inset,
    };
    image.rounded_fill(rect, inset as f32 / 2.0, 0x929292);
    let border = (icon.width() / 16).max(1);
    image.rounded_fill(
        PixelRect {
            left: rect.left + border,
            top: rect.top + border * 3,
            right: rect.right - border,
            bottom: rect.bottom - border,
        },
        border as f32,
        0xc8c8c8,
    );
}
