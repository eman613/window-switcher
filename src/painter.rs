use crate::{
    app::SwitchAppsState,
    appearance::Appearance,
    config::Config,
    font_resources::{FontResources, FontService},
    icon_cache::IconKey,
    layout::{LayoutOptions, LayoutSnapshot},
    window_target::WindowTarget,
};
use anyhow::{ensure, Context, Result};
use std::{path::Path, sync::Arc};
use windows::Win32::{
    Foundation::{COLORREF, HWND, POINT, SIZE},
    Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION},
    UI::{
        Input::KeyboardAndMouse::{GetCapture, ReleaseCapture},
        WindowsAndMessaging::{
            GetCursorPos, ShowWindow, UpdateLayeredWindow, SW_HIDE, SW_SHOW, ULW_ALPHA,
        },
    },
};

mod plan;
mod scene;
mod sprite;
mod text;
use plan::RenderPlan;
use scene::Scene;

#[cfg(test)]
#[path = "painter_tests.rs"]
mod tests;

pub(crate) const ICON_SIZE_BASE: i32 = 64;

#[derive(Default, PartialEq, Eq)]
struct PointerFeedback {
    hovered: Option<IconKey>,
    pressed: Option<IconKey>,
}

pub(crate) struct GdiAAPainter {
    hwnd: HWND,
    config: Config,
    appearance: Appearance,
    fonts: Option<FontResources>,
    font_service: Option<FontService>,
    pointer: PointerFeedback,
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
            let layout = LayoutSnapshot::calculate(
                &LayoutOptions::from_config(config),
                monitor,
                1,
                0,
                text::name_height(config, None),
            )?;
            RenderPlan::new(config, &layout)?;
        }
        Ok(Self {
            hwnd,
            config: config.clone(),
            appearance: Appearance::capture(config),
            fonts: None,
            font_service: None,
            pointer: Default::default(),
            show: false,
            scene: None,
        })
    }

    pub(crate) fn start_fonts(&mut self, ini_dir: &Path, target: Arc<WindowTarget>) {
        match FontService::start(&self.config, ini_dir, target) {
            Ok(service) => self.font_service = Some(service),
            Err(error) => warn!("font stage=start fallback={error:#}; INI unchanged"),
        }
    }

    pub(crate) fn poll_fonts(&mut self) -> bool {
        let Some(result) = self.font_service.as_ref().and_then(FontService::take) else {
            return false;
        };
        match result {
            Ok(fonts) => {
                self.fonts = Some(fonts);
                self.scene = None;
                true
            }
            Err(error) => {
                warn!("font stage=load fallback={error:#}; INI unchanged");
                false
            }
        }
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
            let started = crate::diagnostics::sample_start(self.config.metrics_enabled);
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
            crate::diagnostics::stage_elapsed("show-window", started);
            let focus_started = crate::diagnostics::sample_start(self.config.metrics_enabled);
            crate::utils::focus_window(self.hwnd, &allowed);
            crate::diagnostics::stage_elapsed("panel-focus", focus_started);
            crate::diagnostics::stage_elapsed("panel-show", started);
            if !allowed() {
                self.hide();
                return Ok(());
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
                text::name_height(&self.config, self.fonts.as_ref()),
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
            self.scene = Some(Scene::new(
                state,
                layout,
                plan,
                &self.config,
                &self.appearance,
                self.fonts.as_mut(),
            )?);
            crate::diagnostics::stage_elapsed("render-resources", resources_start);
        }
        let scene = self.scene.as_mut().unwrap();
        let compose_start = crate::diagnostics::sample_start(self.config.metrics_enabled);
        rebuilt += scene.compose(
            state,
            &self.config,
            &self.appearance,
            self.fonts.as_mut(),
            &self.pointer,
        )?;
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

    pub(crate) fn hide(&mut self) {
        unsafe {
            if GetCapture() == self.hwnd {
                if let Err(error) = ReleaseCapture() {
                    debug!("pointer stage=hide-release code={:#x}", error.code().0);
                }
            }
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.show = false;
        self.pointer = Default::default();
    }

    pub(crate) fn invalidate(&mut self) {
        self.scene = None;
        self.appearance = Appearance::capture(&self.config);
    }

    pub(crate) fn layout(&self) -> Option<&LayoutSnapshot> {
        self.scene.as_ref().map(|scene| &scene.layout)
    }

    pub(crate) fn reserved_bytes(&self) -> usize {
        self.scene
            .as_ref()
            .map_or(0, |scene| scene.plan.reserved_bytes)
    }

    pub(crate) fn find_clicked_app_index(&self) -> Option<usize> {
        if !self.show {
            return None;
        }
        let mut cursor = POINT::default();
        unsafe { GetCursorPos(&mut cursor) }.ok()?;
        self.layout()?.hit_test(cursor.x, cursor.y)
    }
    pub(crate) fn hover(&mut self, key: Option<IconKey>) -> bool {
        let changed = self.pointer.hovered != key;
        self.pointer.hovered = key;
        changed
    }
    pub(crate) fn press(&mut self, key: Option<IconKey>) {
        self.pointer.pressed = key;
    }
    pub(crate) fn release(&mut self) -> Option<IconKey> {
        self.pointer.pressed.take()
    }
}
