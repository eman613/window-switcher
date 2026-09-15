use crate::{
    pixels::PixelImage,
    render_surface::RenderSurface,
    utils::{gdi::OwnedGdiObject, to_wstring},
};
use anyhow::{ensure, Context, Result};
use std::{
    mem::size_of,
    time::{Duration, Instant},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Graphics::Gdi::{GetObjectW, BITMAP},
        Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
        UI::{
            Controls::IImageList,
            Shell::{SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_SYSICONINDEX},
            WindowsAndMessaging::{
                CopyIcon, DestroyIcon, DrawIconEx, GetIconInfo, LoadIconW, SendMessageTimeoutW,
                DI_NORMAL, GCLP_HICON, HICON, ICONINFO, ICON_BIG, ICON_SMALL2, IDI_APPLICATION,
                SMTO_ABORTIFHUNG, WM_GETICON,
            },
        },
    },
};

pub(crate) struct OwnedIcon(pub(crate) HICON);
impl OwnedIcon {
    pub(crate) fn into_raw(self) -> HICON {
        let icon = self.0;
        std::mem::forget(self);
        icon
    }
}
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        if let Err(error) = unsafe { DestroyIcon(self.0) } {
            warn!("icon stage=destroy code={:#x}", error.code().0);
        }
    }
}

pub(crate) fn fallback() -> Option<OwnedIcon> {
    let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }.ok()?;
    unsafe { CopyIcon(icon) }.ok().map(OwnedIcon)
}

pub(crate) fn window_icon(hwnd: HWND, timeout: Duration) -> Option<OwnedIcon> {
    let started = Instant::now();
    for kind in [ICON_BIG, ICON_SMALL2] {
        let remaining = timeout
            .saturating_sub(started.elapsed())
            .as_millis()
            .min(u32::MAX as u128) as u32;
        if remaining == 0 {
            break;
        }
        let mut result = 0;
        let status = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                remaining,
                Some(&mut result),
            )
        };
        if status.0 != 0 && result != 0 {
            if let Ok(icon) = unsafe { CopyIcon(HICON(result as _)) } {
                return Some(OwnedIcon(icon));
            }
        }
        if kind == ICON_BIG {
            #[cfg(target_arch = "x86")]
            let icon =
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetClassLongW(hwnd, GCLP_HICON) };
            #[cfg(not(target_arch = "x86"))]
            let icon = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(hwnd, GCLP_HICON)
            };
            if icon != 0 {
                if let Ok(icon) = unsafe { CopyIcon(HICON(icon as _)) } {
                    return Some(OwnedIcon(icon));
                }
            }
        }
    }
    None
}

pub(crate) fn exe_icon(path: &str, allowed: impl Fn() -> bool) -> Option<OwnedIcon> {
    if !allowed() {
        return None;
    }
    let path = to_wstring(path);
    let mut info = SHFILEINFOW::default();
    if unsafe {
        SHGetFileInfoW(
            PCWSTR(path.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX,
        )
    } == 0
    {
        return None;
    }
    for size in [4, 2, 0] {
        // SHIL_JUMBO, SHIL_EXTRALARGE, SHIL_LARGE.
        if !allowed() {
            return None;
        }
        let Some(list) = (unsafe { SHGetImageList::<IImageList>(size) }).ok() else {
            continue;
        };
        let Some(icon) = (unsafe { list.GetIcon(info.iIcon, 1) }).ok().map(OwnedIcon) else {
            continue;
        };
        let Some(image) = rasterize(icon.0, 256).ok() else {
            continue;
        };
        if !topleft_only(&image) {
            return Some(icon);
        }
    }
    None
}

pub(crate) fn rasterize(icon: HICON, size: i32) -> Result<PixelImage> {
    let (width, height) = dimensions(icon).context("icon stage=dimensions unavailable")?;
    ensure!(
        (1..=1024).contains(&width) && (1..=1024).contains(&height) && (1..=1024).contains(&size),
        "icon stage=dimensions exceeds limit"
    );
    let scale = f64::from(size) / f64::from(width.max(height));
    let width = (f64::from(width) * scale).round().max(1.0) as i32;
    let height = (f64::from(height) * scale).round().max(1.0) as i32;
    let mut black = RenderSurface::new(size, size)?;
    let mut white = RenderSurface::new(size, size)?;
    for (surface, background) in [(&mut black, 0), (&mut white, 0xffffff)] {
        surface.fill_rgb(background)?;
        unsafe {
            DrawIconEx(
                surface.dc(),
                (size - width) / 2,
                (size - height) / 2,
                icon,
                width,
                height,
                0,
                None,
                DI_NORMAL,
            )
        }
        .context("icon stage=draw")?;
    }
    PixelImage::recover_alpha(black.pixels()?, white.pixels()?, size, size)
}

fn dimensions(icon: HICON) -> Option<(i32, i32)> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let color = OwnedGdiObject::new(info.hbmColor.into(), "icon-color").ok();
    let mask = OwnedGdiObject::new(info.hbmMask.into(), "icon-mask").ok();
    let object = color.as_ref().or(mask.as_ref())?;
    let mut bitmap = BITMAP::default();
    if unsafe {
        GetObjectW(
            object.0,
            size_of::<BITMAP>() as i32,
            Some(&mut bitmap as *mut _ as _),
        )
    } != size_of::<BITMAP>() as i32
    {
        return None;
    }
    Some((
        bitmap.bmWidth,
        if color.is_some() {
            bitmap.bmHeight
        } else {
            bitmap.bmHeight / 2
        },
    ))
}

fn topleft_only(image: &PixelImage) -> bool {
    let (mut left, mut top, mut right, mut bottom) = (image.width, image.height, -1, -1);
    for (position, pixel) in image.data.as_chunks::<4>().0.iter().enumerate() {
        if pixel[3] != 0 {
            let x = position as i32 % image.width;
            let y = position as i32 / image.width;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
    }
    right < 0
        || ((right - left + 1) * (bottom - top + 1) * 4 < image.width * image.height
            && ((left + right) as f32 / (2 * image.width) as f32) < 0.30
            && ((top + bottom) as f32 / (2 * image.height) as f32) < 0.30)
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
