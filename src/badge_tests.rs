use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use windows::Win32::{
    Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetBkMode,
        GetCurrentObject, GetTextColor, SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE, HBITMAP, HGDIOBJ, OBJ_FONT, SRCCOPY,
    },
    System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS},
};

use super::*;

#[test]
fn badge_count_contract_is_preserved() {
    for count in [0, 1] {
        assert_eq!(format_badge_count(count, 99), None);
    }
    for (count, maximum, expected) in [
        (2, 99, "2"),
        (99, 99, "99"),
        (100, 99, "99+"),
        (10_000, 9999, "9999+"),
        (3, 0, "2+"),
        (10_000, 20_000, "9999+"),
    ] {
        assert_eq!(
            format_badge_count(count, maximum).as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn rgb_configuration_is_converted_to_gdi_color_order() {
    assert_eq!(rgb_colorref(0x4c7094).0, 0x94704c);
    assert_eq!(rgb_colorref(0xffffff).0, 0xffffff);
}

#[test]
fn native_badges_are_circular_readable_and_restore_gdi_state() {
    let mut style = BadgeStyle::from_config(&Config::default());
    for size in [64 * 6, 96 * 6, 128 * 6] {
        let canvas = TestCanvas::new(size, size);
        let font = unsafe { GetCurrentObject(canvas.hdc, OBJ_FONT) };
        let text_color = unsafe { GetTextColor(canvas.hdc) };
        let background_mode = unsafe { GetBkMode(canvas.hdc) };
        for font_size in [8, 12, 24] {
            style.font_size = font_size;
            for (count, maximum) in [(2, 99), (99, 99), (100, 99), (10_000, 9999)] {
                canvas.clear(0xa5a5a5);
                draw_badge(canvas.hdc, count, maximum, canvas.bounds(), style).unwrap();
                assert_eq!(unsafe { GetCurrentObject(canvas.hdc, OBJ_FONT) }, font);
                assert_eq!(unsafe { GetTextColor(canvas.hdc) }, text_color);
                assert_eq!(unsafe { GetBkMode(canvas.hdc) }, background_mode);
                assert_circle(&canvas, style, 0xa5a5a5);
            }
        }
    }
    style.font_size = 12;
    let canvas = TestCanvas::new(384, 384);
    assert!(draw_badge(HDC::default(), 2, 99, canvas.bounds(), style).is_err());

    if let Some(path) = std::env::var_os("WINDOW_SWITCHER_BADGE_PREVIEW") {
        export_preview(Path::new(&path), style);
    }
}

#[test]
#[ignore = "process-wide GDI counters require an isolated serial process"]
fn native_badge_resources_remain_bounded() {
    let style = BadgeStyle::from_config(&Config::default());
    let canvas = TestCanvas::new(384, 384);
    draw_badge(canvas.hdc, 100, 99, canvas.bounds(), style).unwrap();
    let before = unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
    assert!(before > 0, "GDI resource counter is unavailable");
    for _ in 0..200 {
        draw_badge(canvas.hdc, 100, 99, canvas.bounds(), style).unwrap();
    }
    let after = unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
    eprintln!("badge_resource_cycles=200 initial_gdi={before} final_gdi={after}");
    assert!(
        after <= before + 1,
        "Badge GDI objects grew from {before} to {after}"
    );
}

fn assert_circle(canvas: &TestCanvas, style: BadgeStyle, background: u32) {
    let pixels = canvas.pixels();
    let mut left = canvas.width;
    let mut top = canvas.height;
    let mut right = 0;
    let mut bottom = 0;
    let mut has_background = false;
    let mut has_text = false;
    for (position, pixel) in pixels.iter().enumerate() {
        let color = pixel & 0xffffff;
        has_background |= color == style.background;
        has_text |= color == style.foreground;
        if color != background {
            let x = position as i32 % canvas.width;
            let y = position as i32 / canvas.width;
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
    }
    assert!(
        has_background && has_text,
        "Badge background or text was not rendered"
    );
    assert!(
        ((right - left) - (bottom - top)).abs() <= 1,
        "Badge is not circular"
    );
    assert!(left >= 0 && top >= 0 && right < canvas.width && bottom < canvas.height);
    assert_eq!(
        pixels[(top * canvas.width + left) as usize] & 0xffffff,
        background,
        "circle corner must be transparent"
    );
    let radius = f64::from(right - left + 1) / 2.0;
    let center_x = f64::from(left + right + 1) / 2.0;
    let center_y = f64::from(top + bottom + 1) / 2.0;
    for y in top..=bottom {
        for x in left..=right {
            if pixels[(y * canvas.width + x) as usize] & 0xffffff != background {
                let distance = (f64::from(x) + 0.5 - center_x).hypot(f64::from(y) + 0.5 - center_y);
                assert!(distance <= radius + 1.0, "text or fill escaped the circle");
            }
        }
    }
}

struct TestCanvas {
    hdc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    data: *mut u32,
    width: i32,
    height: i32,
}

impl TestCanvas {
    fn new(width: i32, height: i32) -> Self {
        let hdc = unsafe { CreateCompatibleDC(None) };
        assert!(!hdc.is_invalid());
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut data = std::ptr::null_mut();
        let bitmap =
            match unsafe { CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut data, None, 0) }
            {
                Ok(bitmap) => bitmap,
                Err(err) => {
                    unsafe {
                        let _ = DeleteDC(hdc);
                    }
                    panic!("create badge test bitmap: {err}");
                }
            };
        let previous_bitmap = unsafe { SelectObject(hdc, bitmap.into()) };
        Self {
            hdc,
            bitmap,
            previous_bitmap,
            data: data.cast(),
            width,
            height,
        }
    }

    fn bounds(&self) -> RECT {
        RECT {
            left: 0,
            top: 0,
            right: self.width,
            bottom: self.height,
        }
    }

    fn clear(&self, color: u32) {
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts_mut(self.data, (self.width * self.height) as usize)
                .fill(color);
        }
    }

    fn pixels(&self) -> Vec<u32> {
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts(self.data, (self.width * self.height) as usize).to_vec()
        }
    }
}

impl Drop for TestCanvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.hdc, self.previous_bitmap);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.hdc);
        }
    }
}

fn export_preview(path: &Path, style: BadgeStyle) {
    let atlas = TestCanvas::new(512 * 6, 576 * 6);
    atlas.clear(0xe0e0e0);
    let mut top = 0;
    for background in [0xe0e0e0, 0x4c4c4c] {
        for size in [64 * 6, 96 * 6, 128 * 6] {
            let row = RECT {
                left: 0,
                top,
                right: atlas.width,
                bottom: top + size,
            };
            let brush = unsafe { CreateSolidBrush(rgb_colorref(background)) };
            let _brush = OwnedGdiObject::new(brush.into(), "preview brush").unwrap();
            unsafe { windows::Win32::Graphics::Gdi::FillRect(atlas.hdc, &row, brush) };
            for (column, (count, maximum)) in [(2, 99), (99, 99), (100, 99), (10_000, 9999)]
                .into_iter()
                .enumerate()
            {
                let left = column as i32 * 128 * 6;
                draw_badge(
                    atlas.hdc,
                    count,
                    maximum,
                    RECT {
                        left,
                        top,
                        right: left + size,
                        bottom: top + size,
                    },
                    style,
                )
                .unwrap();
            }
            top += size;
        }
    }
    let preview = TestCanvas::new(512, 576);
    unsafe {
        SetStretchBltMode(preview.hdc, HALFTONE);
        StretchBlt(
            preview.hdc,
            0,
            0,
            preview.width,
            preview.height,
            Some(atlas.hdc),
            0,
            0,
            atlas.width,
            atlas.height,
            SRCCOPY,
        )
        .unwrap();
    }
    write_bitmap(path, &preview);
}

fn write_bitmap(path: &Path, canvas: &TestCanvas) {
    let pixels: Vec<u8> = canvas
        .pixels()
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut writer = BufWriter::new(File::create(path).unwrap());
    writer.write_all(b"BM").unwrap();
    for value in [
        54 + pixels.len() as u32,
        0,
        54,
        40,
        canvas.width as u32,
        (-canvas.height) as u32,
    ] {
        writer.write_all(&value.to_le_bytes()).unwrap();
    }
    writer.write_all(&1u16.to_le_bytes()).unwrap();
    writer.write_all(&32u16.to_le_bytes()).unwrap();
    for value in [0, pixels.len() as u32, 2835, 2835, 0, 0] {
        writer.write_all(&value.to_le_bytes()).unwrap();
    }
    writer.write_all(&pixels).unwrap();
    writer.flush().unwrap();
}
