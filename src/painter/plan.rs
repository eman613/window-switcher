use crate::{
    config::{Config, RenderScale},
    layout::LayoutSnapshot,
    utils::gdi::checked_bitmap_bytes,
};
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RenderPlan {
    pub(super) scale: i32,
    pub(super) reserved_bytes: usize,
}

impl RenderPlan {
    pub(super) fn new(config: &Config, layout: &LayoutSnapshot) -> Result<Self> {
        let requested = match config.render_scale {
            RenderScale::Auto => {
                if layout.monitor.dpi >= 144 {
                    1
                } else {
                    2
                }
            }
            RenderScale::One => 1,
            RenderScale::Two => 2,
            RenderScale::Four => 4,
            RenderScale::Six => 6,
        };
        let frame = checked_bitmap_bytes(layout.bounds.width(), layout.bounds.height())?;
        let mut tiles = 0usize;
        let mut largest = 0usize;
        for item in &layout.items {
            let bytes = checked_bitmap_bytes(item.outer.width(), item.outer.height())?;
            tiles = tiles
                .checked_add(bytes)
                .context("render stage=budget tile-overflow")?;
            largest = largest.max(bytes);
        }
        for scale in [6, 4, 2, 1].into_iter().filter(|scale| *scale <= requested) {
            // Native DIB + immutable background + two cached tile states. Six
            // scaled scratch tiles cover icon resize, both badge mattes, alpha
            // recovery and sprite assembly, including replacement overlap.
            let bytes = frame
                .checked_mul(2)
                .and_then(|v| v.checked_add(tiles.checked_mul(2)?))
                .and_then(|v| {
                    v.checked_add(largest.checked_mul(6 * scale as usize * scale as usize + 4)?)
                })
                .context("render stage=budget overflow")?;
            if bytes <= config.render_budget_mb as usize * 1024 * 1024 {
                return Ok(Self {
                    scale,
                    reserved_bytes: bytes,
                });
            }
        }
        bail!("render stage=budget cannot-fit native frame")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LayoutOptions, MonitorSnapshot, PixelRect};
    #[test]
    fn requested_quality_steps_down_within_total_budget() {
        let mut config = Config {
            icon_size: 256,
            render_scale: RenderScale::Six,
            render_budget_mb: 8,
            ..Default::default()
        };
        let screen = PixelRect {
            left: 0,
            top: 0,
            right: 4000,
            bottom: 2200,
        };
        let layout = LayoutSnapshot::calculate(
            &LayoutOptions::from_config(&config),
            MonitorSnapshot {
                identity: 1,
                screen,
                available: screen,
                dpi: 192,
            },
            5,
            1,
            0,
        )
        .unwrap();
        assert!(RenderPlan::new(&config, &layout).is_err());
        config.render_budget_mb = 64;
        let plan = RenderPlan::new(&config, &layout).unwrap();
        assert!(plan.scale < 6);
        assert!(plan.reserved_bytes <= 64 * 1024 * 1024);
        config.render_budget_mb = 256;
        assert!(RenderPlan::new(&config, &layout).unwrap().scale >= plan.scale);
    }
}
