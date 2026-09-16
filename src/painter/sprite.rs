use crate::{
    app::{AppEntry, SwitchAppsState},
    appearance::Appearance,
    badge::BadgeStyle,
    config::Config,
    font_resources::FontResources,
    icon_cache::IconKey,
    layout::{ItemRect, PixelRect},
    pixels::PixelImage,
};
use anyhow::Result;

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
        config: &Config,
        appearance: &Appearance,
        fonts: Option<&mut FontResources>,
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
            let started = crate::diagnostics::sample_start(config.metrics_enabled);
            crate::badge::compose(
                &mut content,
                fonts,
                entry.window_count,
                state.badge_max,
                icon,
                appearance.badge(state.badge_style),
            )?;
            crate::diagnostics::stage_elapsed("badge-raster", started);
        }
        let rect = PixelRect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let radii = appearance.radii(config, state.monitor.dpi, item.outer.height());
        let mut plain = PixelImage::new(width, height)?;
        plain.rounded_fill_opacity(
            rect,
            radii[1] * scale as f32,
            appearance.plain.color,
            appearance.plain.opacity,
        );
        plain.compose(&content, 0, 0)?;
        let mut selected = PixelImage::new(width, height)?;
        selected.rounded_fill_opacity(
            rect,
            radii[2] * scale as f32,
            appearance.selected.color,
            appearance.selected.opacity,
        );
        selected.compose(&content, 0, 0)?;
        let border = (appearance.border_width as f32 * state.monitor.dpi as f32 / 96.0
            * scale as f32)
            .round() as i32;
        selected.rounded_outline(rect, radii[2] * scale as f32, border, appearance.border);
        if border == 0 {
            let mark = (12.0 * state.monitor.dpi as f32 / 96.0 * scale as f32).round() as i32;
            let thickness = (2 * scale).min(height);
            selected.rounded_fill(
                PixelRect {
                    left: (width - mark.min(width)) / 2,
                    right: (width + mark.min(width)) / 2,
                    top: height - thickness,
                    bottom: height,
                },
                0.0,
                appearance.border,
            );
        }
        Ok(Self {
            key: SpriteKey::new(entry, state),
            plain: plain.downsample(scale)?,
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
