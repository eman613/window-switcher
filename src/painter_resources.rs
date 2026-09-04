use anyhow::{anyhow, Result};
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, HPALETTE, HRGN,
        },
        GdiPlus::{
            ColorAdjustTypeBitmap, FillModeAlternate, GdipCreateBitmapFromHBITMAP,
            GdipCreateFromHDC, GdipCreateImageAttributes, GdipCreatePath, GdipCreateSolidFill,
            GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipDisposeImage,
            GdipDisposeImageAttributes, GdipSetImageAttributesColorKeys, GpBitmap, GpBrush,
            GpGraphics, GpImage, GpImageAttributes, GpPath, GpSolidFill, Status,
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

/// Reusable memory DC with a selected compatible bitmap.
///
/// Field order is intentional: the selected object is restored before the
/// bitmap and the DC are destroyed.
pub(super) struct BitmapSurface {
    _selection: SelectedObjectGuard,
    bitmap: BitmapGuard,
    dc: MemoryDcGuard,
    width: i32,
    height: i32,
}

impl BitmapSurface {
    pub(super) fn new(reference: HDC, width: i32, height: i32) -> Result<Self> {
        if width <= 0 || height <= 0 {
            return Err(anyhow!("Invalid bitmap surface dimensions"));
        }
        let dc = MemoryDcGuard::new(reference)?;
        let bitmap = BitmapGuard::new(unsafe { CreateCompatibleBitmap(reference, width, height) })?;
        let selection = SelectedObjectGuard::new(dc.get(), bitmap.get().into())?;
        Ok(Self {
            _selection: selection,
            bitmap,
            dc,
            width,
            height,
        })
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

pub(super) struct GpImageAttributesGuard(*mut GpImageAttributes);

impl GpImageAttributesGuard {
    pub(super) fn with_color_key(argb: u32) -> Result<Self> {
        let mut ptr = std::ptr::null_mut();
        gdiplus_status(
            unsafe { GdipCreateImageAttributes(&mut ptr) },
            "GdipCreateImageAttributes",
        )?;
        if ptr.is_null() {
            return Err(anyhow!("GdipCreateImageAttributes returned a null pointer"));
        }

        let attributes = Self(ptr);
        gdiplus_status(
            unsafe {
                GdipSetImageAttributesColorKeys(
                    attributes.get(),
                    ColorAdjustTypeBitmap,
                    true,
                    argb,
                    argb,
                )
            },
            "GdipSetImageAttributesColorKeys",
        )?;
        Ok(attributes)
    }

    pub(super) fn get(&self) -> *mut GpImageAttributes {
        self.0
    }
}

impl Drop for GpImageAttributesGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = GdipDisposeImageAttributes(self.0);
            }
        }
    }
}
