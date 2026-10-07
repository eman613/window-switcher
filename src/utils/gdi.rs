use anyhow::{bail, Context, Result};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteDC, DeleteObject, RestoreDC, SaveDC, SelectObject, HDC, HGDIOBJ,
};
use windows::Win32::UI::{
    HiDpi::SystemParametersInfoForDpi,
    WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS},
};

pub(crate) struct OwnedGdiObject(pub(crate) HGDIOBJ);

/// A smaller hint face derived from the actual content face, including fallback.
pub(crate) fn hint_font(content: &OwnedGdiObject) -> Result<OwnedGdiObject> {
    use windows::Win32::Graphics::Gdi::{GetObjectW, LOGFONTW};
    let mut font = LOGFONTW::default();
    anyhow::ensure!(
        unsafe {
            GetObjectW(
                content.0,
                std::mem::size_of::<LOGFONTW>() as i32,
                Some((&mut font as *mut LOGFONTW).cast()),
            )
        } == std::mem::size_of::<LOGFONTW>() as i32,
        "font stage=hint-metrics failed"
    );
    font.lfHeight = -(font.lfHeight.saturating_abs() * 10 / 12).max(1);
    font.lfWeight = 400;
    OwnedGdiObject::new(
        HGDIOBJ(unsafe { CreateFontIndirectW(&font) }.0),
        "hint-font",
    )
}

impl OwnedGdiObject {
    pub(crate) fn new(object: HGDIOBJ, operation: &str) -> Result<Self> {
        if object.is_invalid() {
            bail!("gdi stage={operation} invalid object");
        }
        Ok(Self(object))
    }
}

impl Drop for OwnedGdiObject {
    fn drop(&mut self) {
        if !unsafe { DeleteObject(self.0) }.as_bool() {
            warn!("gdi stage=delete-object failed");
        }
    }
}

/// The global content font is local to this application, never a system setting.
pub(crate) fn content_font(
    config: &crate::config::Config,
    dpi: u32,
    minimum_dip: u32,
) -> Result<OwnedGdiObject> {
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, GetObjectW, GetTextFaceW, LOGFONTW};
    if !config.unified_font {
        return message_font_with_floor(dpi, minimum_dip);
    }
    let system = message_font_with_floor(dpi, config.ui_font_size)?;
    let mut value = LOGFONTW::default();
    anyhow::ensure!(
        unsafe {
            GetObjectW(
                system.0,
                std::mem::size_of::<LOGFONTW>() as i32,
                Some((&mut value as *mut LOGFONTW).cast()),
            )
        } != 0,
        "font stage=content-metrics failed"
    );
    value.lfHeight = -((config.ui_font_size * dpi / 96).max(1) as i32);
    if config.ui_font_family != "auto" {
        let family: Vec<_> = config.ui_font_family.encode_utf16().collect();
        if family.len() >= value.lfFaceName.len() {
            warn!("font stage=content-family too-long; using system font, INI unchanged");
            return Ok(system);
        }
        value.lfFaceName.fill(0);
        value.lfFaceName[..family.len()].copy_from_slice(&family);
    }
    let font = OwnedGdiObject::new(
        HGDIOBJ(unsafe { CreateFontIndirectW(&value) }.0),
        "content-font",
    )?;
    if config.ui_font_family != "auto" {
        let dc = MemoryDc(unsafe { CreateCompatibleDC(None) });
        anyhow::ensure!(!dc.0.is_invalid(), "font stage=content-dc failed");
        let selected = SavedDc::new(dc.0)?;
        selected.select(font.0)?;
        let mut name = [0u16; 128];
        let length = unsafe { GetTextFaceW(dc.0, Some(&mut name)) };
        anyhow::ensure!(length > 0, "font stage=content-face failed");
        let end = name
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(name.len());
        if !String::from_utf16_lossy(&name[..end]).eq_ignore_ascii_case(&config.ui_font_family) {
            warn!("font stage=content-family unavailable; using system font, INI unchanged");
            return Ok(system);
        }
    }
    Ok(font)
}

pub(crate) fn message_font_with_floor(dpi: u32, minimum_dip: u32) -> Result<OwnedGdiObject> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
            0,
            dpi,
        )
    }
    .context("gdi stage=system-message-font")?;
    metrics.lfMessageFont.lfHeight = -metrics
        .lfMessageFont
        .lfHeight
        .abs()
        .max((minimum_dip * dpi / 96) as i32);
    OwnedGdiObject::new(
        HGDIOBJ(unsafe { CreateFontIndirectW(&metrics.lfMessageFont) }.0),
        "system-message-font",
    )
}

#[cfg(test)]
mod content_font_tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{GetObjectW, LOGFONTW};

    #[test]
    fn hint_face_matches_content_at_each_size_and_dpi() {
        for size in [12, 20, 48] {
            for dpi in [96, 144, 192] {
                let content = content_font(
                    &crate::config::Config {
                        unified_font: true,
                        ui_font_family: "Arial".into(),
                        ui_font_size: size,
                        ..Default::default()
                    },
                    dpi,
                    0,
                )
                .unwrap();
                let hint = hint_font(&content).unwrap();
                let mut base = LOGFONTW::default();
                let mut small = LOGFONTW::default();
                for (object, metrics) in [(&content, &mut base), (&hint, &mut small)] {
                    assert_ne!(
                        unsafe {
                            GetObjectW(
                                object.0,
                                std::mem::size_of::<LOGFONTW>() as i32,
                                Some((metrics as *mut LOGFONTW).cast()),
                            )
                        },
                        0
                    );
                }
                assert_eq!(base.lfFaceName, small.lfFaceName);
                assert_eq!(small.lfHeight.abs(), base.lfHeight.abs() * 10 / 12);
                assert!(small.lfHeight.abs() < base.lfHeight.abs());
            }
        }
    }

    #[test]
    fn global_content_font_uses_dip_size_on_each_native_dpi() {
        let config = crate::config::Config {
            unified_font: true,
            ui_font_family: "Arial".into(),
            ui_font_size: 20,
            ..Default::default()
        };
        for dpi in [96, 144, 192] {
            let font = content_font(&config, dpi, 0).unwrap();
            let mut metrics = LOGFONTW::default();
            assert_ne!(
                unsafe {
                    GetObjectW(
                        font.0,
                        std::mem::size_of::<LOGFONTW>() as i32,
                        Some((&mut metrics as *mut LOGFONTW).cast()),
                    )
                },
                0
            );
            assert_eq!(metrics.lfHeight, -((20 * dpi / 96) as i32));
            let end = metrics
                .lfFaceName
                .iter()
                .position(|unit| *unit == 0)
                .unwrap();
            assert_eq!(
                String::from_utf16_lossy(&metrics.lfFaceName[..end]),
                "Arial"
            );
        }
    }
}

pub(crate) struct SavedDc(HDC, i32);

impl SavedDc {
    pub(crate) fn new(hdc: HDC) -> Result<Self> {
        let state = unsafe { SaveDC(hdc) };
        if state == 0 {
            bail!("gdi stage=save-dc failed");
        }
        Ok(Self(hdc, state))
    }

    pub(crate) fn select(&self, object: HGDIOBJ) -> Result<()> {
        if unsafe { SelectObject(self.0, object) }.is_invalid() {
            bail!("gdi stage=select-object failed");
        }
        Ok(())
    }
}

impl Drop for SavedDc {
    fn drop(&mut self) {
        if !unsafe { RestoreDC(self.0, self.1) }.as_bool() {
            warn!("gdi stage=restore-dc failed");
        }
    }
}

pub(crate) struct MemoryDc(pub(crate) HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        if !unsafe { DeleteDC(self.0) }.as_bool() {
            warn!("gdi stage=delete-dc failed");
        }
    }
}

pub(crate) fn checked_bitmap_bytes(width: i32, height: i32) -> Result<usize> {
    if width <= 0 || height <= 0 {
        bail!("gdi stage=bitmap-size nonpositive dimension");
    }
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("gdi stage=bitmap-size overflow")?;
    // A non-configurable allocation safety ceiling, not a rendering preference.
    if bytes > 256 * 1024 * 1024 {
        bail!("gdi stage=bitmap-size exceeds safety ceiling");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_allocations_and_gdi_results_are_rejected() {
        assert!(OwnedGdiObject::new(HGDIOBJ::default(), "injected").is_err());
        assert!(SavedDc::new(HDC::default()).is_err());
        for (width, height) in [(0, 16), (-1, 16), (i32::MAX, i32::MAX)] {
            assert!(checked_bitmap_bytes(width, height).is_err());
        }
        assert_eq!(checked_bitmap_bytes(16, 16).unwrap(), 1024);
    }
}
