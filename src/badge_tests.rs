use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS};

use super::*;
use crate::{font_resources::FontResources, layout::PixelRect, pixels::PixelImage};

const COUNTS: [(usize, u32); 8] = [
    (0, 99),
    (1, 99),
    (2, 99),
    (8, 99),
    (11, 99),
    (99, 99),
    (100, 99),
    (10_000, 9999),
];
const SHAPES: [BadgeShape; 2] = [BadgeShape::Circle, BadgeShape::Square];
const SIZES: [Option<u32>; 4] = [None, Some(16), Some(24), Some(48)];

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
    assert_eq!(crate::text_raster::colorref(0x4c7094).0, 0x94704c);
    assert_eq!(crate::text_raster::colorref(0xffffff).0, 0xffffff);
}

#[test]
fn native_badges_fit_shapes_sizes_and_center_visible_ink() {
    let mut cases = 0;
    for family in [None, Some("Segoe UI"), Some("Arial")] {
        let mut fonts = system_fonts(family);
        for icon_size in [24, 64, 96, 128] {
            for shape in SHAPES {
                for size in SIZES {
                    for font_size in [8, 12, 24] {
                        let style = BadgeStyle {
                            background: 0,
                            foreground: 0xffffff,
                            font_size,
                            shape,
                            size,
                        };
                        for (count, maximum) in COUNTS {
                            let image = render(fonts.as_mut(), count, maximum, icon_size, style);
                            assert_badge(&image, count, style);
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    eprintln!("badge_native_cases={cases} ink_center_tolerance_px=0.5");
    if let Some(directory) = std::env::var_os("WINDOW_SWITCHER_BADGE_PREVIEW") {
        export_previews(Path::new(&directory));
    }
}

#[test]
fn supersampling_preserves_centering_after_final_downsample() {
    let mut fonts = system_fonts(Some("Segoe UI"));
    for icon_size in [64, 96, 128] {
        for scale in [1, 2, 4, 6] {
            for shape in SHAPES {
                for size in SIZES {
                    let style = BadgeStyle {
                        background: 0,
                        foreground: 0xffffff,
                        shape,
                        size,
                        ..BadgeStyle::from_config(&Config::default())
                    };
                    for (count, maximum) in [(2, 99), (99, 99), (100, 99), (10_000, 9999)] {
                        let image =
                            render(fonts.as_mut(), count, maximum, icon_size * scale, style)
                                .downsample(scale)
                                .unwrap();
                        assert_badge(&image, count, style);
                    }
                }
            }
        }
    }
}

#[test]
fn auto_circle_keeps_the_original_default_size_and_small_icons_are_skipped() {
    for family in [None, Some("Segoe UI")] {
        let mut fonts = system_fonts(family);
        let style = BadgeStyle::from_config(&Config::default());
        let image = render(fonts.as_mut(), 2, 99, 64, style);
        let bounds = pixel_bounds(&image, |pixel| pixel[3] != 0).unwrap();
        assert_eq!((bounds.width(), bounds.height()), (24, 24));
        assert_eq!((bounds.left, bounds.top), (38, 2));
        let tiny = render(fonts.as_mut(), 2, 99, 3, style);
        assert!(tiny.data.iter().all(|value| *value == 0));
    }
}

#[test]
#[ignore = "process-wide GDI counters require an isolated serial process"]
fn native_badge_resources_remain_bounded() {
    let mut fonts = system_fonts(Some("Segoe UI"));
    let draw = |fonts: &mut Option<FontResources>, cycle: usize| {
        let style = BadgeStyle {
            shape: SHAPES[cycle % SHAPES.len()],
            size: SIZES[cycle % SIZES.len()],
            ..BadgeStyle::from_config(&Config::default())
        };
        for directwrite in [false, true] {
            let _image = render(
                if directwrite { fonts.as_mut() } else { None },
                100,
                99,
                384,
                style,
            );
        }
    };
    for cycle in 0..SIZES.len() {
        draw(&mut fonts, cycle);
    }
    let before = unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
    assert!(before > 0, "GDI resource counter is unavailable");
    for cycle in 0..200 {
        draw(&mut fonts, cycle);
    }
    let after = unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
    eprintln!("badge_resource_cycles=200 initial_gdi={before} final_gdi={after}");
    assert!(
        after <= before + 1,
        "Badge GDI objects grew from {before} to {after}"
    );
}

fn system_fonts(family: Option<&str>) -> Option<FontResources> {
    family.map(|family| {
        FontResources::load(
            &Config {
                badge_font_family: family.into(),
                ..Default::default()
            },
            Path::new("."),
        )
        .unwrap()
    })
}

fn render(
    fonts: Option<&mut FontResources>,
    count: usize,
    maximum: u32,
    icon_size: i32,
    style: BadgeStyle,
) -> PixelImage {
    let mut image = PixelImage::new(icon_size, icon_size).unwrap();
    compose(
        &mut image,
        fonts,
        count,
        maximum,
        PixelRect {
            left: 0,
            top: 0,
            right: icon_size,
            bottom: icon_size,
        },
        style,
    )
    .unwrap_or_else(|error| panic!("icon={icon_size} count={count} style={style:?}: {error:#}"));
    image
}

fn pixel_bounds(image: &PixelImage, include: impl Fn(&[u8; 4]) -> bool) -> Option<PixelRect> {
    image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, pixel)| include(pixel))
        .map(|(index, _)| {
            let x = index as i32 % image.width;
            let y = index as i32 / image.width;
            PixelRect {
                left: x,
                top: y,
                right: x + 1,
                bottom: y + 1,
            }
        })
        .reduce(|a, b| PixelRect {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        })
}

fn assert_badge(image: &PixelImage, count: usize, style: BadgeStyle) {
    let bounds = pixel_bounds(image, |pixel| pixel[3] != 0);
    if count <= 1 {
        assert!(bounds.is_none());
        return;
    }
    let bounds = bounds.expect("Badge background is absent");
    // White glyphs on a black Badge make the actual glyph coverage observable,
    // independent of background alpha and of the production centering helper.
    let ink = pixel_bounds(image, |pixel| pixel[0] != 0).expect("Badge text is absent");
    assert_eq!(
        bounds.width(),
        bounds.height(),
        "Badge became an ellipse or rectangle"
    );
    for (ink_edges, shape_edges) in [
        (ink.left + ink.right, bounds.left + bounds.right),
        (ink.top + ink.bottom, bounds.top + bounds.bottom),
    ] {
        assert!((ink_edges - shape_edges).abs() <= 1,
            "ink is not centered: image={} count={count} style={style:?} badge={bounds:?} ink={ink:?}",
            image.width);
    }
    let inset = (image.width / 32).max(1);
    assert_eq!(bounds.top, inset);
    assert_eq!(bounds.right, image.width - inset);
    if let Some(size) = style.size {
        let expected = ((size as i32 * image.width + 32) / 64).min(image.width - 2 * inset);
        assert_eq!(bounds.width(), expected, "explicit size changed with text");
    }
    let corner = ((bounds.top * image.width + bounds.left) * 4) as usize;
    match style.shape {
        // A fractional outer edge may have partial coverage after downsampling.
        BadgeShape::Square => assert!(image.data[corner + 3] > 0),
        BadgeShape::Circle => {
            assert_eq!(image.data[corner + 3], 0);
            let radius = f64::from(bounds.width()) / 2.0;
            let cx = f64::from(bounds.left + bounds.right) / 2.0;
            let cy = f64::from(bounds.top + bounds.bottom) / 2.0;
            for y in ink.top..ink.bottom {
                for x in ink.left..ink.right {
                    let pixel = ((y * image.width + x) * 4) as usize;
                    if image.data[pixel] != 0 {
                        assert!(
                            (f64::from(x) + 0.5 - cx).hypot(f64::from(y) + 0.5 - cy) <= radius,
                            "glyph escaped circle"
                        );
                    }
                }
            }
        }
    }
}

fn export_previews(directory: &Path) {
    std::fs::create_dir_all(directory).unwrap();
    for (name, family) in [
        ("segoe", Some("Segoe UI")),
        ("arial", Some("Arial")),
        ("gdi", None),
    ] {
        let mut fonts = system_fonts(family);
        for icon_size in [64, 96, 128] {
            let mut atlas = PixelImage::new(icon_size * 6, icon_size * 8).unwrap();
            for (shape_index, shape) in SHAPES.into_iter().enumerate() {
                for (size_index, size) in SIZES.into_iter().enumerate() {
                    let row = (shape_index * SIZES.len() + size_index) as i32;
                    atlas.rounded_fill(
                        PixelRect {
                            left: 0,
                            top: row * icon_size,
                            right: atlas.width,
                            bottom: (row + 1) * icon_size,
                        },
                        0.0,
                        if row % 2 == 0 { 0xe0e0e0 } else { 0x303030 },
                    );
                    let style = BadgeStyle {
                        shape,
                        size,
                        ..BadgeStyle::from_config(&Config::default())
                    };
                    for (column, (count, maximum)) in COUNTS[2..].iter().copied().enumerate() {
                        let image = render(fonts.as_mut(), count, maximum, icon_size * 6, style)
                            .downsample(6)
                            .unwrap();
                        atlas
                            .compose(&image, column as i32 * icon_size, row * icon_size)
                            .unwrap();
                    }
                }
            }
            write_bitmap(&directory.join(format!("{name}-{icon_size}.bmp")), &atlas);
        }
    }
}

fn write_bitmap(path: &Path, image: &PixelImage) {
    let mut writer = BufWriter::new(File::create(path).unwrap());
    writer.write_all(b"BM").unwrap();
    for value in [
        54 + image.data.len() as u32,
        0,
        54,
        40,
        image.width as u32,
        (-image.height) as u32,
    ] {
        writer.write_all(&value.to_le_bytes()).unwrap();
    }
    writer.write_all(&1u16.to_le_bytes()).unwrap();
    writer.write_all(&32u16.to_le_bytes()).unwrap();
    for value in [0, image.data.len() as u32, 2835, 2835, 0, 0] {
        writer.write_all(&value.to_le_bytes()).unwrap();
    }
    writer.write_all(&image.data).unwrap();
    writer.flush().unwrap();
}
