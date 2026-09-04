use anyhow::{anyhow, Result};
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Gdi::{
            CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, HBITMAP,
            HBRUSH, HDC, HFONT, HGDIOBJ, HPALETTE, HRGN,
        },
        GdiPlus::{
            FillModeAlternate, GdipCreateBitmapFromHBITMAP, GdipCreateFromHDC, GdipCreatePath,
            GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath,
            GdipDisposeImage, GpBitmap, GpBrush, GpGraphics, GpImage, GpPath, GpSolidFill, Status,
        },
    },
};

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
            return Err(anyhow!("CreateCompatibleBitmap failed"));
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
