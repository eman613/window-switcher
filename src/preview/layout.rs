use super::PreviewRequest;
use crate::{config::Config, layout::PixelRect};

const GAP_DIP: u32 = 12;
const PADDING_DIP: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PreviewLayout {
    pub(super) bounds: PixelRect,
    pub(super) content: PixelRect,
    /// An output-pixel allowance, not a measurement of DWM GPU allocations.
    pub(super) output_bytes: usize,
}

impl PreviewLayout {
    pub(super) fn calculate(
        config: &Config,
        request: PreviewRequest,
        source: (i32, i32),
        remaining_bytes: usize,
    ) -> Option<Self> {
        let work = request.monitor.available;
        let anchor = request.anchor;
        if source.0 <= 0 || source.1 <= 0 || work.width() <= 0 || work.height() <= 0 {
            return None;
        }
        let px = |dip| (u64::from(dip) * u64::from(request.monitor.dpi) / 96) as i32;
        let gap = px(GAP_DIP).max(1);
        let padding = px(PADDING_DIP).max(1);
        // Each slot is entirely outside the active input surface. Centering the
        // result in the nearest slot never covers a list row or panel hit target.
        let slots = [
            PixelRect {
                left: anchor
                    .right
                    .saturating_add(gap)
                    .clamp(work.left, work.right),
                ..work
            },
            PixelRect {
                right: anchor.left.saturating_sub(gap).clamp(work.left, work.right),
                ..work
            },
            PixelRect {
                top: anchor
                    .bottom
                    .saturating_add(gap)
                    .clamp(work.top, work.bottom),
                ..work
            },
            PixelRect {
                bottom: anchor.top.saturating_sub(gap).clamp(work.top, work.bottom),
                ..work
            },
        ];
        let mut best: Option<Self> = None;
        for slot in slots {
            let width = slot.width().min(px(config.preview_max_width));
            let height = slot.height().min(px(config.preview_max_height));
            let Some((width, height, output_bytes)) =
                fit(source, width, height, padding, remaining_bytes)
            else {
                continue;
            };
            let left = (i64::from(anchor.left) + i64::from(anchor.width() - width) / 2)
                .clamp(i64::from(slot.left), i64::from(slot.right - width))
                as i32;
            let top = (i64::from(anchor.top) + i64::from(anchor.height() - height) / 2)
                .clamp(i64::from(slot.top), i64::from(slot.bottom - height))
                as i32;
            let layout = Self {
                bounds: PixelRect {
                    left,
                    top,
                    right: left + width,
                    bottom: top + height,
                },
                content: PixelRect {
                    left: padding,
                    top: padding,
                    right: width - padding,
                    bottom: height - padding,
                },
                output_bytes,
            };
            if best.is_none_or(|old| layout.output_bytes > old.output_bytes) {
                best = Some(layout);
            }
        }
        best
    }
}

fn fit(
    source: (i32, i32),
    width: i32,
    height: i32,
    padding: i32,
    budget: usize,
) -> Option<(i32, i32, usize)> {
    let inner_width = width - 2 * padding;
    let inner_height = height - 2 * padding;
    if inner_width <= 0 || inner_height <= 0 {
        return None;
    }
    // Search the largest width that satisfies height and total output budget.
    // Products remain bounded even when the public API returns enormous sources.
    let (mut low, mut high, mut best) = (1, inner_width, None);
    while low <= high {
        let content_width = low + (high - low) / 2;
        let content_height =
            (i64::from(content_width) * i64::from(source.1) / i64::from(source.0)).max(1);
        let outer_width = content_width + 2 * padding;
        let outer_height = content_height + i64::from(2 * padding);
        let bytes = i64::from(outer_width) * outer_height * 4;
        if content_height <= i64::from(inner_height)
            && bytes <= budget.min(256 * 1024 * 1024) as i64
        {
            best = Some((outer_width, outer_height as i32, bytes as usize));
            low = content_width + 1;
        } else {
            high = content_width - 1;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intersects(a: PixelRect, b: PixelRect) -> bool {
        a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
    }

    #[test]
    fn sides_dpi_aspect_and_budget_share_one_nonoverlapping_layout() {
        let config = Config::default();
        for dpi in [96, 144, 192] {
            for anchor in [
                PixelRect {
                    left: 0,
                    top: 0,
                    right: 1600,
                    bottom: 1080,
                },
                PixelRect {
                    left: 320,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                PixelRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 800,
                },
                PixelRect {
                    left: 0,
                    top: 280,
                    right: 1920,
                    bottom: 1080,
                },
            ] {
                let mut request = super::super::fixture(1);
                request.anchor = anchor;
                request.monitor.dpi = dpi;
                for budget in [256 * 1024, 8 * 1024 * 1024] {
                    let layout = PreviewLayout::calculate(&config, request, (1920, 1080), budget)
                        .unwrap_or_else(|| panic!("dpi={dpi} anchor={anchor:?} budget={budget}"));
                    assert!(!intersects(layout.bounds, anchor));
                    assert!(layout.bounds.left >= 0 && layout.bounds.right <= 1920);
                    assert!(layout.bounds.top >= 0 && layout.bounds.bottom <= 1080);
                    assert!(layout.bounds.width() <= (config.preview_max_width * dpi / 96) as i32);
                    assert!(
                        layout.bounds.height() <= (config.preview_max_height * dpi / 96) as i32
                    );
                    assert!(layout.output_bytes <= budget);
                    assert!(
                        (layout.content.width() * 1080 - layout.content.height() * 1920).abs()
                            < 1920
                    );
                }
            }
        }
    }

    #[test]
    fn extreme_dpi_hides_when_spacing_leaves_no_content_area() {
        let mut request = super::super::fixture(1);
        request.monitor.dpi = 960;
        request.anchor = PixelRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 800,
        };
        // The 280 physical pixels below the panel are consumed by a 120 px
        // gap and 160 px padding. A positive thumbnail cannot fit there.
        assert_eq!(
            PreviewLayout::calculate(&Config::default(), request, (1920, 1080), 8 * 1024 * 1024),
            None
        );
    }

    #[test]
    fn negative_monitors_exhausted_space_and_large_sources_are_bounded() {
        let config = Config {
            preview_max_width: 160,
            preview_max_height: 90,
            ..Default::default()
        };
        let mut request = super::super::fixture(1);
        request.monitor.available = PixelRect {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: 0,
        };
        request.anchor = PixelRect {
            left: -1280,
            top: -800,
            right: -640,
            bottom: -360,
        };
        let result =
            PreviewLayout::calculate(&config, request, (i32::MAX, i32::MAX), 1024 * 1024).unwrap();
        assert!(result.bounds.left >= -1920 && result.bounds.right <= 0);
        assert!(result.bounds.top >= -1080 && result.bounds.bottom <= 0);
        assert_eq!(result.content.width(), result.content.height());
        assert_eq!(
            PreviewLayout::calculate(&config, request, (320, 200), 0),
            None
        );
        assert_eq!(
            PreviewLayout::calculate(&config, request, (0, 200), usize::MAX),
            None
        );
        request.anchor = request.monitor.available;
        assert_eq!(
            PreviewLayout::calculate(&config, request, (320, 200), usize::MAX),
            None
        );
    }
}
