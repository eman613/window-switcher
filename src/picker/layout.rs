use anyhow::{ensure, Result};
use windows::Win32::Foundation::RECT;

use super::{skin::px, ViewKind};
use crate::{config::Config, layout::MonitorSnapshot};

pub(super) struct PickerLayout {
    pub label: RECT,
    pub edit: RECT,
    pub results_label: RECT,
    pub list: RECT,
    pub status: RECT,
    pub back: RECT,
    pub clear: RECT,
    pub dismiss: RECT,
    pub notice: RECT,
    pub help: RECT,
    pub query_bottom: i32,
    pub row_height: i32,
}

fn rect(left: i32, top: i32, width: i32, height: i32) -> RECT {
    RECT {
        left,
        top,
        right: left + width.max(0),
        bottom: top + height.max(0),
    }
}

impl PickerLayout {
    pub fn target(
        config: &Config,
        monitor: MonitorSnapshot,
        kind: ViewKind,
        row_height: i32,
        text_height: i32,
        help_open: bool,
    ) -> (i32, i32) {
        let (width, height) = if kind == ViewKind::Search {
            let (query, heading, footer, _) = chrome(monitor.dpi, text_height, help_open);
            (
                px(config.search_width as i32, monitor.dpi),
                query + heading + footer + row_height * config.search_visible_rows as i32,
            )
        } else {
            (px(640, monitor.dpi), px(440, monitor.dpi))
        };
        (
            width.min(monitor.available.width()),
            height.min(monitor.available.height()),
        )
    }

    pub fn calculate(
        width: i32,
        height: i32,
        dpi: u32,
        kind: ViewKind,
        row_height: i32,
        text_height: i32,
        help_open: bool,
    ) -> Result<Self> {
        ensure!(
            width >= px(240, dpi)
                && height >= px(if kind == ViewKind::Search { 60 } else { 180 }, dpi),
            "picker stage=layout insufficient-work-area"
        );
        let p = |value| px(value, dpi);
        let margin = p(16);
        if kind == ViewKind::Details {
            return Ok(Self {
                label: rect(margin, margin, width - 2 * margin, p(22)),
                edit: RECT::default(),
                results_label: RECT::default(),
                list: rect(
                    margin,
                    margin + p(30),
                    width - 2 * margin,
                    height - margin - p(82),
                ),
                status: rect(margin, height - p(42), width - 2 * margin - p(158), p(32)),
                back: rect(width - margin - p(148), height - p(42), p(148), p(32)),
                clear: RECT::default(),
                dismiss: RECT::default(),
                notice: RECT::default(),
                help: RECT::default(),
                query_bottom: p(40),
                row_height: p(30),
            });
        }
        let (query_bottom, heading_height, footer_height, input_height) =
            chrome(dpi, text_height, help_open);
        let list_top = query_bottom + heading_height;
        let footer_top = height - footer_height;
        ensure!(
            footer_top > list_top,
            "picker stage=layout insufficient-text-area"
        );
        let help_height = if help_open {
            p(17).max(text_height + p(5))
        } else {
            0
        };
        let button_height = (text_height * 18 / 12).max(p(18));
        let button_top = help_height + p(4) + (input_height + p(12) - button_height) / 2;
        Ok(Self {
            label: RECT::default(),
            edit: rect(p(10), help_height + p(10), width - p(48), input_height),
            results_label: RECT::default(),
            list: rect(p(4), list_top, width - p(22), footer_top - list_top),
            status: RECT::default(),
            clear: RECT::default(),
            dismiss: rect(width - p(29), button_top, p(15), button_height),
            back: RECT::default(),
            notice: rect(
                p(24),
                list_top,
                width - p(48),
                (text_height * 15 / 14 + p(8)).max(p(34)),
            ),
            help: rect(p(8), p(4), width - p(16), help_height),
            query_bottom,
            row_height,
        })
    }
}

fn chrome(dpi: u32, text_height: i32, help_open: bool) -> (i32, i32, i32, i32) {
    let input_height = text_height * 15 * 127 / 1200;
    let help = if help_open {
        px(17, dpi).max(text_height + px(5, dpi))
    } else {
        0
    };
    (
        input_height + px(16, dpi) + help,
        px(3, dpi),
        px(4, dpi),
        input_height,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PixelRect;

    #[test]
    fn search_sizes_follow_config_dpi_and_work_area() {
        let config = Config {
            search_width: 960,
            search_visible_rows: 9,
            ..Default::default()
        };
        for dpi in [96, 144, 192] {
            let area = PixelRect {
                left: -1600,
                top: 0,
                right: 0,
                bottom: 1000,
            };
            let monitor = MonitorSnapshot {
                identity: 1,
                screen: area,
                available: area,
                dpi,
            };
            let (width, height) = PickerLayout::target(
                &config,
                monitor,
                ViewKind::Search,
                px(64, dpi),
                px(14, dpi),
                false,
            );
            assert_eq!(width, px(960, dpi).min(1600));
            assert!(height <= 1000);
            let layout = PickerLayout::calculate(
                width,
                height,
                dpi,
                ViewKind::Search,
                px(64, dpi),
                px(14, dpi),
                false,
            )
            .unwrap();
            assert!(layout.list.bottom <= height);
            assert!(layout.edit.right < layout.dismiss.left);
            assert!(layout.dismiss.right <= width);
            assert!(layout.list.bottom > layout.list.top);
        }
        assert!(PickerLayout::calculate(180, 120, 96, ViewKind::Search, 64, 14, false).is_err());
    }

    #[test]
    fn larger_text_keeps_input_actions_separate_and_readable() {
        for dpi in [96, 144, 192] {
            for text_scale in [1, 2] {
                let text = px(14 * text_scale, dpi);
                let layout = PickerLayout::calculate(
                    px(480, dpi),
                    px(700, dpi),
                    dpi,
                    ViewKind::Search,
                    px(100, dpi),
                    text,
                    false,
                )
                .unwrap();
                assert!(layout.edit.right < layout.dismiss.left);
                assert_eq!(layout.clear.right, 0);
                assert!(layout.dismiss.bottom < layout.query_bottom);
                assert!(layout.dismiss.bottom - layout.dismiss.top > text);
                assert!(layout.dismiss.right <= px(480, dpi));
            }
        }
    }
}
