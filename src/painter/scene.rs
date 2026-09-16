use super::{plan::RenderPlan, sprite::Sprite, PointerFeedback};
use crate::{
    app::SwitchAppsState,
    appearance::Appearance,
    config::Config,
    font_resources::{FontResources, FontRole, TextBox},
    layout::{LayoutSnapshot, PixelRect},
    pixels::PixelImage,
    render_surface::RenderSurface,
};
use anyhow::Result;
use std::sync::Arc;

pub(super) struct Scene {
    pub(super) layout: LayoutSnapshot,
    pub(super) count: usize,
    pub(super) surface: RenderSurface,
    pub(super) plan: RenderPlan,
    background: PixelImage,
    pub(super) sprites: Vec<Sprite>,
    selected: usize,
    feedback: Vec<u8>,
    name: Option<Arc<str>>,
}

impl Scene {
    pub(super) fn new(
        state: &SwitchAppsState,
        layout: LayoutSnapshot,
        plan: RenderPlan,
        config: &Config,
        appearance: &Appearance,
        mut fonts: Option<&mut FontResources>,
    ) -> Result<Self> {
        let width = layout.bounds.width();
        let height = layout.bounds.height();
        let mut background = PixelImage::new(width, height)?;
        let radius =
            appearance.radii(config, layout.monitor.dpi, layout.items[0].outer.height())[0];
        background.rounded_fill_opacity(
            PixelRect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
            radius,
            appearance.panel.color,
            appearance.panel.opacity,
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
                config,
                appearance,
                fonts.as_deref_mut(),
            )?);
        }
        let feedback = vec![0; sprites.len()];
        Ok(Self {
            layout,
            count: state.apps.len(),
            surface,
            plan,
            background,
            sprites,
            selected: usize::MAX,
            feedback,
            name: None,
        })
    }

    pub(super) fn compose(
        &mut self,
        state: &SwitchAppsState,
        config: &Config,
        appearance: &Appearance,
        mut fonts: Option<&mut FontResources>,
        pointer: &PointerFeedback,
    ) -> Result<usize> {
        let mut rebuilt = 0;
        for ((item, sprite), previous_feedback) in self
            .layout
            .items
            .iter()
            .zip(&mut self.sprites)
            .zip(&mut self.feedback)
        {
            let entry = &state.apps[item.index];
            let feedback = if pointer.hovered.as_ref() == Some(&entry.key) {
                if pointer.pressed.as_ref() == Some(&entry.key) {
                    2
                } else {
                    1
                }
            } else {
                0
            };
            let changed = !sprite.matches(entry, state);
            if changed {
                *sprite = Sprite::new(
                    entry,
                    state,
                    item,
                    self.plan.scale,
                    config,
                    appearance,
                    fonts.as_deref_mut(),
                )?;
                rebuilt += 1;
            }
            let selected = item.index == state.index;
            if changed
                || item.index == self.selected
                || selected
                || self.selected == usize::MAX
                || *previous_feedback != feedback
            {
                self.surface.restore(&self.background, item.outer)?;
                self.surface.compose(
                    if selected {
                        &sprite.selected
                    } else {
                        &sprite.plain
                    },
                    item.outer.left,
                    item.outer.top,
                )?;
                if feedback != 0 {
                    let mut outline = PixelImage::new(item.outer.width(), item.outer.height())?;
                    let radius =
                        appearance.radii(config, self.layout.monitor.dpi, item.outer.height())
                            [if selected { 2 } else { 1 }];
                    let width = ((if feedback == 2 { 3.0 } else { 1.0 })
                        * self.layout.monitor.dpi as f32
                        / 96.0)
                        .round()
                        .max(1.0) as i32;
                    outline.rounded_outline(
                        PixelRect {
                            left: 0,
                            top: 0,
                            right: outline.width,
                            bottom: outline.height,
                        },
                        radius,
                        width,
                        appearance.border,
                    );
                    self.surface
                        .compose(&outline, item.outer.left, item.outer.top)?;
                }
            }
            *previous_feedback = feedback;
        }
        self.selected = state.index;
        if let Some(footer) = self.layout.footer {
            let name = &state.apps[state.index].display_name;
            if self.name.as_ref() != Some(name) {
                self.surface.restore(&self.background, footer)?;
                let pixels = (config.app_name_font_size as f32 * self.layout.monitor.dpi as f32
                    / 96.0)
                    .round()
                    .max(1.0) as u32;
                let image = super::text::name(
                    fonts,
                    config,
                    name,
                    TextBox {
                        role: FontRole::Name,
                        pixels,
                        width: footer.width(),
                        height: footer.height(),
                        ellipsis: true,
                    },
                    appearance.text,
                )?;
                self.surface.compose(&image, footer.left, footer.top)?;
                self.name = Some(name.clone());
            }
        }
        Ok(rebuilt)
    }
}
