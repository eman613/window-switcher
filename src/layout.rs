use crate::{
    config::{AppearanceConfig, LayoutMode},
    utils::MonitorContext,
};

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{POINT, RECT};

const BASE_DPI: i64 = 96;
const MIN_ICON_SIZE_DIP: i32 = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayoutItem {
    pub(crate) app_index: usize,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) hit_rect: RECT,
}

#[derive(Debug, Clone)]
pub(crate) struct LayoutSnapshot {
    pub(crate) app_count: usize,
    pub(crate) monitor: MonitorContext,
    pub(crate) panel_rect: RECT,
    pub(crate) content_width: i32,
    pub(crate) content_height: i32,
    pub(crate) icon_size: i32,
    pub(crate) icon_padding: i32,
    pub(crate) item_size: i32,
    pub(crate) panel_padding: i32,
    pub(crate) items: Vec<LayoutItem>,
}

impl LayoutSnapshot {
    pub(crate) fn new(
        app_count: usize,
        selected_index: usize,
        appearance: &AppearanceConfig,
        monitor: MonitorContext,
    ) -> Result<Self> {
        if app_count == 0 {
            return Err(anyhow!("Cannot lay out an empty app list"));
        }
        if selected_index >= app_count {
            return Err(anyhow!("Selected app index is out of range"));
        }

        let monitor_width = positive_extent(monitor.rect.left, monitor.rect.right, "width")?;
        let monitor_height = positive_extent(monitor.rect.top, monitor.rect.bottom, "height")?;
        let dpi = monitor.dpi.max(1);
        let requested_icon_size = dip_to_px(appearance.icon_size, dpi)?;
        let minimum_icon_size = dip_to_px(MIN_ICON_SIZE_DIP, dpi)?;
        let icon_padding = dip_to_px(appearance.icon_padding, dpi)?;
        let item_gap = dip_to_px(appearance.item_gap, dpi)?;
        let panel_padding = dip_to_px(appearance.panel_padding, dpi)?;

        let minimum_item_size = checked_add(
            minimum_icon_size,
            checked_mul(icon_padding, 2, "minimum icon padding")?,
            "minimum item size",
        )?;
        let minimum_panel_size = checked_add(
            minimum_item_size,
            checked_mul(panel_padding, 2, "minimum panel padding")?,
            "minimum panel size",
        )?;
        if monitor_width < minimum_panel_size || monitor_height < minimum_panel_size {
            return Err(anyhow!(
                "Monitor work area is too small for the minimum icon size"
            ));
        }

        let max_panel_width =
            configured_extent(monitor_width, appearance.max_width, dpi, minimum_panel_size)?;
        let max_panel_height = configured_extent(
            monitor_height,
            appearance.max_height,
            dpi,
            minimum_panel_size,
        )?;
        let available_width = max_panel_width - panel_padding * 2;
        let available_height = max_panel_height - panel_padding * 2;

        let icon_size = match appearance.layout {
            LayoutMode::SingleRow => {
                let target_columns = configured_count_limit(app_count, appearance.max_columns);
                let fit =
                    icon_size_for_columns(available_width, target_columns, icon_padding, item_gap)?;
                let height_limit = available_height - icon_padding * 2;
                requested_icon_size
                    .min(fit.max(minimum_icon_size))
                    .min(height_limit)
                    .max(minimum_icon_size)
            }
            LayoutMode::Grid | LayoutMode::Paged => {
                let width_limit = available_width - icon_padding * 2;
                let height_limit = available_height - icon_padding * 2;
                requested_icon_size
                    .min(width_limit)
                    .min(height_limit)
                    .max(minimum_icon_size)
            }
        };
        let item_size = checked_add(
            icon_size,
            checked_mul(icon_padding, 2, "icon padding")?,
            "item size",
        )?;

        let columns_by_space = capacity_for_axis(available_width, item_size, item_gap)?;
        let rows_by_space = capacity_for_axis(available_height, item_size, item_gap)?;
        let columns = apply_count_limit(columns_by_space, appearance.max_columns);
        let rows = match appearance.layout {
            LayoutMode::SingleRow => 1,
            LayoutMode::Grid => apply_count_limit(rows_by_space, appearance.max_rows),
            LayoutMode::Paged => {
                let requested_rows = if appearance.max_rows == 0 {
                    1
                } else {
                    appearance.max_rows
                };
                rows_by_space.min(requested_rows).max(1)
            }
        };
        let page_capacity = columns
            .checked_mul(rows)
            .ok_or_else(|| anyhow!("Layout page capacity overflow"))?
            .max(1);
        let page_start = selected_index / page_capacity * page_capacity;
        let visible_count = page_capacity.min(app_count - page_start);

        let reserved_count = if app_count > page_capacity {
            page_capacity
        } else {
            app_count
        };
        let panel_columns = columns.min(reserved_count).max(1);
        let panel_rows = reserved_count.div_ceil(panel_columns).min(rows).max(1);
        let content_width = grid_extent(panel_columns, item_size, item_gap, "content width")?;
        let content_height = grid_extent(panel_rows, item_size, item_gap, "content height")?;
        let panel_width = checked_add(
            content_width,
            checked_mul(panel_padding, 2, "horizontal panel padding")?,
            "panel width",
        )?;
        let panel_height = checked_add(
            content_height,
            checked_mul(panel_padding, 2, "vertical panel padding")?,
            "panel height",
        )?;
        if panel_width > max_panel_width || panel_height > max_panel_height {
            return Err(anyhow!("Calculated panel exceeds the configured bounds"));
        }

        let panel_left = monitor.rect.left + (monitor_width - panel_width) / 2;
        let panel_top = monitor.rect.top + (monitor_height - panel_height) / 2;
        let panel_rect = RECT {
            left: panel_left,
            top: panel_top,
            right: panel_left + panel_width,
            bottom: panel_top + panel_height,
        };

        let mut items = Vec::with_capacity(visible_count);
        for offset in 0..visible_count {
            let column = offset % panel_columns;
            let row = offset / panel_columns;
            let x = checked_axis_offset(column, item_size, item_gap)?;
            let y = checked_axis_offset(row, item_size, item_gap)?;
            let left = panel_left + panel_padding + x;
            let top = panel_top + panel_padding + y;
            items.push(LayoutItem {
                app_index: page_start + offset,
                x,
                y,
                hit_rect: RECT {
                    left,
                    top,
                    right: left + item_size,
                    bottom: top + item_size,
                },
            });
        }

        Ok(Self {
            app_count,
            monitor,
            panel_rect,
            content_width,
            content_height,
            icon_size,
            icon_padding,
            item_size,
            panel_padding,
            items,
        })
    }

    pub(crate) fn panel_width(&self) -> i32 {
        self.panel_rect.right - self.panel_rect.left
    }

    pub(crate) fn panel_height(&self) -> i32 {
        self.panel_rect.bottom - self.panel_rect.top
    }

    pub(crate) fn hit_test(&self, point: POINT) -> Option<usize> {
        self.items
            .iter()
            .find(|item| {
                point.x >= item.hit_rect.left
                    && point.x < item.hit_rect.right
                    && point.y >= item.hit_rect.top
                    && point.y < item.hit_rect.bottom
            })
            .map(|item| item.app_index)
    }
}

fn configured_extent(
    monitor_extent: i32,
    configured_dip: i32,
    dpi: u32,
    minimum: i32,
) -> Result<i32> {
    if configured_dip == 0 {
        return Ok(monitor_extent);
    }
    Ok(dip_to_px(configured_dip, dpi)?
        .max(minimum)
        .min(monitor_extent))
}

fn configured_count_limit(app_count: usize, configured: usize) -> usize {
    if configured == 0 {
        app_count
    } else {
        app_count.min(configured)
    }
    .max(1)
}

fn apply_count_limit(available: usize, configured: usize) -> usize {
    if configured == 0 {
        available
    } else {
        available.min(configured)
    }
    .max(1)
}

fn capacity_for_axis(available: i32, item_size: i32, gap: i32) -> Result<usize> {
    if available <= 0 || item_size <= 0 || gap < 0 {
        return Err(anyhow!("Invalid layout axis dimensions"));
    }
    let capacity =
        (i64::from(available) + i64::from(gap)) / (i64::from(item_size) + i64::from(gap));
    usize::try_from(capacity.max(1)).map_err(|_| anyhow!("Layout axis capacity overflow"))
}

fn icon_size_for_columns(
    available: i32,
    columns: usize,
    icon_padding: i32,
    gap: i32,
) -> Result<i32> {
    let columns = i64::try_from(columns).map_err(|_| anyhow!("Column count overflow"))?;
    let gaps = columns.saturating_sub(1) * i64::from(gap);
    let icon_size = (i64::from(available) - gaps) / columns - i64::from(icon_padding) * 2;
    i32::try_from(icon_size).map_err(|_| anyhow!("Calculated icon size overflow"))
}

fn grid_extent(count: usize, item_size: i32, gap: i32, name: &str) -> Result<i32> {
    let count = i64::try_from(count).map_err(|_| anyhow!("{name} count overflow"))?;
    let value = count * i64::from(item_size) + count.saturating_sub(1) * i64::from(gap);
    i32::try_from(value).map_err(|_| anyhow!("{name} overflow"))
}

fn checked_axis_offset(index: usize, item_size: i32, gap: i32) -> Result<i32> {
    let index = i64::try_from(index).map_err(|_| anyhow!("Layout index overflow"))?;
    let value = index * (i64::from(item_size) + i64::from(gap));
    i32::try_from(value).map_err(|_| anyhow!("Layout offset overflow"))
}

fn dip_to_px(value: i32, dpi: u32) -> Result<i32> {
    let pixels = (i64::from(value) * i64::from(dpi) + BASE_DPI / 2) / BASE_DPI;
    i32::try_from(pixels).map_err(|_| anyhow!("DIP conversion overflow"))
}

fn positive_extent(start: i32, end: i32, name: &str) -> Result<i32> {
    end.checked_sub(start)
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("Monitor {name} is invalid"))
}

fn checked_add(left: i32, right: i32, name: &str) -> Result<i32> {
    left.checked_add(right)
        .ok_or_else(|| anyhow!("{name} overflow"))
}

fn checked_mul(value: i32, multiplier: i32, name: &str) -> Result<i32> {
    value
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow!("{name} overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LayoutMode, MonitorTarget};
    use windows::Win32::Graphics::Gdi::HMONITOR;

    fn monitor(rect: RECT, dpi: u32) -> MonitorContext {
        MonitorContext {
            handle: HMONITOR::default(),
            rect,
            dpi,
        }
    }

    #[test]
    fn single_row_uses_dip_spacing_and_centers_panel() {
        let appearance = AppearanceConfig::default();
        let layout = LayoutSnapshot::new(
            4,
            1,
            &appearance,
            monitor(
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                96,
            ),
        )
        .unwrap();

        assert_eq!(layout.icon_size, 64);
        assert_eq!(layout.item_size, 72);
        assert_eq!(layout.content_width, 312);
        assert_eq!(layout.panel_width(), 332);
        assert_eq!(layout.panel_height(), 92);
        assert_eq!(layout.panel_rect.left, 794);
        assert_eq!(layout.panel_rect.top, 494);
        assert_eq!(layout.items.len(), 4);
    }

    #[test]
    fn single_row_pages_without_shrinking_below_minimum() {
        let appearance = AppearanceConfig::default();
        let layout = LayoutSnapshot::new(
            10,
            8,
            &appearance,
            monitor(
                RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 500,
                },
                96,
            ),
        )
        .unwrap();

        assert_eq!(layout.icon_size, MIN_ICON_SIZE_DIP);
        assert_eq!(layout.items.first().map(|item| item.app_index), Some(7));
        assert_eq!(layout.items.last().map(|item| item.app_index), Some(9));
        assert_eq!(layout.panel_width(), 292);
    }

    #[test]
    fn grid_wraps_and_keeps_negative_monitor_coordinates() {
        let appearance = AppearanceConfig {
            layout: LayoutMode::Grid,
            ..AppearanceConfig::default()
        };
        let layout = LayoutSnapshot::new(
            5,
            4,
            &appearance,
            monitor(
                RECT {
                    left: -300,
                    top: -250,
                    right: 0,
                    bottom: 0,
                },
                96,
            ),
        )
        .unwrap();

        assert_eq!(layout.items.len(), 5);
        assert_eq!(layout.content_width, 232);
        assert_eq!(layout.content_height, 152);
        assert!(layout.panel_rect.left < 0);
        assert!(layout.panel_rect.top < 0);
        assert_eq!(layout.items[3].y, 80);
    }

    #[test]
    fn paged_layout_defaults_to_one_row_and_honors_limits() {
        let appearance = AppearanceConfig {
            monitor: MonitorTarget::Primary,
            layout: LayoutMode::Paged,
            max_columns: 3,
            max_rows: 2,
            ..AppearanceConfig::default()
        };
        let layout = LayoutSnapshot::new(
            14,
            7,
            &appearance,
            monitor(
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                144,
            ),
        )
        .unwrap();

        assert_eq!(layout.items.len(), 6);
        assert_eq!(layout.items[0].app_index, 6);
        assert_eq!(layout.items[5].app_index, 11);
        assert_eq!(layout.icon_size, 96);
    }

    #[test]
    fn hit_test_returns_global_app_index() {
        let layout = LayoutSnapshot::new(
            10,
            8,
            &AppearanceConfig::default(),
            monitor(
                RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 500,
                },
                96,
            ),
        )
        .unwrap();
        let item = layout.items[1];

        assert_eq!(
            layout.hit_test(POINT {
                x: item.hit_rect.left + 1,
                y: item.hit_rect.top + 1,
            }),
            Some(8)
        );
        assert_eq!(
            layout.hit_test(POINT {
                x: layout.panel_rect.left,
                y: layout.panel_rect.top,
            }),
            None
        );
    }

    #[test]
    fn handles_empty_and_large_app_count_boundaries() {
        let monitor = monitor(
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            96,
        );
        let appearance = AppearanceConfig::default();

        assert!(LayoutSnapshot::new(0, 0, &appearance, monitor).is_err());
        for app_count in [1, 2, 20, 100] {
            let selected_index = app_count - 1;
            let layout =
                LayoutSnapshot::new(app_count, selected_index, &appearance, monitor).unwrap();

            assert_eq!(layout.app_count, app_count);
            assert!(layout
                .items
                .iter()
                .any(|item| item.app_index == selected_index));
            assert!(layout.panel_rect.left >= monitor.rect.left);
            assert!(layout.panel_rect.top >= monitor.rect.top);
            assert!(layout.panel_rect.right <= monitor.rect.right);
            assert!(layout.panel_rect.bottom <= monitor.rect.bottom);
        }
    }

    #[test]
    fn rejects_coordinate_and_dpi_overflow() {
        let appearance = AppearanceConfig::default();
        let coordinate_overflow = LayoutSnapshot::new(
            1,
            0,
            &appearance,
            monitor(
                RECT {
                    left: i32::MIN,
                    top: 0,
                    right: i32::MAX,
                    bottom: 1080,
                },
                96,
            ),
        );
        assert!(coordinate_overflow.is_err());

        let dpi_overflow = LayoutSnapshot::new(
            1,
            0,
            &appearance,
            monitor(
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                u32::MAX,
            ),
        );
        assert!(dpi_overflow.is_err());
    }

    #[test]
    fn dimensions_scale_with_monitor_dpi() {
        let appearance = AppearanceConfig::default();

        for (dpi, expected_icon_size, expected_item_size) in
            [(96, 64, 72), (144, 96, 108), (192, 128, 144)]
        {
            let layout = LayoutSnapshot::new(
                2,
                1,
                &appearance,
                monitor(
                    RECT {
                        left: 0,
                        top: 0,
                        right: 3840,
                        bottom: 2160,
                    },
                    dpi,
                ),
            )
            .unwrap();

            assert_eq!(layout.icon_size, expected_icon_size);
            assert_eq!(layout.item_size, expected_item_size);
        }
    }
}
