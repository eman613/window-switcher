//! One immutable layout is used by drawing, pagination and hit testing.
use anyhow::{bail, ensure, Result};

use crate::config::Config;

mod monitor;
pub(crate) use monitor::{enable_per_monitor, window_monitor_dpi, MonitorSnapshot};

#[cfg(test)]
mod tests;

pub(crate) const MIN_ICON_DIP: u32 = 24;
pub(crate) const MAX_ICON_DIP: u32 = 256;
pub(crate) const MAX_WINDOWS: usize = 4096;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

impl PixelRect {
    pub(crate) fn width(self) -> i32 {
        self.right.saturating_sub(self.left)
    }
    pub(crate) fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }
    pub(crate) fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutOptions {
    pub(crate) panel_width: u32,
    pub(crate) panel_height: u32,
    pub(crate) icon_size: u32,
    pub(crate) icon_padding: u32,
    pub(crate) item_gap: u32,
    pub(crate) panel_padding: u32,
    pub(crate) max_width: u32,
    pub(crate) max_height: u32,
    pub(crate) max_columns: u32,
}

impl LayoutOptions {
    pub(crate) fn from_config(config: &Config) -> Self {
        Self {
            panel_width: config.panel_width,
            panel_height: config.panel_height,
            icon_size: config.icon_size,
            icon_padding: config.icon_padding,
            item_gap: config.item_gap,
            panel_padding: config.panel_padding,
            max_width: config.max_width,
            max_height: config.max_height,
            max_columns: config.max_columns,
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let minimum = MIN_ICON_DIP + 2 * (self.icon_padding + self.panel_padding);
        for (name, value) in [
            ("panel_width", self.panel_width),
            ("panel_height", self.panel_height),
            ("max_width", self.max_width),
            ("max_height", self.max_height),
        ] {
            if value != 0 && value < minimum {
                bail!("[appearance] {name} 与内边距冲突；至少需要 {minimum} DIP，原值未修改");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemRect {
    pub(crate) index: usize,
    pub(crate) outer: PixelRect,
    pub(crate) icon: PixelRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutSnapshot {
    pub(crate) monitor: MonitorSnapshot,
    pub(crate) bounds: PixelRect,
    pub(crate) icon_size: i32,
    pub(crate) capacity: usize,
    pub(crate) page: usize,
    pub(crate) items: Vec<ItemRect>,
    pub(crate) footer: Option<PixelRect>,
}

impl LayoutSnapshot {
    pub(crate) fn calculate(
        options: &LayoutOptions,
        monitor: MonitorSnapshot,
        count: usize,
        selected: usize,
        name_height: u32,
    ) -> Result<Self> {
        options.validate()?;
        ensure!(
            count != 0 && count <= MAX_WINDOWS && selected < count,
            "layout stage=count invalid"
        );
        ensure!(
            (48..=960).contains(&monitor.dpi),
            "layout stage=dpi invalid"
        );
        let scale = f64::from(monitor.dpi) / 96.0;
        let area = monitor.available;
        ensure!(
            area.width() > 0 && area.height() > 0,
            "layout stage=monitor unavailable"
        );
        let limit = |pixels: i32, maximum: u32| {
            let available = f64::from(pixels) / scale;
            if maximum == 0 {
                available
            } else {
                available.min(f64::from(maximum))
            }
        };
        let width_limit = limit(area.width(), options.max_width);
        let height_limit = limit(area.height(), options.max_height);
        let target = |requested: u32, maximum: f64| {
            if requested == 0 {
                maximum
            } else {
                f64::from(requested).min(maximum)
            }
        };
        let width = target(options.panel_width, width_limit);
        let height = target(options.panel_height, height_limit);
        let panel_padding = f64::from(options.panel_padding);
        let icon_padding = f64::from(options.icon_padding);
        let gap = f64::from(options.item_gap);
        let inner_width = width - 2.0 * panel_padding;
        let inner_height = height - 2.0 * panel_padding - f64::from(name_height);
        let height_fit = inner_height - 2.0 * icon_padding;
        ensure!(
            height_fit >= f64::from(MIN_ICON_DIP),
            "layout stage=height insufficient space"
        );
        let explicit = options.panel_width != 0 || options.panel_height != 0;
        let preferred = if explicit {
            f64::from(MIN_ICON_DIP)
        } else {
            f64::from(options.icon_size)
                .min(height_fit)
                .min(inner_width - 2.0 * icon_padding)
        };
        let slots = ((inner_width + gap) / (preferred + 2.0 * icon_padding + gap)).floor();
        ensure!(slots >= 1.0, "layout stage=width insufficient space");
        let column_limit = if options.max_columns == 0 {
            MAX_WINDOWS
        } else {
            options.max_columns as usize
        };
        let capacity = (slots as usize).min(column_limit).min(count);
        let width_fit =
            (inner_width - (capacity - 1) as f64 * gap) / capacity as f64 - 2.0 * icon_padding;
        let natural_limit = if explicit {
            MAX_ICON_DIP
        } else {
            options.icon_size
        };
        let icon = width_fit
            .min(height_fit)
            .min(f64::from(natural_limit))
            .floor();
        ensure!(
            icon >= f64::from(MIN_ICON_DIP),
            "layout stage=fit insufficient space"
        );
        let item = icon + 2.0 * icon_padding;
        let row_width = capacity as f64 * item + (capacity - 1) as f64 * gap;
        let final_width = if options.panel_width == 0 {
            row_width + 2.0 * panel_padding
        } else {
            width
        };
        let final_height = if options.panel_height == 0 {
            item + 2.0 * panel_padding + f64::from(name_height)
        } else {
            height
        };
        let px = |dip: f64| (dip * scale).round() as i32;
        let physical_width = px(final_width).min(area.width());
        let physical_height = px(final_height).min(area.height());
        ensure!(
            physical_width > 0 && physical_height > 0,
            "layout stage=pixels invalid"
        );
        let left = area
            .left
            .saturating_add((area.width() - physical_width) / 2);
        let top = area
            .top
            .saturating_add((area.height() - physical_height) / 2);
        let bounds = PixelRect {
            left,
            top,
            right: left.saturating_add(physical_width),
            bottom: top.saturating_add(physical_height),
        };
        let page = selected / capacity;
        let first = page * capacity;
        // Keep the full page geometry, including on the last, partial page.
        let row_left = (final_width - row_width) / 2.0;
        let row_top = (final_height - f64::from(name_height) - item) / 2.0;
        let items = (first..(first + capacity).min(count))
            .map(|index| {
                let x = row_left + (index - first) as f64 * (item + gap);
                ItemRect {
                    index,
                    outer: PixelRect {
                        left: px(x),
                        top: px(row_top),
                        right: px(x + item),
                        bottom: px(row_top + item),
                    },
                    icon: PixelRect {
                        left: px(x + icon_padding),
                        top: px(row_top + icon_padding),
                        right: px(x + icon_padding + icon),
                        bottom: px(row_top + icon_padding + icon),
                    },
                }
            })
            .collect();
        Ok(Self {
            monitor,
            bounds,
            icon_size: px(icon),
            capacity,
            page,
            items,
            footer: (name_height != 0).then(|| PixelRect {
                left: px(panel_padding),
                right: px(final_width - panel_padding),
                top: px(final_height - panel_padding - f64::from(name_height)),
                bottom: px(final_height - panel_padding),
            }),
        })
    }

    pub(crate) fn hit_test(&self, screen_x: i32, screen_y: i32) -> Option<usize> {
        let x = screen_x.checked_sub(self.bounds.left)?;
        let y = screen_y.checked_sub(self.bounds.top)?;
        self.items
            .iter()
            .find(|item| item.outer.contains(x, y))
            .map(|item| item.index)
    }
}
