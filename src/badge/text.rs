//! Font backend measurement and guarded rasterization for Badge glyphs.
use super::gdi;
use crate::{
    font_resources::{FontResources, FontRole, TextBox},
    layout::PixelRect,
    pixels::PixelImage,
};
use anyhow::{ensure, Result};
use windows::Win32::{
    Foundation::SIZE,
    Graphics::DirectWrite::{IDWriteFactory, IDWriteTextLayout, DWRITE_TEXT_METRICS},
};

const GLYPH_GUARD: i32 = 2;

pub(super) struct Text {
    pub(super) measured: SIZE,
    backend: Backend,
}

enum Backend {
    DirectWrite {
        factory: IDWriteFactory,
        layout: IDWriteTextLayout,
        ink: PixelRect,
    },
    Gdi(gdi::Text),
}

impl Text {
    pub(super) fn measure(
        fonts: Option<&mut FontResources>,
        text: &str,
        pixels: u32,
        size: i32,
    ) -> Result<Self> {
        let Some(fonts) = fonts else {
            let glyphs = gdi::Text::new(text, pixels)?;
            return Ok(Self {
                measured: glyphs.measured,
                backend: Backend::Gdi(glyphs),
            });
        };
        let layout = fonts.layout(
            text,
            TextBox {
                role: FontRole::Badge,
                pixels,
                width: size,
                height: size,
                ellipsis: false,
            },
        )?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            layout.GetMetrics(&mut metrics)?;
        }
        let overhang = unsafe { layout.GetOverhangMetrics() }?;
        ensure!(
            [
                metrics.widthIncludingTrailingWhitespace,
                metrics.height,
                overhang.left,
                overhang.top,
                overhang.right,
                overhang.bottom
            ]
            .iter()
            .all(|value| value.is_finite() && value.abs() < i32::MAX as f32 / 4.0),
            "font stage=badge invalid-metrics"
        );
        let measured = SIZE {
            cx: (metrics.widthIncludingTrailingWhitespace
                + overhang.left.max(0.0)
                + overhang.right.max(0.0))
            .ceil() as i32,
            cy: (metrics.height + overhang.top.max(0.0) + overhang.bottom.max(0.0)).ceil() as i32,
        };
        // DirectWrite overhangs are relative to the layout box; negative
        // values are whitespace. Preserve any ink outside that box as well.
        let ink = PixelRect {
            left: (-overhang.left).floor() as i32,
            top: (-overhang.top).floor() as i32,
            right: (size as f32 + overhang.right).ceil() as i32,
            bottom: (size as f32 + overhang.bottom).ceil() as i32,
        };
        ensure!(
            ink.width() > 0 && ink.height() > 0,
            "font stage=badge empty-metrics"
        );
        Ok(Self {
            measured,
            backend: Backend::DirectWrite {
                factory: fonts.factory.clone(),
                layout,
                ink,
            },
        })
    }

    pub(super) fn rasterize(&self, color: u32) -> Result<PixelImage> {
        match &self.backend {
            Backend::Gdi(glyphs) => glyphs.rasterize(color),
            Backend::DirectWrite {
                factory,
                layout,
                ink,
            } => crate::text_raster::rasterize_at(
                factory,
                layout,
                ink.width() + GLYPH_GUARD * 2,
                ink.height() + GLYPH_GUARD * 2,
                color,
                (
                    (GLYPH_GUARD - ink.left) as f32,
                    (GLYPH_GUARD - ink.top) as f32,
                ),
            ),
        }
    }
}
