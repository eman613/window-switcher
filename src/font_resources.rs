//! One background font initialization; native collections stay role-specific.
use anyhow::{Context, Result};
use std::{
    path::Path,
    sync::Arc,
    thread::{self, JoinHandle},
};
use windows::Win32::Graphics::DirectWrite::*;

use crate::{
    config::Config,
    window_target::WindowTarget,
    worker::{self, Mailbox},
};

mod formats;
mod loader;
mod local_file;
pub(crate) use formats::TextBox;
#[cfg(test)]
mod tests;

pub(crate) const WM_FONTS: u32 = 6013;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FontRole {
    Name,
    Badge,
}

impl FontRole {
    fn index(self) -> usize {
        match self {
            Self::Name => 0,
            Self::Badge => 1,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Badge => "badge",
        }
    }
}

// Field order releases formats/collections before unregistering the private loader.
pub(crate) struct FontResources {
    formats: indexmap::IndexMap<(FontRole, u32), IDWriteTextFormat>,
    fonts: [loader::ResolvedFont; 2],
    pub(crate) factory: IDWriteFactory,
    _loader: Option<loader::MemoryLoader>,
}

impl FontResources {
    pub(crate) fn line_height(&self, role: FontRole, size: u32) -> u32 {
        let mut metrics = DWRITE_FONT_METRICS::default();
        unsafe { self.fonts[role.index()].face.GetMetrics(&mut metrics) };
        let units = u32::from(metrics.designUnitsPerEm).max(1);
        let line =
            (i32::from(metrics.ascent) + i32::from(metrics.descent) + i32::from(metrics.lineGap))
                .max(1) as u32;
        ((u64::from(line) * u64::from(size)).div_ceil(u64::from(units)) as u32).min(256)
    }

    pub(crate) fn load(config: &Config, ini_dir: &Path) -> Result<Self> {
        use loader::{resolve, FontChoice, MemoryLoader};
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) }
            .context("font stage=factory")?;
        let private = config.app_name_font_file.is_some() || config.badge_font_file.is_some();
        let loader = if private {
            match MemoryLoader::new(&factory) {
                Ok(loader) => Some(loader),
                Err(error) => {
                    warn!("font stage=private-api-fallback error={error:#}; INI unchanged");
                    None
                }
            }
        } else {
            None
        };
        let mut system = None;
        unsafe { factory.GetSystemFontCollection(&mut system, false) }?;
        let system = system.context("font stage=system null-collection")?;
        let family = loader::system_ui_family();
        let name = resolve(
            &system,
            loader.as_ref(),
            ini_dir,
            FontChoice {
                role: FontRole::Name,
                family: &config.app_name_font_family,
                file: config.app_name_font_file.as_deref(),
                weight: config.app_name_font_weight,
                italic: config.app_name_font_italic,
            },
            &family,
        )?;
        let badge = resolve(
            &system,
            loader.as_ref(),
            ini_dir,
            FontChoice {
                role: FontRole::Badge,
                family: &config.badge_font_family,
                file: config.badge_font_file.as_deref(),
                weight: 600,
                italic: false,
            },
            &family,
        )?;
        Ok(Self {
            formats: Default::default(),
            fonts: [name, badge],
            factory,
            _loader: loader,
        })
    }
}

pub(crate) struct FontService {
    mailbox: Arc<Mailbox<(), Result<FontResources>>>,
    thread: Option<JoinHandle<()>>,
}

impl FontService {
    pub(crate) fn start(
        config: &Config,
        ini_dir: &Path,
        target: Arc<WindowTarget>,
    ) -> Result<Self> {
        let mailbox = Mailbox::new(1);
        let shared = mailbox.clone();
        let config = config.clone();
        let ini_dir = ini_dir.to_owned();
        let generation = mailbox.request(());
        let thread = thread::Builder::new()
            .name("font-loader".into())
            .spawn(move || {
                let result = crate::utils::com::ComApartment::sta().and_then(|_com| {
                    let started = crate::diagnostics::sample_start(config.metrics_enabled);
                    let fonts = FontResources::load(&config, &ini_dir);
                    crate::diagnostics::stage_elapsed("font-load", started);
                    fonts
                });
                if shared.current(generation)
                    && target.is_live()
                    && shared.publish(generation, result)
                {
                    target.try_post(WM_FONTS);
                }
            })
            .context("font stage=thread-create")?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
        })
    }

    pub(crate) fn take(&self) -> Option<Result<FontResources>> {
        self.mailbox
            .take()
            .into_iter()
            .next()
            .map(|(_, result)| result)
    }
}

impl Drop for FontService {
    fn drop(&mut self) {
        self.mailbox.close();
        worker::retire(self.thread.take(), "fonts");
    }
}
