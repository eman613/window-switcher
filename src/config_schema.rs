use std::str::FromStr;

use log::LevelFilter;

use crate::{
    config::{
        BackdropFallback, BackdropMode, BackgroundColor, Config, ConfigReloadMode, Hotkey,
        LayoutMode, MonitorTarget, RenderScale, BACKGROUND_OPACITY_MAX, BADGE_MAX_MAX,
        BADGE_MAX_MIN, GRID_EXTENT_MAX, ICON_CACHE_LIMIT_MAX, ICON_CACHE_LIMIT_MIN, ICON_SIZE_MAX,
        ICON_SIZE_MIN, PANEL_EXTENT_MAX, SPACING_MAX,
    },
    localization::Language,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingStatus {
    Valid,
    Invalid(&'static str),
    DeprecatedKey(&'static str),
    DeprecatedValue(&'static str),
    Unknown,
}

pub(crate) fn is_known_section(section: Option<&str>) -> bool {
    matches!(
        section,
        None | Some(
            "startup"
                | "appearance"
                | "localization"
                | "performance"
                | "switch-windows"
                | "switch-apps"
                | "log"
        )
    )
}

pub(crate) fn validate_setting(section: Option<&str>, key: &str, value: &str) -> SettingStatus {
    match (section, key) {
        (None, "config_version") => validate_parse::<u32>(value, "non-negative integer"),
        (None, "trayicon") => validate_bool(value),
        (Some("startup"), "run_as_admin") => validate_bool(value),
        (Some("appearance"), "monitor") => {
            validate_parse::<MonitorTarget>(value, "cursor, foreground, or primary")
        }
        (Some("appearance"), "use_work_area" | "show_badge") => validate_bool(value),
        (Some("appearance"), "icon_size") => validate_i32(value, ICON_SIZE_MIN, ICON_SIZE_MAX),
        (Some("appearance"), "icon_padding" | "item_gap" | "panel_padding") => {
            validate_i32(value, 0, SPACING_MAX)
        }
        (Some("appearance"), "max_width" | "max_height") => {
            validate_i32(value, 0, PANEL_EXTENT_MAX)
        }
        (Some("appearance"), "layout") => {
            validate_parse::<LayoutMode>(value, "single-row, grid, or paged")
        }
        (Some("appearance"), "max_columns" | "max_rows") => {
            validate_usize(value, 0, GRID_EXTENT_MAX)
        }
        (Some("appearance"), "background_color") => {
            validate_parse::<BackgroundColor>(value, "auto or #RRGGBB")
        }
        (Some("appearance"), "background_opacity") => validate_u8(value, 0, BACKGROUND_OPACITY_MAX),
        (Some("appearance"), "backdrop") => {
            validate_parse::<BackdropMode>(value, "none, alpha, blur, acrylic, mica, or auto")
        }
        (Some("appearance"), "backdrop_fallback") => {
            if value.trim().eq_ignore_ascii_case("none") {
                SettingStatus::DeprecatedValue("solid")
            } else {
                validate_parse::<BackdropFallback>(value, "alpha or solid")
            }
        }
        (Some("appearance"), "badge_max") => validate_usize(value, BADGE_MAX_MIN, BADGE_MAX_MAX),
        (Some("localization"), "language") => {
            validate_parse::<Language>(value, "auto, zh-CN, or en-US")
        }
        (Some("performance"), "icon_cache_limit") => {
            validate_usize(value, ICON_CACHE_LIMIT_MIN, ICON_CACHE_LIMIT_MAX)
        }
        (Some("performance"), "render_scale") => {
            validate_parse::<RenderScale>(value, "auto, 1, 2, 4, or 6")
        }
        (Some("performance"), "config_reload") => {
            validate_parse::<ConfigReloadMode>(value, "restart, on-open, or watch")
        }
        (Some("switch-windows"), "hotkey") | (Some("switch-apps"), "hotkey") => {
            validate_hotkeys(value)
        }
        (Some("switch-windows"), "blacklist") => SettingStatus::Valid,
        (Some("switch-windows"), "ignore_minimal")
        | (Some("switch-apps"), "enable" | "ignore_minimal") => validate_bool(value),
        (Some("switch-windows" | "switch-apps"), "only_current_desktop") => {
            validate_auto_bool(value)
        }
        (Some("switch-apps"), "override_icons") => validate_override_icons(value),
        (Some("log"), "level") => {
            validate_parse::<LevelFilter>(value, "off, error, warn, info, debug, or trace")
        }
        (Some("log"), "path") => SettingStatus::Valid,
        (Some("log"), "file") => SettingStatus::DeprecatedKey("path"),
        _ => SettingStatus::Unknown,
    }
}

fn validate_bool(value: &str) -> SettingStatus {
    if Config::to_bool(value.trim()).is_some() {
        SettingStatus::Valid
    } else {
        SettingStatus::Invalid("yes/no, true/false, on/off, or 1/0")
    }
}

fn validate_auto_bool(value: &str) -> SettingStatus {
    if value.trim().eq_ignore_ascii_case("auto") {
        SettingStatus::Valid
    } else {
        validate_bool(value)
    }
}

fn validate_hotkeys(value: &str) -> SettingStatus {
    let valid = value
        .split("||")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .all(|part| Hotkey::parse(part).is_some());
    if valid && value.split("||").any(|part| !part.trim().is_empty()) {
        SettingStatus::Valid
    } else {
        SettingStatus::Invalid("one or more supported hotkeys separated by ||")
    }
}

fn validate_override_icons(value: &str) -> SettingStatus {
    let valid = value.split([',', ';']).all(|mapping| {
        let mapping = mapping.trim();
        if mapping.is_empty() {
            return true;
        }
        mapping
            .split_once('=')
            .map(|(key, path)| !key.trim().is_empty() && !path.trim().is_empty())
            .unwrap_or(false)
    });
    if valid {
        SettingStatus::Valid
    } else {
        SettingStatus::Invalid("app.exe=icon_path entries separated by comma or semicolon")
    }
}

fn validate_parse<T: FromStr>(value: &str, expected: &'static str) -> SettingStatus {
    if value.trim().parse::<T>().is_ok() {
        SettingStatus::Valid
    } else {
        SettingStatus::Invalid(expected)
    }
}

fn validate_i32(value: &str, min: i32, max: i32) -> SettingStatus {
    match value.trim().parse::<i32>() {
        Ok(parsed) if (min..=max).contains(&parsed) => SettingStatus::Valid,
        _ => SettingStatus::Invalid("integer outside the supported range"),
    }
}

fn validate_u8(value: &str, min: u8, max: u8) -> SettingStatus {
    match value.trim().parse::<u8>() {
        Ok(parsed) if (min..=max).contains(&parsed) => SettingStatus::Valid,
        _ => SettingStatus::Invalid("integer outside the supported range"),
    }
}

fn validate_usize(value: &str, min: usize, max: usize) -> SettingStatus {
    match value.trim().parse::<usize>() {
        Ok(parsed) if (min..=max).contains(&parsed) => SettingStatus::Valid,
        _ => SettingStatus::Invalid("integer outside the supported range"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_validates_localization_and_performance_values() {
        assert_eq!(
            validate_setting(Some("localization"), "language", "zh-CN"),
            SettingStatus::Valid
        );
        assert!(matches!(
            validate_setting(Some("performance"), "icon_cache_limit", "8"),
            SettingStatus::Invalid(_)
        ));
        assert_eq!(
            validate_setting(Some("performance"), "render_scale", "4"),
            SettingStatus::Valid
        );
    }

    #[test]
    fn schema_recognizes_deprecated_and_unknown_settings() {
        assert_eq!(
            validate_setting(Some("log"), "file", "switcher.log"),
            SettingStatus::DeprecatedKey("path")
        );
        assert_eq!(
            validate_setting(Some("appearance"), "backdrop_fallback", "none"),
            SettingStatus::DeprecatedValue("solid")
        );
        assert_eq!(
            validate_setting(Some("appearance"), "mystery", "1"),
            SettingStatus::Unknown
        );
    }
}
