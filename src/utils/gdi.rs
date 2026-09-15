use anyhow::{bail, Context, Result};
use windows::Win32::Graphics::Gdi::{
    DeleteDC, DeleteObject, RestoreDC, SaveDC, SelectObject, HDC, HGDIOBJ,
};

pub(crate) struct OwnedGdiObject(pub(crate) HGDIOBJ);

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
