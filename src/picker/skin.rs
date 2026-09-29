use std::mem::size_of;

use anyhow::{ensure, Result};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, CreateSolidBrush, GetObjectW, CLEARTYPE_QUALITY, HBRUSH, HGDIOBJ, LOGFONTW,
};

use crate::{
    appearance::Appearance,
    config::{Config, Theme},
    utils::gdi::{message_font_with_floor, OwnedGdiObject},
};

#[derive(Clone, Copy)]
pub(super) struct SearchPalette {
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub selected: u32,
    pub selected_text: u32,
    pub accent: u32,
    pub border: u32,
    pub divider: u32,
    pub hover: u32,
    pub pressed: u32,
    pub high_contrast: bool,
}

impl SearchPalette {
    fn capture(config: &Config) -> Self {
        let appearance = Appearance::capture(config);
        if appearance.high_contrast {
            return Self {
                surface: appearance.panel.color,
                text: appearance.text,
                muted: appearance.text,
                selected: appearance.selected.color,
                selected_text: appearance.border,
                accent: appearance.text,
                border: appearance.text,
                divider: appearance.text,
                hover: appearance.panel.color,
                pressed: appearance.selected.color,
                high_contrast: true,
            };
        }
        let light = match config.theme {
            Theme::Auto => crate::utils::is_light_theme(),
            Theme::Light => true,
            Theme::Dark => false,
        };
        Self::theme(light)
    }

    fn theme(light: bool) -> Self {
        if light {
            Self {
                surface: 0xffffff,
                text: 0x000000,
                muted: 0x808080,
                selected: 0xf0f0f0,
                selected_text: 0x000000,
                accent: 0x569de5,
                border: 0xa9a9a9,
                divider: 0xd9d9d9,
                hover: 0xf5f5f5,
                pressed: 0xe6e6e6,
                high_contrast: false,
            }
        } else {
            Self {
                surface: 0x171c24,
                text: 0xedf1f7,
                muted: 0xb3c0d0,
                selected: 0x2d3b4d,
                selected_text: 0xedf1f7,
                accent: 0x9cbddf,
                border: 0x728399,
                divider: 0x455162,
                hover: 0x2b3543,
                pressed: 0x3a4b61,
                high_contrast: false,
            }
        }
    }
}

pub(super) struct SearchSkin {
    pub palette: SearchPalette,
    pub normal: OwnedGdiObject,
    pub title: OwnedGdiObject,
    pub input: OwnedGdiObject,
    pub brush: OwnedGdiObject,
    pub secondary_height: i32,
    pub row_height: i32,
    pub bold: OwnedGdiObject,
    pub help_font: OwnedGdiObject,
    pub question_font: OwnedGdiObject,
    pub match_mode: crate::config::SearchMatch,
    pub dpi: u32,
}

impl SearchSkin {
    pub fn new(config: &Config, dpi: u32) -> Result<Self> {
        let normal = message_font_with_floor(dpi, 0)?;
        let mut font = LOGFONTW::default();
        ensure!(
            unsafe {
                GetObjectW(
                    normal.0,
                    size_of::<LOGFONTW>() as i32,
                    Some((&mut font as *mut LOGFONTW).cast()),
                )
            } == size_of::<LOGFONTW>() as i32,
            "picker stage=font metrics unavailable"
        );
        let base = font.lfHeight.saturating_abs();
        let palette = SearchPalette::capture(config);
        font.lfQuality = CLEARTYPE_QUALITY;
        let make_font = |height: i32, weight: i32| -> Result<OwnedGdiObject> {
            let mut value = font;
            value.lfHeight = -height;
            value.lfWeight = weight;
            OwnedGdiObject::new(
                HGDIOBJ(unsafe { CreateFontIndirectW(&value) }.0),
                "picker-font",
            )
        };
        let title_height = base;
        let brush = OwnedGdiObject::new(
            HGDIOBJ(unsafe { CreateSolidBrush(crate::text_raster::colorref(palette.surface)) }.0),
            "picker-background",
        )?;
        Ok(Self {
            palette,
            title: make_font(title_height, 400)?,
            input: make_font(base * 15 / 12, 400)?,
            normal: make_font(base, 400)?,
            brush,
            secondary_height: base,
            row_height: px(31, dpi).max(title_height * 127 / 100 + px(12, dpi)),
            bold: make_font(base, 700)?,
            help_font: make_font(base * 10 / 12, 400)?,
            question_font: make_font(base * 18 / 12, 700)?,
            match_mode: config.search_match,
            dpi,
        })
    }

    pub fn background_brush(&self) -> HBRUSH {
        HBRUSH(self.brush.0 .0)
    }
}

pub(super) fn px(dip: i32, dpi: u32) -> i32 {
    (i64::from(dip) * i64::from(dpi) / 96) as i32
}
