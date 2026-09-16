//! Temporary system-font glyphs before the isolated DirectWrite fonts are ready.
use anyhow::{bail, ensure, Context, Result};
use windows::Win32::{
    Foundation::{RECT, SIZE},
    Graphics::Gdi::{
        CreateCompatibleDC, CreateFontW, DrawTextW, GetTextExtentPoint32W, SetBkMode, SetTextColor,
        ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, CLR_INVALID, DEFAULT_CHARSET, DEFAULT_PITCH,
        DT_CENTER, DT_NOCLIP, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_SEMIBOLD,
        OUT_DEFAULT_PRECIS, TRANSPARENT,
    },
};

use crate::{
    pixels::PixelImage,
    render_surface::RenderSurface,
    text_raster::colorref,
    utils::gdi::{MemoryDc, OwnedGdiObject, SavedDc},
};

pub(super) struct Text {
    font: OwnedGdiObject,
    utf16: Vec<u16>,
    pixels: i32,
    pub(super) measured: SIZE,
}

impl Text {
    pub(super) fn new(text: &str, pixels: u32) -> Result<Self> {
        let pixels = i32::try_from(pixels).context("font stage=badge-gdi size")?;
        let font = unsafe {
            CreateFontW(
                -pixels,
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
                windows::core::w!("Segoe UI"),
            )
        };
        let font = OwnedGdiObject::new(font.into(), "badge-font")?;
        let raw_dc = unsafe { CreateCompatibleDC(None) };
        ensure!(!raw_dc.is_invalid(), "font stage=badge-gdi create-dc");
        let dc = MemoryDc(raw_dc);
        let state = SavedDc::new(dc.0)?;
        state.select(font.0)?;
        let utf16: Vec<_> = text.encode_utf16().collect();
        let mut measured = SIZE::default();
        unsafe { GetTextExtentPoint32W(dc.0, &utf16, &mut measured) }
            .ok()
            .context("font stage=badge-gdi measure")?;
        Ok(Self {
            font,
            utf16,
            pixels,
            measured,
        })
    }

    pub(super) fn rasterize(&self, color: u32) -> Result<PixelImage> {
        // The fixed fallback face can overhang its advance box. Keep a full em
        // of guard pixels; the caller verifies that no ink reaches an edge.
        let guard = self.pixels.max(2);
        let width = self.measured.cx + guard * 2;
        let height = self.measured.cy + guard * 2;
        let mut black = RenderSurface::new(width, height)?;
        let mut white = RenderSurface::new(width, height)?;
        let mut utf16 = self.utf16.clone();
        for (surface, rgb) in [(&mut black, 0), (&mut white, 0xffffff)] {
            surface.fill_rgb(rgb)?;
            let state = SavedDc::new(surface.dc())?;
            state.select(self.font.0)?;
            let mut bounds = RECT {
                left: guard,
                top: guard,
                right: guard + self.measured.cx,
                bottom: guard + self.measured.cy,
            };
            unsafe {
                if SetBkMode(surface.dc(), TRANSPARENT) == 0
                    || SetTextColor(surface.dc(), colorref(color)).0 == CLR_INVALID
                {
                    bail!("font stage=badge-gdi color");
                }
                if DrawTextW(
                    surface.dc(),
                    &mut utf16,
                    &mut bounds,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
                ) == 0
                {
                    bail!("font stage=badge-gdi draw");
                }
            }
        }
        PixelImage::recover_alpha(black.pixels()?, white.pixels()?, width, height)
    }
}
