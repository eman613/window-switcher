//! Local, role-specific DirectWrite collections. No process/system font registration.
use anyhow::{ensure, Context, Result};
use std::path::Path;
use windows::{
    core::{w, Interface, HSTRING},
    Win32::{
        Graphics::DirectWrite::*,
        UI::WindowsAndMessaging::{
            SystemParametersInfoW, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        },
    },
};

use super::FontRole;

pub(super) struct MemoryLoader {
    factory: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
}

impl MemoryLoader {
    pub(super) fn new(factory: &IDWriteFactory) -> Result<Self> {
        let factory: IDWriteFactory5 = factory.cast().context("font stage=private-api")?;
        let loader = unsafe { factory.CreateInMemoryFontFileLoader() }?;
        unsafe { factory.RegisterFontFileLoader(&loader) }?;
        Ok(Self { factory, loader })
    }

    fn collection(&self, path: &Path, ini_dir: &Path) -> Result<IDWriteFontCollection> {
        let data = super::local_file::read(path, ini_dir)?;
        // With a null owner DirectWrite owns a copy of the buffer (dwrite_3 contract).
        let font = unsafe {
            self.loader.CreateInMemoryFontFileReference(
                &self.factory,
                data.as_ptr().cast(),
                data.len() as u32,
                None,
            )
        }?;
        let builder = unsafe { self.factory.CreateFontSetBuilder() }?;
        unsafe { builder.AddFontFile(&font) }?;
        let set = unsafe { builder.CreateFontSet() }?;
        let collection = unsafe { self.factory.CreateFontCollectionFromFontSet(&set) }?;
        Ok(collection.cast()?)
    }
}

impl Drop for MemoryLoader {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.factory.UnregisterFontFileLoader(&self.loader) } {
            warn!("font stage=loader-release code={:#x}", error.code().0);
        }
    }
}

pub(super) struct ResolvedFont {
    pub(super) collection: IDWriteFontCollection,
    pub(super) family: HSTRING,
    pub(super) face: IDWriteFontFace,
    pub(super) weight: DWRITE_FONT_WEIGHT,
    pub(super) style: DWRITE_FONT_STYLE,
    pub(super) missing_reported: bool,
    #[cfg(test)]
    pub(super) private: bool,
}

pub(super) struct FontChoice<'a> {
    pub(super) role: FontRole,
    pub(super) family: &'a str,
    pub(super) file: Option<&'a Path>,
    pub(super) weight: u32,
    pub(super) italic: bool,
}

pub(super) fn resolve(
    system: &IDWriteFontCollection,
    loader: Option<&MemoryLoader>,
    ini_dir: &Path,
    choice: FontChoice<'_>,
    system_family: &str,
) -> Result<ResolvedFont> {
    let requested = if choice.family == "auto" {
        system_family
    } else {
        choice.family
    };
    if let Some(path) = choice.file {
        let loaded = loader
            .context("font stage=private unavailable")
            .and_then(|loader| loader.collection(path, ini_dir))
            .and_then(|collection| {
                let family = private_family(&collection, requested, choice.role)?;
                materialize(collection, family, &choice, true)
            });
        match loaded {
            Ok(value) => return Ok(value),
            Err(error) => warn!(
                "font stage=private-fallback role={} error={error:#}; INI unchanged",
                choice.role.label()
            ),
        }
    }
    for family in [requested, system_family, "Segoe UI"] {
        if find_family(system, family).is_some() {
            match materialize(system.clone(), family.to_owned(), &choice, false) {
                Ok(font) => {
                    if !family.eq_ignore_ascii_case(requested) {
                        warn!(
                            "font stage=family-fallback role={}; INI unchanged",
                            choice.role.label()
                        );
                    }
                    return Ok(font);
                }
                Err(error) => warn!(
                    "font stage=face-fallback role={} error={error:#}",
                    choice.role.label()
                ),
            }
        }
    }
    anyhow::bail!("font stage=system no-ui-family")
}

fn materialize(
    collection: IDWriteFontCollection,
    family: String,
    choice: &FontChoice<'_>,
    private: bool,
) -> Result<ResolvedFont> {
    let index = find_family(&collection, &family).context("font stage=family unavailable")?;
    let family_object = unsafe { collection.GetFontFamily(index) }?;
    let weight = DWRITE_FONT_WEIGHT(choice.weight as i32);
    let style = if choice.italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };
    let matched =
        unsafe { family_object.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style) }?;
    let actual_weight = unsafe { matched.GetWeight() };
    let actual_style = unsafe { matched.GetStyle() };
    if actual_weight != weight || actual_style != style {
        warn!(
            "font stage=nearest-style role={} weight={} italic={}; INI unchanged",
            choice.role.label(),
            actual_weight.0,
            actual_style == DWRITE_FONT_STYLE_ITALIC
        );
    }
    let face = unsafe { matched.CreateFontFace() }?;
    let mut metrics = DWRITE_FONT_METRICS::default();
    unsafe { face.GetMetrics(&mut metrics) };
    let line = i32::from(metrics.ascent) + i32::from(metrics.descent) + i32::from(metrics.lineGap);
    ensure!(
        metrics.designUnitsPerEm > 0 && line > 0 && line <= 4 * i32::from(metrics.designUnitsPerEm),
        "font stage=face unsupported-line-metrics"
    );
    if private {
        debug!("font stage=private-ready role={}", choice.role.label());
    }
    Ok(ResolvedFont {
        collection,
        family: HSTRING::from(family),
        face,
        weight,
        style,
        missing_reported: false,
        #[cfg(test)]
        private,
    })
}

fn find_family(collection: &IDWriteFontCollection, name: &str) -> Option<u32> {
    let mut index = 0;
    let mut exists = windows::core::BOOL(0);
    unsafe { collection.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists) }.ok()?;
    exists.as_bool().then_some(index)
}

fn private_family(
    collection: &IDWriteFontCollection,
    requested: &str,
    role: FontRole,
) -> Result<String> {
    if find_family(collection, requested).is_some() {
        return Ok(requested.to_owned());
    }
    let count = unsafe { collection.GetFontFamilyCount() };
    ensure!(
        (1..=128).contains(&count),
        "font stage=private family-count"
    );
    let mut families = Vec::with_capacity(count as usize);
    for index in 0..count {
        let family = unsafe { collection.GetFontFamily(index) }?;
        let names = unsafe { family.GetFamilyNames() }?;
        let mut locale = 0;
        let mut exists = windows::core::BOOL(0);
        unsafe { names.FindLocaleName(w!("en-us"), &mut locale, &mut exists) }?;
        if !exists.as_bool() {
            locale = 0;
        }
        let length = unsafe { names.GetStringLength(locale) }?;
        ensure!(
            (1..=128).contains(&length),
            "font stage=private family-length"
        );
        let mut text = vec![0u16; length as usize + 1];
        unsafe { names.GetString(locale, &mut text) }?;
        families.push(String::from_utf16(&text[..length as usize])?);
    }
    families.sort_by_key(|name| (name.to_lowercase(), name.clone()));
    let family = families.remove(0);
    if count > 1 {
        info!(
            "font stage=private-family-choice role={} family={family}",
            role.label()
        );
    }
    Ok(family)
}

pub(super) fn system_ui_family() -> String {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    match unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    } {
        Ok(()) => {
            let length = metrics
                .lfMessageFont
                .lfFaceName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(metrics.lfMessageFont.lfFaceName.len());
            let family = String::from_utf16_lossy(&metrics.lfMessageFont.lfFaceName[..length]);
            if !family.trim().is_empty() {
                return family;
            }
        }
        Err(error) => warn!("font stage=system-ui-fallback code={:#x}", error.code().0),
    }
    "Segoe UI".into()
}
