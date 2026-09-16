use windows::Win32::Foundation::SIZE;

use crate::{
    config::{BadgeShape, Config},
    painter::ICON_SIZE_BASE,
};

mod gdi;
mod raster;
mod text;
pub(crate) use raster::compose;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeStyle {
    pub background: u32,
    pub foreground: u32,
    pub font_size: u32,
    pub shape: BadgeShape,
    pub size: Option<u32>,
}

impl BadgeStyle {
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            background: config.switch_apps_badge_color,
            foreground: config.switch_apps_badge_text_color,
            font_size: config.switch_apps_badge_font_size,
            shape: config.switch_apps_badge_shape,
            size: config.switch_apps_badge_size,
        }
    }
}

pub(crate) fn format_badge_count(window_count: usize, badge_max: u32) -> Option<String> {
    if window_count <= 1 {
        return None;
    }
    let badge_max = badge_max.clamp(2, 9999) as usize;
    Some(if window_count > badge_max {
        format!("{badge_max}+")
    } else {
        window_count.to_string()
    })
}

fn automatic_side(icon_size: i32, measured: SIZE, shape: BadgeShape) -> i32 {
    let padding = (icon_size / ICON_SIZE_BASE).max(1);
    text_side(measured, padding, shape).max((i64::from(icon_size) * 3 / 8) as i32)
}

fn text_side(measured: SIZE, padding: i32, shape: BadgeShape) -> i32 {
    let width = f64::from(measured.cx) + f64::from(padding) * 2.0;
    let height = f64::from(measured.cy) + f64::from(padding) * 2.0;
    match shape {
        // Include text corners, not only its width and height independently.
        BadgeShape::Circle => width.hypot(height).ceil() as i32,
        BadgeShape::Square => width.max(height).ceil() as i32,
    }
}

#[cfg(test)]
#[path = "badge_tests.rs"]
mod tests;
