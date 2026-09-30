use std::mem::size_of;

use anyhow::{ensure, Result};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, CreateSolidBrush, GetObjectW, CLEARTYPE_QUALITY, HBRUSH, HGDIOBJ, LOGFONTW,
};

use crate::{
    appearance::Appearance,
    config::Config,
    utils::gdi::{content_font, OwnedGdiObject},
};

#[derive(Clone, Copy)]
pub(super) struct SearchPalette {
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub selected: u32,
    pub selected_text: u32,
    pub selected_muted: u32,
    pub pressed_text: u32,
    pub pressed_muted: u32,
    pub accent: u32,
    pub border: u32,
    pub divider: u32,
    pub hover: u32,
    pub pressed: u32,
    pub high_contrast: bool,
}

impl SearchPalette {
    fn capture(appearance: Appearance, automatic_text: bool) -> Self {
        if appearance.high_contrast {
            return Self {
                surface: appearance.panel.color,
                text: appearance.text,
                muted: appearance.text,
                selected: appearance.selected.color,
                selected_text: appearance.border,
                selected_muted: appearance.border,
                pressed_text: appearance.border,
                pressed_muted: appearance.border,
                accent: appearance.text,
                border: appearance.text,
                divider: appearance.text,
                hover: appearance.panel.color,
                pressed: appearance.selected.color,
                high_contrast: true,
            };
        }
        let surface = appearance.panel.color;
        let selected = mix(
            appearance.selected.color,
            surface,
            appearance.selected.opacity,
        );
        let pressed = mix(appearance.border, selected, 40);
        let foreground = |background| {
            if automatic_text {
                crate::appearance::best_text(background)
            } else {
                appearance.text
            }
        };
        let selected_text = foreground(selected);
        let pressed_text = foreground(pressed);
        if crate::appearance::contrast_ratio(selected_text, selected) < 4.5
            || crate::appearance::contrast_ratio(pressed_text, pressed) < 4.5
        {
            warn!("picker stage=contrast selected-text-below-4.5; INI unchanged");
        }
        Self {
            surface,
            text: appearance.text,
            muted: readable_muted(appearance.text, surface),
            selected,
            selected_text,
            selected_muted: readable_muted(selected_text, selected),
            pressed_text,
            pressed_muted: readable_muted(pressed_text, pressed),
            accent: appearance.border,
            border: appearance.border,
            // Selection is carried by the fill; keep its outline quieter than
            // the input frame, as in Switcheroo's unfocused list selection.
            divider: mix(appearance.border, selected, 32),
            hover: mix(appearance.text, surface, 18),
            pressed,
            high_contrast: false,
        }
    }
}

fn readable_muted(foreground: u32, background: u32) -> u32 {
    let muted = mix(foreground, background, 190);
    if crate::appearance::contrast_ratio(muted, background) >= 4.5 {
        muted
    } else {
        foreground
    }
}

fn mix(foreground: u32, background: u32, alpha: u8) -> u32 {
    let alpha = u32::from(alpha);
    [0, 8, 16]
        .into_iter()
        .map(|shift| {
            ((((foreground >> shift) & 255) * alpha
                + ((background >> shift) & 255) * (255 - alpha)
                + 127)
                / 255)
                << shift
        })
        .sum()
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
    pub panel_radius: i32,
    pub selection_radius: i32,
}

impl SearchSkin {
    pub fn new(config: &Config, dpi: u32) -> Result<Self> {
        let normal = content_font(config, dpi, 0)?;
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
        let appearance = Appearance::capture(config);
        let palette = SearchPalette::capture(appearance, config.app_name_text_color.is_none());
        let radii = appearance.radii(config, dpi, px(config.icon_size as i32, dpi));
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
            panel_radius: radii[0].round() as i32,
            selection_radius: radii[2].round() as i32,
        })
    }

    pub fn background_brush(&self) -> HBRUSH {
        HBRUSH(self.brush.0 .0)
    }
}

pub(super) fn px(dip: i32, dpi: u32) -> i32 {
    (i64::from(dip) * i64::from(dpi) / 96) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_selected_and_pressed_text_remains_readable_on_inverted_and_gray_fills() {
        for background in [0xffffff, 0x171717] {
            for value in 0..=255u32 {
                for opacity in [35, 100] {
                    let config = Config {
                        background_color: Some(background),
                        selection_color: Some(value * 0x010101),
                        selection_opacity: opacity,
                        ..Default::default()
                    };
                    let palette = SearchPalette::capture(
                        Appearance::resolve(&config, false, true, None),
                        true,
                    );
                    for (foreground, background) in [
                        (palette.selected_text, palette.selected),
                        (palette.selected_muted, palette.selected),
                        (palette.pressed_text, palette.pressed),
                        (palette.pressed_muted, palette.pressed),
                    ] {
                        assert!(crate::appearance::contrast_ratio(foreground, background) >= 4.5);
                    }
                }
            }
        }
    }

    #[test]
    fn explicit_text_color_is_preserved_and_high_contrast_uses_system_text() {
        let config = Config {
            app_name_text_color: Some(0x171717),
            selection_color: Some(0x171717),
            ..Default::default()
        };
        let palette =
            SearchPalette::capture(Appearance::resolve(&config, false, true, None), false);
        assert_eq!(palette.selected_text, 0x171717);
        let palette = SearchPalette::capture(
            Appearance::resolve(&config, false, true, Some([0, 0xffffff, 0xff, 0xffffff])),
            true,
        );
        assert_eq!(palette.selected_text, 0xffffff);
        assert_eq!(palette.selected_muted, 0xffffff);
    }

    #[test]
    fn search_uses_the_panel_palette_including_custom_colors_and_contrast() {
        let config = Config {
            background_color: Some(0x233141),
            selection_color: Some(0x40516a),
            app_name_text_color: Some(0xffffff),
            ..Default::default()
        };
        for contrast in [None, Some([0, 0xffffff, 0x0000ff, 0xffffff])] {
            let appearance = Appearance::resolve(&config, false, true, contrast);
            let palette = SearchPalette::capture(appearance, config.app_name_text_color.is_none());
            assert_eq!(palette.surface, appearance.panel.color);
            assert_eq!(palette.selected, appearance.selected.color);
            assert_eq!(palette.text, appearance.text);
            assert_eq!(palette.high_contrast, appearance.high_contrast);
        }
    }
}
