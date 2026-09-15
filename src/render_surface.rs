use crate::{
    layout::PixelRect,
    pixels::PixelImage,
    utils::gdi::{checked_bitmap_bytes, MemoryDc, OwnedGdiObject, SavedDc},
};
use anyhow::{ensure, Context, Result};
use std::{marker::PhantomData, ptr::NonNull, rc::Rc};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, GdiFlush, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HDC,
};

/// Selection is restored before the DIB and DC are destroyed. Pixel references
/// are local borrows; neither this native surface nor its DC crosses threads.
pub(crate) struct RenderSurface {
    _selection: SavedDc,
    _bitmap: OwnedGdiObject,
    dc: MemoryDc,
    bits: NonNull<u8>,
    length: usize,
    pub(crate) width: i32,
    pub(crate) height: i32,
    _thread: PhantomData<Rc<()>>,
}

impl RenderSurface {
    pub(crate) fn new(width: i32, height: i32) -> Result<Self> {
        let length = checked_bitmap_bytes(width, height)?;
        let raw = unsafe { CreateCompatibleDC(None) };
        ensure!(!raw.is_invalid(), "render stage=create-dc failed");
        let dc = MemoryDc(raw);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap =
            unsafe { CreateDIBSection(Some(raw), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .context("render stage=create-dib")?;
        let bitmap = OwnedGdiObject::new(bitmap.into(), "render-dib")?;
        let bits = NonNull::new(bits.cast()).context("render stage=dib null-pixels")?;
        let selection = SavedDc::new(raw)?;
        selection.select(bitmap.0)?;
        let mut surface = Self {
            _selection: selection,
            _bitmap: bitmap,
            dc,
            bits,
            length,
            width,
            height,
            _thread: PhantomData,
        };
        surface.pixels_mut()?.fill(0);
        Ok(surface)
    }

    pub(crate) fn dc(&self) -> HDC {
        self.dc.0
    }
    pub(crate) fn pixels(&self) -> Result<&[u8]> {
        unsafe { GdiFlush() }
            .ok()
            .context("render stage=gdi-flush")?;
        Ok(unsafe { std::slice::from_raw_parts(self.bits.as_ptr(), self.length) })
    }
    pub(crate) fn pixels_mut(&mut self) -> Result<&mut [u8]> {
        unsafe { GdiFlush() }
            .ok()
            .context("render stage=gdi-flush")?;
        Ok(unsafe { std::slice::from_raw_parts_mut(self.bits.as_ptr(), self.length) })
    }
    pub(crate) fn fill_rgb(&mut self, rgb: u32) -> Result<()> {
        let pixel = crate::pixels::premultiply(rgb, 255);
        for destination in self.pixels_mut()?.as_chunks_mut::<4>().0.iter_mut() {
            destination.copy_from_slice(&pixel);
        }
        Ok(())
    }
    pub(crate) fn restore(&mut self, background: &PixelImage, rect: PixelRect) -> Result<()> {
        ensure!(
            background.width == self.width
                && background.height == self.height
                && rect.left >= 0
                && rect.top >= 0
                && rect.right <= self.width
                && rect.bottom <= self.height,
            "render stage=restore bounds"
        );
        let width = self.width;
        let pixels = self.pixels_mut()?;
        for y in rect.top..rect.bottom {
            let start = ((y * width + rect.left) * 4) as usize;
            let end = ((y * width + rect.right) * 4) as usize;
            pixels[start..end].copy_from_slice(&background.data[start..end]);
        }
        Ok(())
    }
    pub(crate) fn compose(&mut self, image: &PixelImage, x: i32, y: i32) -> Result<()> {
        let (width, height) = (self.width, self.height);
        crate::pixels::compose(self.pixels_mut()?, width, height, image, x, y)
    }
}
