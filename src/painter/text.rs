//! Text uses its own alpha surface; background opacity never multiplies glyphs.
use crate::{
    config::Config,
    font_resources::{FontResources, FontRole, TextBox},
    pixels::PixelImage,
    render_surface::RenderSurface,
    text_raster::{colorref, rasterize},
    utils::gdi::{OwnedGdiObject, SavedDc},
};
use anyhow::{bail, Result};
use windows::{
    core::w,
    Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            CreateFontW, DrawTextW, SetBkMode, SetTextColor, ANTIALIASED_QUALITY,
            CLIP_DEFAULT_PRECIS, CLR_INVALID, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER,
            DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, OUT_DEFAULT_PRECIS,
            TRANSPARENT,
        },
    },
};

pub(super) fn name(
    fonts: Option<&mut FontResources>,
    config: &Config,
    text: &str,
    bounds: TextBox,
    color: u32,
) -> Result<PixelImage> {
    if let Some(fonts) = fonts {
        let layout = fonts.layout(text, bounds)?;
        return rasterize(&fonts.factory, &layout, bounds.width, bounds.height, color);
    }
    let font = unsafe {
        CreateFontW(
            -(bounds.pixels as i32),
            0,
            0,
            0,
            config.app_name_font_weight as i32,
            u32::from(config.app_name_font_italic),
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        )
    };
    let font = OwnedGdiObject::new(font.into(), "name-font-fallback")?;
    let mut black = RenderSurface::new(bounds.width, bounds.height)?;
    let mut white = RenderSurface::new(bounds.width, bounds.height)?;
    let mut text: Vec<_> = text.encode_utf16().collect();
    for (surface, rgb) in [(&mut black, 0), (&mut white, 0xffffff)] {
        surface.fill_rgb(rgb)?;
        let saved = SavedDc::new(surface.dc())?;
        saved.select(font.0)?;
        let mut rect = RECT {
            right: bounds.width,
            bottom: bounds.height,
            ..Default::default()
        };
        unsafe {
            if SetBkMode(surface.dc(), TRANSPARENT) == 0
                || SetTextColor(surface.dc(), colorref(color)).0 == CLR_INVALID
            {
                bail!("font stage=name-fallback color");
            }
            if !text.is_empty()
                && DrawTextW(
                    surface.dc(),
                    &mut text,
                    &mut rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                ) == 0
            {
                bail!("font stage=name-fallback draw");
            }
        }
    }
    PixelImage::recover_alpha(
        black.pixels()?,
        white.pixels()?,
        bounds.width,
        bounds.height,
    )
}

pub(super) fn name_height(config: &Config, fonts: Option<&FontResources>) -> u32 {
    let base = crate::layout::LayoutOptions::from_config(config).name_height;
    if base == 0 {
        return 0;
    }
    fonts.map_or(base, |fonts| {
        base.max(fonts.line_height(FontRole::Name, config.app_name_font_size) + 6)
    })
}
