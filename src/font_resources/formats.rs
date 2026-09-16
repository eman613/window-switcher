use anyhow::{ensure, Result};
use windows::{
    core::{w, Interface},
    Win32::Graphics::DirectWrite::*,
};

use super::{FontResources, FontRole};

#[derive(Clone, Copy)]
pub(crate) struct TextBox {
    pub(crate) role: FontRole,
    pub(crate) pixels: u32,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) ellipsis: bool,
}

impl FontResources {
    fn format(&mut self, role: FontRole, pixels: u32) -> Result<IDWriteTextFormat> {
        let key = (role, pixels);
        if let Some(format) = self.formats.shift_remove(&key) {
            self.formats.insert(key, format.clone());
            return Ok(format);
        }
        let font = &self.fonts[role.index()];
        let format = unsafe {
            self.factory.CreateTextFormat(
                &font.family,
                &font.collection,
                font.weight,
                font.style,
                DWRITE_FONT_STRETCH_NORMAL,
                pixels as f32,
                w!(""),
            )
        }?;
        unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            if let Ok(factory) = self.factory.cast::<IDWriteFactory2>() {
                let fallback = factory.GetSystemFontFallback()?;
                format
                    .cast::<IDWriteTextFormat1>()?
                    .SetFontFallback(&fallback)?;
            }
        }
        while self.formats.len() >= 64 {
            self.formats.shift_remove_index(0);
        }
        self.formats.insert(key, format.clone());
        Ok(format)
    }

    pub(crate) fn layout(&mut self, text: &str, bounds: TextBox) -> Result<IDWriteTextLayout> {
        let utf16: Vec<_> = text.encode_utf16().collect();
        ensure!(
            utf16.len() <= 4096
                && (1..=4096).contains(&bounds.pixels)
                && bounds.width > 0
                && bounds.height > 0,
            "font stage=layout bounds"
        );
        let font = &mut self.fonts[bounds.role.index()];
        if !font.missing_reported {
            let codepoints: Vec<_> = text.chars().map(u32::from).collect();
            let mut glyphs = vec![0; codepoints.len()];
            unsafe {
                font.face.GetGlyphIndices(
                    codepoints.as_ptr(),
                    codepoints.len() as u32,
                    glyphs.as_mut_ptr(),
                )
            }?;
            if glyphs.contains(&0) {
                font.missing_reported = true;
                debug!(
                    "font stage=glyph-fallback role={}; using system fallback, INI unchanged",
                    bounds.role.label()
                );
            }
        }
        let format = self.format(bounds.role, bounds.pixels)?;
        let layout = unsafe {
            self.factory.CreateTextLayout(
                &utf16,
                &format,
                bounds.width as f32,
                bounds.height as f32,
            )
        }?;
        if bounds.ellipsis {
            let ellipsis = unsafe { self.factory.CreateEllipsisTrimmingSign(&format) }?;
            unsafe {
                layout.SetTrimming(
                    &DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        ..Default::default()
                    },
                    &ellipsis,
                )
            }?;
        }
        Ok(layout)
    }
}
