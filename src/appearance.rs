//! Resolved paint values; explicit INI values are never rewritten by theme policy.
use crate::{
    badge::BadgeStyle,
    config::{AppNameMode, Config, Theme},
    utils,
};
use windows::Win32::{
    Graphics::Gdi::{
        GetSysColor, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    },
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        WindowsAndMessaging::{
            SystemParametersInfoW, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        },
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fill {
    pub(crate) color: u32,
    pub(crate) opacity: u8,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Appearance {
    pub(crate) panel: Fill,
    pub(crate) plain: Fill,
    pub(crate) selected: Fill,
    pub(crate) border: u32,
    pub(crate) border_width: u32,
    pub(crate) text: u32,
    pub(crate) rounded: bool,
    pub(crate) high_contrast: bool,
}

impl Appearance {
    pub(crate) fn capture(config: &Config) -> Self {
        let mut contrast = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        let high_contrast = match unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                Some((&mut contrast as *mut HIGHCONTRASTW).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        } {
            Ok(()) => contrast.dwFlags & HCF_HIGHCONTRASTON != Default::default(),
            Err(error) => {
                warn!(
                    "appearance stage=high-contrast-query code={:#x}",
                    error.code().0
                );
                false
            }
        };
        let system_colors = unsafe {
            [
                GetSysColor(COLOR_WINDOW),
                GetSysColor(COLOR_WINDOWTEXT),
                GetSysColor(COLOR_HIGHLIGHT),
                GetSysColor(COLOR_HIGHLIGHTTEXT),
            ]
        }
        .map(|value| crate::text_raster::colorref(value).0);
        let result = Self::resolve(
            config,
            utils::is_light_theme(),
            utils::is_win11(),
            high_contrast.then_some(system_colors),
        );
        result.diagnose(config);
        result
    }

    pub(crate) fn resolve(
        config: &Config,
        system_light: bool,
        rounded: bool,
        high_contrast: Option<[u32; 4]>,
    ) -> Self {
        if let Some([background, text, selection, selection_text]) = high_contrast {
            return Self {
                panel: Fill {
                    color: background,
                    opacity: 255,
                },
                plain: Fill {
                    color: background,
                    opacity: 255,
                },
                selected: Fill {
                    color: selection,
                    opacity: 255,
                },
                border: selection_text,
                border_width: config.selection_border_width.max(2),
                text,
                rounded: false,
                high_contrast: true,
            };
        }
        let light = match config.theme {
            Theme::Auto => system_light,
            Theme::Light => true,
            Theme::Dark => false,
        };
        let background = config
            .background_color
            .unwrap_or(if light { 0xe0e0e0 } else { 0x4c4c4c });
        let selected = config
            .selection_color
            .unwrap_or(if light { 0xf2f2f2 } else { 0x3b3b3b });
        let mut border = if light { 0x4c7094 } else { 0xb7cfdf };
        if contrast_ratio(border, background).min(contrast_ratio(border, selected)) < 3.0 {
            border = best_text(background);
        }
        let opacity = |percent: u32| ((percent.min(100) * 255 + 50) / 100) as u8;
        Self {
            panel: Fill {
                color: background,
                opacity: opacity(config.background_opacity),
            },
            plain: Fill {
                color: config.icon_background_color.unwrap_or(background),
                opacity: opacity(config.icon_background_opacity),
            },
            selected: Fill {
                color: selected,
                opacity: opacity(config.selection_opacity),
            },
            border: config.selection_border_color.unwrap_or(border),
            border_width: config.selection_border_width,
            text: config
                .app_name_text_color
                .unwrap_or_else(|| best_text(background)),
            rounded,
            high_contrast: false,
        }
    }

    pub(crate) fn radii(&self, config: &Config, dpi: u32, item_size: i32) -> [f32; 3] {
        if self.high_contrast {
            return [0.0; 3];
        }
        let automatic = if self.rounded {
            item_size as f32 / 8.0
        } else {
            0.0
        };
        let physical =
            |value: Option<u32>| value.map_or(automatic, |value| value as f32 * dpi as f32 / 96.0);
        let icon = physical(config.icon_corner_radius);
        [
            physical(config.panel_corner_radius),
            icon,
            config
                .selection_corner_radius
                .map_or(icon, |value| value as f32 * dpi as f32 / 96.0),
        ]
    }

    pub(crate) fn badge(&self, mut style: BadgeStyle) -> BadgeStyle {
        if self.high_contrast {
            style.background = self.selected.color;
            style.foreground = self.border;
        }
        style
    }

    fn diagnose(&self, config: &Config) {
        if self.high_contrast {
            return;
        }
        if self.panel.opacity < 255 {
            warn!("appearance stage=contrast translucent-background; contrast depends on desktop, INI unchanged");
        }
        if config.app_name_mode != AppNameMode::Off
            && contrast_ratio(self.text, self.panel.color) < 4.5
        {
            warn!("appearance stage=contrast app-name-below-4.5; INI unchanged");
        }
        if contrast_ratio(self.border, self.panel.color)
            .min(contrast_ratio(self.border, self.selected.color))
            < 3.0
        {
            warn!("appearance stage=contrast selection-border-below-3; INI unchanged");
        }
    }
}

fn best_text(background: u32) -> u32 {
    if contrast_ratio(0x171717, background) >= contrast_ratio(0xffffff, background) {
        0x171717
    } else {
        0xffffff
    }
}

pub(crate) fn contrast_ratio(first: u32, second: u32) -> f64 {
    fn luminance(rgb: u32) -> f64 {
        let channel = |shift| {
            let value = f64::from((rgb >> shift) & 255u32) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    let first = luminance(first);
    let second = luminance(second);
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_have_readable_names_and_selection_in_both_themes() {
        for light in [false, true] {
            let style = Appearance::resolve(&Config::default(), light, true, None);
            assert!(contrast_ratio(style.text, style.panel.color) >= 4.5);
            assert!(contrast_ratio(style.border, style.panel.color) >= 3.0);
            assert!(contrast_ratio(style.border, style.selected.color) >= 3.0);
            assert_eq!(
                (
                    style.panel.opacity,
                    style.plain.opacity,
                    style.selected.opacity
                ),
                (255, 0, 255)
            );
        }
    }
    #[test]
    fn three_fills_radii_and_high_contrast_are_independent_of_persisted_values() {
        let config = Config {
            background_color: Some(0x112233),
            background_opacity: 0,
            icon_background_color: Some(0x445566),
            icon_background_opacity: 50,
            selection_color: Some(0x778899),
            selection_opacity: 1,
            panel_corner_radius: Some(80),
            icon_corner_radius: Some(7),
            selection_corner_radius: None,
            ..Default::default()
        };
        let style = Appearance::resolve(&config, true, true, None);
        assert_eq!(
            (
                style.panel.opacity,
                style.plain.opacity,
                style.selected.opacity
            ),
            (0, 128, 3)
        );
        assert_eq!(style.radii(&config, 192, 100), [160.0, 14.0, 14.0]);
        let contrast =
            Appearance::resolve(&config, true, true, Some([0, 0xffffff, 0x123456, 0xffffff]));
        assert_eq!(
            contrast.panel,
            Fill {
                color: 0,
                opacity: 255
            }
        );
        assert_eq!(contrast.radii(&config, 192, 100), [0.0; 3]);
        assert_eq!(config.background_color, Some(0x112233));
        assert_eq!(config.background_opacity, 0);
    }
}
