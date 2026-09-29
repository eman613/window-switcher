use std::mem::size_of;

use anyhow::{ensure, Result};
use windows::Win32::{
    Graphics::Gdi::{
        CreateFontIndirectW, CreateSolidBrush, GetObjectW, CLEARTYPE_QUALITY, HBRUSH, HGDIOBJ,
        LOGFONTW,
    },
    UI::Input::KeyboardAndMouse::GetKeyNameTextW,
};

use crate::{
    appearance::Appearance,
    config::{Config, Hotkey, Theme},
    utils::gdi::{message_font, OwnedGdiObject},
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
                surface: 0xfcfdfe,
                text: 0x202a36,
                muted: 0x526173,
                selected: 0xe1eaf5,
                selected_text: 0x202a36,
                accent: 0x35608b,
                border: 0x9aa9bb,
                divider: 0xccd5df,
                hover: 0xeaf0f6,
                pressed: 0xd4e0ed,
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
    pub shortcut: String,
    pub dpi: u32,
}

impl SearchSkin {
    pub fn new(config: &Config, dpi: u32) -> Result<Self> {
        let normal = message_font(dpi)?;
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
        if !palette.high_contrast {
            font.lfFaceName = [0; 32];
            for (slot, unit) in font
                .lfFaceName
                .iter_mut()
                .zip("Microsoft YaHei UI".encode_utf16())
            {
                *slot = unit;
            }
            font.lfQuality = CLEARTYPE_QUALITY;
        }
        let make_font = |height: i32, weight: i32| -> Result<OwnedGdiObject> {
            let mut value = font;
            value.lfHeight = -height;
            value.lfWeight = weight;
            OwnedGdiObject::new(
                HGDIOBJ(unsafe { CreateFontIndirectW(&value) }.0),
                "picker-font",
            )
        };
        let title_height = (base * 15 / 14).max(base);
        let brush = OwnedGdiObject::new(
            HGDIOBJ(unsafe { CreateSolidBrush(crate::text_raster::colorref(palette.surface)) }.0),
            "picker-background",
        )?;
        Ok(Self {
            palette,
            title: make_font(title_height, 500)?,
            input: make_font(base * 18 / 14, 400)?,
            normal: make_font(base, 400)?,
            brush,
            secondary_height: base,
            row_height: px(42, dpi).max(title_height + px(18, dpi)),
            shortcut: hotkey_label(&config.search_hotkey),
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

fn hotkey_label(hotkey: &Hotkey) -> String {
    let modifier = match hotkey.get_modifier() {
        0x1d => "Ctrl",
        0x38 => "Alt",
        0x5b => "Win",
        _ => return String::new(),
    };
    let mut name = [0u16; 64];
    let scan = ((hotkey.code & 0xff) << 16) | if hotkey.code > 0xff { 1 << 24 } else { 0 };
    let length = unsafe { GetKeyNameTextW(scan as i32, &mut name) };
    if length <= 0 {
        return String::new();
    }
    format!(
        "{modifier} + {}",
        String::from_utf16_lossy(&name[..length as usize])
    )
}
