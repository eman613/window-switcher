use anyhow::{anyhow, Result};
use std::{ffi::c_void, mem::size_of, ptr::NonNull};
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HBRUSH, HDC,
            HFONT, HGDIOBJ, HPALETTE, HRGN,
        },
        GdiPlus::{
            FillModeAlternate, GdipCreateBitmapFromHBITMAP, GdipCreateFromHDC, GdipCreatePath,
            GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath,
            GdipDisposeImage, GpBitmap, GpBrush, GpGraphics, GpImage, GpPath, GpSolidFill, Status,
        },
    },
};

pub(super) const MAX_BITMAP_BYTES: usize = 128 * 1024 * 1024;
pub(super) const MAX_SURFACE_BYTES: usize = 256 * 1024 * 1024;

pub(super) fn bitmap_byte_len(width: i32, height: i32) -> Result<usize> {
    if width <= 0 || height <= 0 {
        return Err(anyhow!("Bitmap dimensions must be positive"));
    }
    let width = usize::try_from(width).map_err(|_| anyhow!("Bitmap width is invalid"))?;
    let height = usize::try_from(height).map_err(|_| anyhow!("Bitmap height is invalid"))?;
    let bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("Bitmap byte size overflow"))?;
    if bytes > MAX_BITMAP_BYTES {
        return Err(anyhow!(
            "Bitmap allocation of {bytes} bytes exceeds the {MAX_BITMAP_BYTES}-byte limit"
        ));
    }
    Ok(bytes)
}

pub(super) fn gdiplus_status(status: Status, operation: &str) -> Result<()> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(anyhow!("{operation} failed with GDI+ status {}", status.0))
    }
}

pub(super) struct ScreenDcGuard {
    hwnd: HWND,
    handle: HDC,
}

impl ScreenDcGuard {
    pub(super) fn new(hwnd: HWND) -> Result<Self> {
        let handle = unsafe { GetDC(Some(hwnd)) };
        if handle.is_invalid() {
            return Err(anyhow!("GetDC failed"));
        }
        Ok(Self { hwnd, handle })
    }

    pub(super) fn get(&self) -> HDC {
        self.handle
    }
}

impl Drop for ScreenDcGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseDC(Some(self.hwnd), self.handle);
        }
    }
}

pub(super) struct MemoryDcGuard(HDC);

impl MemoryDcGuard {
    pub(super) fn new(reference: HDC) -> Result<Self> {
        let handle = unsafe { CreateCompatibleDC(Some(reference)) };
        if handle.is_invalid() {
            return Err(anyhow!("CreateCompatibleDC failed"));
        }
        Ok(Self(handle))
    }

    pub(super) fn get(&self) -> HDC {
        self.0
    }
}

impl Drop for MemoryDcGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

pub(super) struct BitmapGuard {
    handle: HBITMAP,
}

impl BitmapGuard {
    pub(super) fn new(handle: HBITMAP) -> Result<Self> {
        if handle.is_invalid() {
            return Err(anyhow!("CreateDIBSection failed"));
        }
        Ok(Self { handle })
    }

    pub(super) fn get(&self) -> HBITMAP {
        self.handle
    }
}

impl Drop for BitmapGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.handle.into());
        }
    }
}

pub(super) struct BrushGuard(HBRUSH);

impl BrushGuard {
    pub(super) fn new(handle: HBRUSH) -> Result<Self> {
        if handle.is_invalid() {
            return Err(anyhow!("CreateSolidBrush failed"));
        }
        Ok(Self(handle))
    }

    pub(super) fn get(&self) -> HBRUSH {
        self.0
    }
}

impl Drop for BrushGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

pub(super) struct FontGuard(HFONT);

impl FontGuard {
    pub(super) fn new(handle: HFONT) -> Result<Self> {
        if handle.is_invalid() {
            return Err(anyhow!("CreateFontW failed"));
        }
        Ok(Self(handle))
    }

    pub(super) fn get(&self) -> HFONT {
        self.0
    }
}

impl Drop for FontGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

pub(super) struct RegionGuard(HRGN);

impl RegionGuard {
    pub(super) fn new(handle: HRGN) -> Result<Self> {
        if handle.is_invalid() {
            return Err(anyhow!("CreateRoundRectRgn failed"));
        }
        Ok(Self(handle))
    }

    pub(super) fn get(&self) -> HRGN {
        self.0
    }
}

impl Drop for RegionGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

pub(super) struct SelectedObjectGuard {
    dc: HDC,
    previous: HGDIOBJ,
}

impl SelectedObjectGuard {
    pub(super) fn new(dc: HDC, object: HGDIOBJ) -> Result<Self> {
        let previous = unsafe { SelectObject(dc, object) };
        if previous.is_invalid() {
            return Err(anyhow!("SelectObject failed"));
        }
        Ok(Self { dc, previous })
    }
}

impl Drop for SelectedObjectGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
        }
    }
}

/// Reusable memory DC with a selected compatible bitmap.
///
/// Field order is intentional: the selected object is restored before the
/// bitmap and the DC are destroyed.
pub(super) struct BitmapSurface {
    _selection: SelectedObjectGuard,
    bitmap: BitmapGuard,
    dc: MemoryDcGuard,
    bits: NonNull<u8>,
    byte_len: usize,
    width: i32,
    height: i32,
}

impl BitmapSurface {
    pub(super) fn new(reference: HDC, width: i32, height: i32) -> Result<Self> {
        if width <= 0 || height <= 0 {
            return Err(anyhow!("Invalid bitmap surface dimensions"));
        }
        let byte_len = bitmap_byte_len(width, height)?;
        let dc = MemoryDcGuard::new(reference)?;
        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut::<c_void>();
        let bitmap = BitmapGuard::new(
            unsafe {
                CreateDIBSection(
                    Some(reference),
                    &bitmap_info,
                    DIB_RGB_COLORS,
                    &mut bits,
                    None,
                    0,
                )
            }
            .map_err(|err| anyhow!("CreateDIBSection failed, {err}"))?,
        )?;
        let bits = NonNull::new(bits.cast::<u8>())
            .ok_or_else(|| anyhow!("CreateDIBSection returned null pixel storage"))?;
        let selection = SelectedObjectGuard::new(dc.get(), bitmap.get().into())?;
        let surface = Self {
            _selection: selection,
            bitmap,
            dc,
            bits,
            byte_len,
            width,
            height,
        };
        surface.clear();
        Ok(surface)
    }

    pub(super) fn matches(&self, width: i32, height: i32) -> bool {
        self.width == width && self.height == height
    }

    pub(super) fn dc(&self) -> HDC {
        self.dc.get()
    }

    pub(super) fn bitmap(&self) -> HBITMAP {
        self.bitmap.get()
    }

    pub(super) fn clear(&self) {
        unsafe { std::ptr::write_bytes(self.bits.as_ptr(), 0, self.byte_len) };
    }
}

pub(super) struct GpGraphicsGuard(*mut GpGraphics);

impl GpGraphicsGuard {
    pub(super) fn new(hdc: HDC) -> Result<Self> {
        let mut ptr = std::ptr::null_mut();
        gdiplus_status(
            unsafe { GdipCreateFromHDC(hdc, &mut ptr) },
            "GdipCreateFromHDC",
        )?;
        if ptr.is_null() {
            return Err(anyhow!("GdipCreateFromHDC returned a null pointer"));
        }
        Ok(Self(ptr))
    }

    pub(super) fn get(&self) -> *mut GpGraphics {
        self.0
    }
}

impl Drop for GpGraphicsGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = GdipDeleteGraphics(self.0);
            }
        }
    }
}

pub(super) struct GpBrushGuard(*mut GpBrush);

impl GpBrushGuard {
    pub(super) fn new(color: u32) -> Result<Self> {
        let mut ptr: *mut GpSolidFill = std::ptr::null_mut();
        gdiplus_status(
            unsafe { GdipCreateSolidFill(color, &mut ptr) },
            "GdipCreateSolidFill",
        )?;
        if ptr.is_null() {
            return Err(anyhow!("GdipCreateSolidFill returned a null pointer"));
        }
        Ok(Self(ptr as *mut GpBrush))
    }

    pub(super) fn get(&self) -> *mut GpBrush {
        self.0
    }
}

impl Drop for GpBrushGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = GdipDeleteBrush(self.0);
            }
        }
    }
}

pub(super) struct GpPathGuard(*mut GpPath);

impl GpPathGuard {
    pub(super) fn new() -> Result<Self> {
        let mut ptr = std::ptr::null_mut();
        gdiplus_status(
            unsafe { GdipCreatePath(FillModeAlternate, &mut ptr) },
            "GdipCreatePath",
        )?;
        if ptr.is_null() {
            return Err(anyhow!("GdipCreatePath returned a null pointer"));
        }
        Ok(Self(ptr))
    }

    pub(super) fn get(&self) -> *mut GpPath {
        self.0
    }
}

impl Drop for GpPathGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = GdipDeletePath(self.0);
            }
        }
    }
}

pub(super) struct GpImageGuard(*mut GpImage);

impl GpImageGuard {
    pub(super) fn from_hbitmap(bitmap: HBITMAP) -> Result<Self> {
        let mut ptr: *mut GpBitmap = std::ptr::null_mut();
        gdiplus_status(
            unsafe { GdipCreateBitmapFromHBITMAP(bitmap, HPALETTE::default(), &mut ptr) },
            "GdipCreateBitmapFromHBITMAP",
        )?;
        if ptr.is_null() {
            return Err(anyhow!(
                "GdipCreateBitmapFromHBITMAP returned a null pointer"
            ));
        }
        Ok(Self(ptr as *mut GpImage))
    }

    pub(super) fn get(&self) -> *mut GpImage {
        self.0
    }
}

impl Drop for GpImageGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = GdipDisposeImage(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_byte_len_enforces_dimensions_and_budget() {
        assert_eq!(bitmap_byte_len(1, 1).unwrap(), 4);
        assert!(bitmap_byte_len(0, 1).is_err());
        assert!(bitmap_byte_len(-1, 1).is_err());
        assert!(bitmap_byte_len(16_384, 16_384).is_err());
    }
}
