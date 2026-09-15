//! Explicit top-down premultiplied BGRA pixels. No HBITMAP-to-GDI+ alpha conversion.
use crate::{layout::PixelRect, utils::gdi::checked_bitmap_bytes};
use anyhow::{ensure, Context, Result};

#[derive(Debug, Clone)]
pub(crate) struct PixelImage {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) data: Vec<u8>,
}

impl PixelImage {
    pub(crate) fn new(width: i32, height: i32) -> Result<Self> {
        let bytes = checked_bitmap_bytes(width, height)?;
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .context("pixels stage=allocate")?;
        data.resize(bytes, 0);
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub(crate) fn rounded_fill(&mut self, rect: PixelRect, radius: f32, color: u32) {
        let radius = radius
            .max(0.0)
            .min(rect.width().min(rect.height()) as f32 / 2.0);
        for y in rect.top.max(0)..rect.bottom.min(self.height) {
            for x in rect.left.max(0)..rect.right.min(self.width) {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let cx = px.clamp(rect.left as f32 + radius, rect.right as f32 - radius);
                let cy = py.clamp(rect.top as f32 + radius, rect.bottom as f32 - radius);
                let coverage = if radius == 0.0 {
                    1.0
                } else {
                    (radius + 0.5 - (px - cx).hypot(py - cy)).clamp(0.0, 1.0)
                };
                let offset = ((y * self.width + x) * 4) as usize;
                over(
                    &mut self.data[offset..offset + 4],
                    &premultiply(color, (coverage * 255.0).round() as u8),
                );
            }
        }
    }

    pub(crate) fn compose(&mut self, source: &Self, x: i32, y: i32) -> Result<()> {
        compose(&mut self.data, self.width, self.height, source, x, y)
    }

    pub(crate) fn resize(&self, width: i32, height: i32) -> Result<Self> {
        let mut output = Self::new(width, height)?;
        for y in 0..height {
            let sy =
                ((f64::from(y) + 0.5) * f64::from(self.height) / f64::from(height) - 0.5).max(0.0);
            let y0 = (sy.floor() as i32).min(self.height - 1);
            let y1 = (y0 + 1).min(self.height - 1);
            let fy = sy.fract();
            for x in 0..width {
                let sx = ((f64::from(x) + 0.5) * f64::from(self.width) / f64::from(width) - 0.5)
                    .max(0.0);
                let x0 = (sx.floor() as i32).min(self.width - 1);
                let x1 = (x0 + 1).min(self.width - 1);
                let fx = sx.fract();
                for channel in 0..4 {
                    let at =
                        |x, y| f64::from(self.data[((y * self.width + x) * 4) as usize + channel]);
                    let a = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
                    let b = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
                    output.data[((y * width + x) * 4) as usize + channel] =
                        (a * (1.0 - fy) + b * fy).round() as u8;
                }
            }
        }
        Ok(output)
    }

    pub(crate) fn downsample(self, factor: i32) -> Result<Self> {
        ensure!(
            factor > 0 && self.width % factor == 0 && self.height % factor == 0,
            "pixels stage=downsample dimensions"
        );
        if factor == 1 {
            return Ok(self);
        }
        let mut output = Self::new(self.width / factor, self.height / factor)?;
        let samples = (factor * factor) as u32;
        for y in 0..output.height {
            for x in 0..output.width {
                let mut sum = [0u32; 4];
                for dy in 0..factor {
                    for dx in 0..factor {
                        let offset =
                            (((y * factor + dy) * self.width + x * factor + dx) * 4) as usize;
                        for (channel, sum) in sum.iter_mut().enumerate() {
                            *sum += u32::from(self.data[offset + channel]);
                        }
                    }
                }
                let offset = ((y * output.width + x) * 4) as usize;
                for (channel, sum) in sum.into_iter().enumerate() {
                    output.data[offset + channel] = ((sum + samples / 2) / samples) as u8;
                }
            }
        }
        Ok(output)
    }

    pub(crate) fn recover_alpha(
        black: &[u8],
        white: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Self> {
        let mut image = Self::new(width, height)?;
        ensure!(
            black.len() == image.data.len() && white.len() == black.len(),
            "pixels stage=alpha dimensions"
        );
        for ((destination, black), white) in image
            .data
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(black.as_chunks::<4>().0.iter())
            .zip(white.as_chunks::<4>().0.iter())
        {
            let difference: u16 = (0..3)
                .map(|channel| u16::from(white[channel].saturating_sub(black[channel])))
                .sum();
            let alpha = 255 - ((difference + 1) / 3) as u8;
            for channel in 0..3 {
                destination[channel] = black[channel].min(alpha);
            }
            destination[3] = alpha;
        }
        Ok(image)
    }
}

pub(crate) fn premultiply(rgb: u32, alpha: u8) -> [u8; 4] {
    let component = |shift: u32| (((rgb >> shift) & 255u32) * u32::from(alpha) + 127) / 255;
    [
        component(0) as u8,
        component(8) as u8,
        component(16) as u8,
        alpha,
    ]
}

fn over(destination: &mut [u8], source: &[u8]) {
    let inverse = 255 - u16::from(source[3]);
    for channel in 0..4 {
        destination[channel] = (u16::from(source[channel])
            + (u16::from(destination[channel]) * inverse + 127) / 255)
            .min(255) as u8;
    }
}

pub(crate) fn compose(
    destination: &mut [u8],
    width: i32,
    height: i32,
    source: &PixelImage,
    x: i32,
    y: i32,
) -> Result<()> {
    ensure!(
        destination.len() == checked_bitmap_bytes(width, height)?
            && x >= 0
            && y >= 0
            && source.width <= width - x
            && source.height <= height - y,
        "pixels stage=compose bounds"
    );
    for sy in 0..source.height {
        let start = (((y + sy) * width + x) * 4) as usize;
        let source_start = (sy * source.width * 4) as usize;
        for (target, input) in destination[start..start + source.width as usize * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(
                source.data[source_start..source_start + source.width as usize * 4]
                    .as_chunks::<4>()
                    .0
                    .iter(),
            )
        {
            over(target, input);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_order_and_transparent_edges_survive_composition_and_scaling() {
        assert_eq!(premultiply(0xff0000, 128), [0, 0, 128, 128]);
        let black = [0, 0, 128, 0, 0, 0, 0, 0];
        let white = [127, 127, 255, 0, 255, 255, 255, 0];
        let icon = PixelImage::recover_alpha(&black, &white, 2, 1).unwrap();
        assert_eq!(&icon.data, &[0, 0, 128, 128, 0, 0, 0, 0]);
        let scaled = icon.resize(8, 4).unwrap();
        for pixel in scaled.data.as_chunks::<4>().0.iter() {
            assert!(pixel[..3].iter().all(|c| *c <= pixel[3]));
        }
        let mut background = PixelImage::new(2, 1).unwrap();
        background
            .data
            .copy_from_slice(&[255, 0, 0, 255, 255, 0, 0, 255]);
        background.compose(&icon, 0, 0).unwrap();
        assert_eq!(&background.data, &[127, 0, 128, 255, 255, 0, 0, 255]);
        assert!(background.compose(&icon, 1, 0).is_err());
    }
    #[test]
    fn rounded_surfaces_have_clear_corners_and_premultiplied_edges() {
        let mut image = PixelImage::new(40, 40).unwrap();
        image.rounded_fill(
            PixelRect {
                left: 0,
                top: 0,
                right: 40,
                bottom: 40,
            },
            12.0,
            0xd04020,
        );
        assert_eq!(image.data[3], 0);
        assert_eq!(image.data[(20 * 40 + 20) * 4 + 3], 255);
        assert!(image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] > 0 && pixel[3] < 255));
        for pixel in image.downsample(2).unwrap().data.as_chunks::<4>().0.iter() {
            assert!(pixel[..3].iter().all(|c| *c <= pixel[3]));
        }
    }
}
