use super::{draw_badge, format_badge_count, required_diameter, BadgeStyle};
use crate::{
    font_resources::{FontResources, FontRole, TextBox},
    layout::PixelRect,
    pixels::PixelImage,
    render_surface::RenderSurface,
};
use anyhow::{bail, Result};
use windows::Win32::{
    Foundation::{RECT, SIZE},
    Graphics::DirectWrite::DWRITE_TEXT_METRICS,
};

pub(crate) fn compose(
    content: &mut PixelImage,
    fonts: Option<&mut FontResources>,
    count: usize,
    maximum: u32,
    icon: PixelRect,
    style: BadgeStyle,
) -> Result<()> {
    let Some(text) = format_badge_count(count, maximum) else {
        return Ok(());
    };
    let Some(fonts) = fonts else {
        let mut black = RenderSurface::new(content.width, content.height)?;
        let mut white = RenderSurface::new(content.width, content.height)?;
        let rect = RECT {
            left: icon.left,
            top: icon.top,
            right: icon.right,
            bottom: icon.bottom,
        };
        for (surface, rgb) in [(&mut black, 0), (&mut white, 0xffffff)] {
            surface.fill_rgb(rgb)?;
            draw_badge(surface.dc(), count, maximum, rect, style)?;
        }
        return content.compose(
            &PixelImage::recover_alpha(
                black.pixels()?,
                white.pixels()?,
                content.width,
                content.height,
            )?,
            0,
            0,
        );
    };
    let size = icon.width().min(icon.height());
    let inset = (size / 32).max(1);
    let available = size - inset * 2;
    let mut pixels = (style.font_size * size as u32 / crate::painter::ICON_SIZE_BASE as u32).max(1);
    for _ in 0..8 {
        let layout = fonts.layout(
            &text,
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
        let measured = SIZE {
            cx: (metrics.widthIncludingTrailingWhitespace
                + overhang.left.max(0.0)
                + overhang.right.max(0.0))
            .ceil() as i32,
            cy: (metrics.height + overhang.top.max(0.0) + overhang.bottom.max(0.0)).ceil() as i32,
        };
        let diameter = required_diameter(size, measured);
        if diameter > available {
            pixels = ((u64::from(pixels) * available as u64 / diameter as u64) as u32)
                .saturating_sub(1)
                .max(1);
            continue;
        }
        unsafe {
            layout.SetMaxWidth(diameter as f32)?;
            layout.SetMaxHeight(diameter as f32)?;
        }
        let mut circle = PixelImage::new(diameter, diameter)?;
        circle.rounded_fill(
            PixelRect {
                left: 0,
                top: 0,
                right: diameter,
                bottom: diameter,
            },
            diameter as f32 / 2.0,
            style.background,
        );
        let text = crate::text_raster::rasterize(
            &fonts.factory,
            &layout,
            diameter,
            diameter,
            style.foreground,
        )?;
        circle.compose(&text, 0, 0)?;
        return content.compose(&circle, icon.right - inset - diameter, icon.top + inset);
    }
    bail!("font stage=badge cannot-fit count")
}
