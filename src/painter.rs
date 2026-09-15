use crate::{
    app::SwitchAppsState,
    config::Config,
    layout::{LayoutOptions, LayoutSnapshot, PixelRect},
    pixels::PixelImage,
    render_surface::RenderSurface,
    utils::{is_light_theme, is_win11},
};
use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::{COLORREF, HWND, POINT, SIZE},
    Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION},
    UI::{
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{
            GetCursorPos, ShowWindow, UpdateLayeredWindow, SW_HIDE, SW_SHOW, ULW_ALPHA,
        },
    },
};

mod plan;
mod sprite;
use plan::RenderPlan;
use sprite::Sprite;

#[cfg(test)]
#[path = "painter_tests.rs"]
mod tests;

pub(crate) const ICON_SIZE_BASE: i32 = 64;

struct Scene {
    layout: LayoutSnapshot,
    count: usize,
    background: PixelImage,
    surface: RenderSurface,
    sprites: Vec<Sprite>,
    selected: usize,
    plan: RenderPlan,
}

pub(crate) struct GdiAAPainter {
    hwnd: HWND,
    config: Config,
    rounded_corner: bool,
    colors: (u32, u32),
    show: bool,
    scene: Option<Scene>,
}

impl GdiAAPainter {
    pub(crate) fn new(hwnd: HWND, config: &Config) -> Result<Self> {
        LayoutOptions::from_config(config).validate()?;
        if config.switch_apps_enable {
            let monitor = crate::layout::MonitorSnapshot::capture(
                config,
                crate::utils::get_foreground_window(),
            )?;
            let layout =
                LayoutSnapshot::calculate(&LayoutOptions::from_config(config), monitor, 1, 0, 0)?;
            RenderPlan::new(config, &layout)?;
        }
        Ok(Self {
            hwnd,
            config: config.clone(),
            rounded_corner: is_win11(),
            colors: theme_color(is_light_theme()),
            show: false,
            scene: None,
        })
    }

    pub(crate) fn paint(
        &mut self,
        state: &SwitchAppsState,
        allowed: impl Fn() -> bool,
    ) -> Result<()> {
        if !allowed() {
            return Ok(());
        }
        self.render_allowed(state, &allowed)?;
        if !allowed() {
            self.hide();
            return Ok(());
        }
        if !self.show {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                if allowed() {
                    let _ = SetFocus(Some(self.hwnd));
                } else {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                    return Ok(());
                }
            }
            self.show = true;
        }
        Ok(())
    }

    #[cfg(test)]
    fn render(&mut self, state: &SwitchAppsState) -> Result<()> {
        self.render_allowed(state, || true)
    }

    fn render_allowed(
        &mut self,
        state: &SwitchAppsState,
        allowed: impl Fn() -> bool,
    ) -> Result<()> {
        ensure!(
            !state.apps.is_empty() && state.index < state.apps.len(),
            "paint stage=count invalid"
        );
        let layout_start = crate::diagnostics::sample_start(self.config.metrics_enabled);
        let reuse = self.scene.as_ref().is_some_and(|scene| {
            scene.count == state.apps.len()
                && scene.layout.monitor == state.monitor
                && scene
                    .layout
                    .items
                    .iter()
                    .any(|item| item.index == state.index)
        });
        let replacement = if !reuse {
            let layout = LayoutSnapshot::calculate(
                &LayoutOptions::from_config(&self.config),
                state.monitor,
                state.apps.len(),
                state.index,
                0,
            )?;
            let plan = RenderPlan::new(&self.config, &layout)?;
            Some((layout, plan))
        } else {
            None
        };
        crate::diagnostics::stage_elapsed("layout", layout_start);
        let mut rebuilt = 0;
        if let Some((layout, plan)) = replacement {
            let resources_start = crate::diagnostics::sample_start(self.config.metrics_enabled);
            rebuilt = layout.items.len();
            self.scene = None; // Release old surfaces before allocating their replacements.
            self.scene = Some(self.create_scene(state, layout, plan)?);
            crate::diagnostics::stage_elapsed("render-resources", resources_start);
        }
        let scene = self.scene.as_mut().unwrap();
        let compose_start = crate::diagnostics::sample_start(self.config.metrics_enabled);
        let previous = scene.selected;
        for (item, sprite) in scene.layout.items.iter().zip(&mut scene.sprites) {
            let entry = &state.apps[item.index];
            let changed = !sprite.matches(entry, state);
            if changed {
                *sprite = Sprite::new(
                    entry,
                    state,
                    item,
                    scene.plan.scale,
                    self.rounded_corner,
                    self.colors.0,
                    self.config.metrics_enabled,
                )?;
                rebuilt += 1;
            }
            if changed
                || item.index == previous
                || item.index == state.index
                || previous == usize::MAX
            {
                scene.surface.restore(&scene.background, item.outer)?;
                scene.surface.compose(
                    if item.index == state.index {
                        &sprite.selected
                    } else {
                        &sprite.plain
                    },
                    item.outer.left,
                    item.outer.top,
                )?;
            }
        }
        scene.selected = state.index;
        crate::diagnostics::stage_elapsed("composition", compose_start);
        if !allowed() {
            return Ok(());
        }
        let submit_start = crate::diagnostics::sample_start(self.config.metrics_enabled);
        let bounds = scene.layout.bounds;
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
            ..Default::default()
        };
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&POINT {
                    x: bounds.left,
                    y: bounds.top,
                }),
                Some(&SIZE {
                    cx: bounds.width(),
                    cy: bounds.height(),
                }),
                Some(scene.surface.dc()),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
        .context("paint stage=layered-window")?;
        crate::diagnostics::stage_elapsed("submit", submit_start);
        if self.config.metrics_enabled {
            info!("metrics event=frame width={} height={} scale={} reserved_bytes={} rebuilt_sprites={} page={}",
                bounds.width(), bounds.height(), scene.plan.scale, scene.plan.reserved_bytes, rebuilt, scene.layout.page);
        }
        Ok(())
    }

    fn create_scene(
        &self,
        state: &SwitchAppsState,
        layout: LayoutSnapshot,
        plan: RenderPlan,
    ) -> Result<Scene> {
        let width = layout.bounds.width();
        let height = layout.bounds.height();
        let mut background = PixelImage::new(width, height)?;
        let radius = if self.rounded_corner {
            layout.items[0].outer.height() as f32 / 8.0
        } else {
            0.0
        };
        background.rounded_fill(
            PixelRect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
            radius,
            self.colors.1,
        );
        let mut surface = RenderSurface::new(width, height)?;
        surface.pixels_mut()?.copy_from_slice(&background.data);
        let mut sprites = Vec::with_capacity(layout.items.len());
        for item in &layout.items {
            sprites.push(Sprite::new(
                &state.apps[item.index],
                state,
                item,
                plan.scale,
                self.rounded_corner,
                self.colors.0,
                self.config.metrics_enabled,
            )?);
        }
        Ok(Scene {
            layout,
            count: state.apps.len(),
            background,
            surface,
            sprites,
            selected: usize::MAX,
            plan,
        })
    }

    pub(crate) fn hide(&mut self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.show = false;
    }

    pub(crate) fn invalidate(&mut self) {
        self.scene = None;
        self.colors = theme_color(is_light_theme());
    }

    pub(crate) fn layout(&self) -> Option<&LayoutSnapshot> {
        self.scene.as_ref().map(|scene| &scene.layout)
    }

    pub(crate) fn find_clicked_app_index(&self) -> Option<usize> {
        if !self.show {
            return None;
        }
        let mut cursor = POINT::default();
        unsafe { GetCursorPos(&mut cursor) }.ok()?;
        self.layout()?.hit_test(cursor.x, cursor.y)
    }
}

const fn theme_color(light: bool) -> (u32, u32) {
    if light {
        (0xf2f2f2, 0xe0e0e0)
    } else {
        (0x3b3b3b, 0x4c4c4c)
    }
}
