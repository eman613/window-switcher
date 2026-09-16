//! DirectWrite glyph runs rendered with grayscale coverage into premultiplied pixels.
use anyhow::{ensure, Result};
use std::ffi::c_void;
use windows::{
    core::{implement, IUnknown, IUnknownImpl, Interface, Ref, BOOL},
    Win32::{
        Foundation::{COLORREF, E_FAIL, E_POINTER, RECT},
        Graphics::{
            DirectWrite::*,
            Gdi::{BitBlt, CreateSolidBrush, FillRect, HDC, SRCCOPY},
        },
    },
};

use crate::{pixels::PixelImage, render_surface::RenderSurface, utils::gdi::OwnedGdiObject};

pub(crate) fn rasterize(
    factory: &IDWriteFactory,
    layout: &IDWriteTextLayout,
    width: i32,
    height: i32,
    color: u32,
) -> Result<PixelImage> {
    rasterize_at(factory, layout, width, height, color, (0.0, 0.0))
}

pub(crate) fn rasterize_at(
    factory: &IDWriteFactory,
    layout: &IDWriteTextLayout,
    width: i32,
    height: i32,
    color: u32,
    origin: (f32, f32),
) -> Result<PixelImage> {
    ensure!(width > 0 && height > 0, "font stage=raster dimensions");
    ensure!(
        origin.0.is_finite() && origin.1.is_finite(),
        "font stage=raster origin"
    );
    let mut black = RenderSurface::new(width, height)?;
    let mut white = RenderSurface::new(width, height)?;
    let interop = unsafe { factory.GetGdiInterop() }?;
    let target = unsafe { interop.CreateBitmapRenderTarget(None, width as u32, height as u32) }?;
    unsafe {
        target.SetPixelsPerDip(1.0)?;
        target
            .cast::<IDWriteBitmapRenderTarget1>()?
            .SetTextAntialiasMode(DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE)?;
    }
    let rendering = unsafe {
        factory.CreateCustomRenderingParams(
            1.0,
            0.0,
            0.0,
            DWRITE_PIXEL_GEOMETRY_FLAT,
            DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
        )
    }?;
    let renderer: IDWriteTextRenderer = GlyphRenderer {
        target: target.clone(),
        rendering,
        color: colorref(color),
    }
    .into();
    let rectangle = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    for (surface, background) in [(&mut black, 0), (&mut white, 0xffffff)] {
        let dc = unsafe { target.GetMemoryDC() };
        fill(dc, rectangle, colorref(background))?;
        unsafe {
            layout.Draw(None, &renderer, origin.0, origin.1)?;
            BitBlt(surface.dc(), 0, 0, width, height, Some(dc), 0, 0, SRCCOPY)?;
        }
    }
    PixelImage::recover_alpha(black.pixels()?, white.pixels()?, width, height)
}

pub(crate) const fn colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff))
}

fn fill(dc: HDC, rectangle: RECT, color: COLORREF) -> windows::core::Result<()> {
    let brush = unsafe { CreateSolidBrush(color) };
    let _owned = OwnedGdiObject::new(brush.into(), "font-brush")
        .map_err(|_| windows::core::Error::from(E_FAIL))?;
    if unsafe { FillRect(dc, &rectangle, brush) } == 0 {
        return Err(E_FAIL.into());
    }
    Ok(())
}

#[implement(IDWriteTextRenderer)]
struct GlyphRenderer {
    target: IDWriteBitmapRenderTarget,
    rendering: IDWriteRenderingParams,
    color: COLORREF,
}

impl IDWritePixelSnapping_Impl for GlyphRenderer_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> windows::core::Result<BOOL> {
        Ok(false.into())
    }
    fn GetCurrentTransform(
        &self,
        _: *const c_void,
        transform: *mut DWRITE_MATRIX,
    ) -> windows::core::Result<()> {
        if transform.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.0,
                m22: 1.0,
                ..Default::default()
            };
        }
        Ok(())
    }
    fn GetPixelsPerDip(&self, _: *const c_void) -> windows::core::Result<f32> {
        Ok(1.0)
    }
}

impl IDWriteTextRenderer_Impl for GlyphRenderer_Impl {
    fn DrawGlyphRun(
        &self,
        _: *const c_void,
        x: f32,
        y: f32,
        mode: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        _: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        if run.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            self.target
                .DrawGlyphRun(x, y, mode, run, &self.rendering, self.color, None)
        }
    }
    fn DrawUnderline(
        &self,
        _: *const c_void,
        x: f32,
        y: f32,
        line: *const DWRITE_UNDERLINE,
        _: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        let line = unsafe { line.as_ref() }.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
        self.line(x, y + line.offset, line.width, line.thickness)
    }
    fn DrawStrikethrough(
        &self,
        _: *const c_void,
        x: f32,
        y: f32,
        line: *const DWRITE_STRIKETHROUGH,
        _: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        let line = unsafe { line.as_ref() }.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
        self.line(x, y + line.offset, line.width, line.thickness)
    }
    fn DrawInlineObject(
        &self,
        context: *const c_void,
        x: f32,
        y: f32,
        object: Ref<'_, IDWriteInlineObject>,
        sideways: BOOL,
        rtl: BOOL,
        effect: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        let renderer: IDWriteTextRenderer = self.to_interface();
        unsafe {
            object.ok()?.Draw(
                Some(context),
                &renderer,
                x,
                y,
                sideways.as_bool(),
                rtl.as_bool(),
                effect.as_ref(),
            )
        }
    }
}

impl GlyphRenderer_Impl {
    fn line(&self, x: f32, y: f32, width: f32, height: f32) -> windows::core::Result<()> {
        fill(
            unsafe { self.target.GetMemoryDC() },
            RECT {
                left: x.floor() as i32,
                top: y.floor() as i32,
                right: (x + width).ceil() as i32,
                bottom: (y + height.max(1.0)).ceil() as i32,
            },
            self.color,
        )
    }
}
