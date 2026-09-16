use super::{automatic_side, format_badge_count, text::Text, text_side, BadgeStyle};
use crate::{
    config::BadgeShape, font_resources::FontResources, layout::PixelRect, painter::ICON_SIZE_BASE,
    pixels::PixelImage,
};
use anyhow::{bail, ensure, Context, Result};
use windows::Win32::Foundation::SIZE;

const MAX_FIT_ATTEMPTS: usize = 8;

pub(crate) fn compose(
    content: &mut PixelImage,
    mut fonts: Option<&mut FontResources>,
    count: usize,
    maximum: u32,
    icon: PixelRect,
    style: BadgeStyle,
) -> Result<()> {
    let Some(text) = format_badge_count(count, maximum) else {
        return Ok(());
    };
    let size = icon.width().min(icon.height());
    if size < 4 {
        return Ok(());
    }
    let inset = (size / 32).max(1);
    let available = size - inset * 2;
    let fixed = style.size.map(|value| {
        ((u64::from(value) * size as u64 + ICON_SIZE_BASE as u64 / 2) / ICON_SIZE_BASE as u64)
            .clamp(1, available as u64) as i32
    });
    let padding = (size / ICON_SIZE_BASE).max(1);
    let mut pixels = (u64::from(style.font_size.clamp(8, 24)) * size as u64 / ICON_SIZE_BASE as u64)
        .max(1) as u32;
    for _ in 0..MAX_FIT_ATTEMPTS {
        let glyphs = Text::measure(fonts.as_deref_mut(), &text, pixels, size)?;
        let automatic = automatic_side(size, glyphs.measured, style.shape);
        if automatic > available {
            if !shrink(&mut pixels, available, automatic) {
                break;
            }
            continue;
        }
        let side = fixed.unwrap_or(automatic);
        let image = glyphs.rasterize(style.foreground)?;
        let ink = ink_bounds(&image).context("font stage=badge empty-glyphs")?;
        ensure!(
            ink.left > 0 && ink.top > 0 && ink.right < image.width && ink.bottom < image.height,
            "font stage=badge clipped-glyphs"
        );
        let needed = text_side(
            SIZE {
                cx: ink.width(),
                cy: ink.height(),
            },
            padding,
            style.shape,
        );
        if needed > side && pixels > 1 {
            if !shrink(&mut pixels, side, needed) {
                break;
            }
            continue;
        }
        let mut image = crop_ink(image, ink);
        if needed > side {
            image = fit_subpixel_ink(image, side, padding, style.shape)?;
        }
        let mut badge = PixelImage::new(side, side)?;
        badge.rounded_fill(
            PixelRect {
                left: 0,
                top: 0,
                right: side,
                bottom: side,
            },
            match style.shape {
                BadgeShape::Circle => side as f32 / 2.0,
                BadgeShape::Square => 0.0,
            },
            style.background,
        );
        // Center visible ink, not the font's ascender/descender line box. The
        // half-pixel parity remainder stays within one raster pixel.
        badge.compose(&image, (side - image.width) / 2, (side - image.height) / 2)?;
        return content.compose(&badge, icon.right - inset - side, icon.top + inset);
    }
    bail!(
        "font stage=badge cannot-fit count={text} icon={size} shape={:?} size={:?}",
        style.shape,
        style.size
    )
}

fn shrink(pixels: &mut u32, available: i32, needed: i32) -> bool {
    let reduced = ((u64::from(*pixels) * available as u64 / needed as u64) as u32)
        .saturating_sub(1)
        .max(1);
    if reduced >= *pixels {
        return false;
    }
    *pixels = reduced;
    true
}

fn crop_ink(mut image: PixelImage, ink: PixelRect) -> PixelImage {
    let row_bytes = ink.width() as usize * 4;
    for row in 0..ink.height() {
        let source = ((ink.top + row) * image.width + ink.left) as usize * 4;
        image
            .data
            .copy_within(source..source + row_bytes, row as usize * row_bytes);
    }
    image.width = ink.width();
    image.height = ink.height();
    image.data.truncate(row_bytes * image.height as usize);
    image
}

fn fit_subpixel_ink(
    image: PixelImage,
    side: i32,
    mut padding: i32,
    shape: BadgeShape,
) -> Result<PixelImage> {
    // Some hinted system fonts cannot get narrower at a one-pixel font size.
    // Downscale the complete count instead of clipping or rejecting valid INI.
    while padding > 0 && text_side(SIZE { cx: 1, cy: 1 }, padding, shape) > side {
        padding -= 1;
    }
    let (mut width, mut height) = (image.width, image.height);
    for _ in 0..MAX_FIT_ATTEMPTS {
        let needed = text_side(
            SIZE {
                cx: width,
                cy: height,
            },
            padding,
            shape,
        );
        if needed <= side {
            let resized = image.resize(width, height)?;
            let ink = ink_bounds(&resized).context("font stage=badge empty-scaled-glyphs")?;
            return Ok(crop_ink(resized, ink));
        }
        width = ((i64::from(width) * i64::from(side) / i64::from(needed)) as i32).max(1);
        height = ((i64::from(height) * i64::from(side) / i64::from(needed)) as i32).max(1);
    }
    bail!("font stage=badge cannot-fit minimum-glyphs size={side}")
}

fn ink_bounds(image: &PixelImage) -> Option<PixelRect> {
    let mut bounds = PixelRect {
        left: image.width,
        top: image.height,
        right: 0,
        bottom: 0,
    };
    for (index, pixel) in image.data.as_chunks::<4>().0.iter().enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let x = index as i32 % image.width;
        let y = index as i32 / image.width;
        bounds.left = bounds.left.min(x);
        bounds.top = bounds.top.min(y);
        bounds.right = bounds.right.max(x + 1);
        bounds.bottom = bounds.bottom.max(y + 1);
    }
    (bounds.right > bounds.left && bounds.bottom > bounds.top).then_some(bounds)
}
