use super::*;
use crate::{config::test_support::TestDirectory, pixels::PixelImage};
use std::{
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
};
use windows::{
    core::Interface,
    Win32::Graphics::DirectWrite::{
        DWRITE_FONT_FACE_TYPE_CFF, DWRITE_FONT_FACE_TYPE_TRUETYPE, DWRITE_TEXT_METRICS,
    },
};

fn fixtures() -> TestDirectory {
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("narrow.ttf"),
        include_bytes!("fixtures/narrow.ttf"),
    )
    .unwrap();
    fs::write(
        directory.0.join("wide.otf"),
        include_bytes!("fixtures/wide.otf"),
    )
    .unwrap();
    directory
}
fn choices() -> Config {
    Config {
        app_name_font_family: "WS Role Fixture".into(),
        app_name_font_file: Some("narrow.ttf".into()),
        badge_font_family: "WS Role Fixture".into(),
        badge_font_file: Some("wide.otf".into()),
        ..Default::default()
    }
}
fn bounds(role: FontRole, pixels: u32) -> TextBox {
    TextBox {
        role,
        pixels,
        width: 400,
        height: 80,
        ellipsis: false,
    }
}
fn width(fonts: &mut FontResources, role: FontRole) -> f32 {
    let layout = fonts.layout("AAAA", bounds(role, 20)).unwrap();
    let mut metrics = DWRITE_TEXT_METRICS::default();
    unsafe { layout.GetMetrics(&mut metrics) }.unwrap();
    metrics.width
}

#[test]
fn same_family_private_ttf_and_cff_are_isolated_and_survive_source_removal() {
    let directory = fixtures();
    let mut fonts = FontResources::load(&choices(), &directory.0).unwrap();
    assert!(fonts.fonts.iter().all(|font| font.private));
    assert_eq!(fonts.fonts[0].family, fonts.fonts[1].family);
    assert_ne!(
        fonts.fonts[0].collection.as_raw(),
        fonts.fonts[1].collection.as_raw()
    );
    assert_eq!(
        unsafe { fonts.fonts[0].face.GetType() },
        DWRITE_FONT_FACE_TYPE_TRUETYPE
    );
    assert_eq!(
        unsafe { fonts.fonts[1].face.GetType() },
        DWRITE_FONT_FACE_TYPE_CFF
    );
    fs::remove_file(directory.0.join("narrow.ttf")).unwrap();
    fs::remove_file(directory.0.join("wide.otf")).unwrap();
    let narrow = width(&mut fonts, FontRole::Name);
    let wide = width(&mut fonts, FontRole::Badge);
    assert!(
        wide > narrow * 1.5,
        "role collections were mixed: {narrow}, {wide}"
    );
    for role in [FontRole::Name, FontRole::Badge] {
        let layout = fonts
            .layout(
                "A 中文 Ω",
                TextBox {
                    ellipsis: true,
                    width: 90,
                    ..bounds(role, 20)
                },
            )
            .unwrap();
        let image =
            crate::text_raster::rasterize(&fonts.factory, &layout, 90, 80, 0x123456).unwrap();
        assert!(image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] > 0));
        assert!(image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[..3].iter().all(|value| *value <= pixel[3])));
        assert!(fonts.fonts[role.index()].missing_reported);
    }
}

#[test]
fn invalid_missing_locked_and_remote_files_fall_back_only_for_their_role() {
    let directory = fixtures();
    fs::write(directory.0.join("broken.ttf"), b"not a font").unwrap();
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(directory.0.join("narrow.ttf"))
        .unwrap();
    for path in [
        "broken.ttf",
        "missing.ttf",
        "narrow.ttf",
        r"\\unreachable.invalid\fonts\test.ttf",
        "wrong.woff",
    ] {
        let config = Config {
            app_name_font_file: Some(path.into()),
            ..choices()
        };
        let fonts = FontResources::load(&config, &directory.0).unwrap();
        assert!(!fonts.fonts[0].private, "{path}");
        assert!(fonts.fonts[1].private, "name fallback changed Badge");
        assert_eq!(config.app_name_font_file, Some(PathBuf::from(path)));
    }
    drop(locked);
}

#[test]
fn private_system_family_never_replaces_the_other_roles_system_collection() {
    let directory = fixtures();
    let system_font = PathBuf::from(std::env::var_os("WINDIR").unwrap())
        .join("Fonts")
        .join("segoeui.ttf");
    let config = Config {
        app_name_font_family: "Segoe UI".into(),
        app_name_font_file: Some(system_font),
        badge_font_family: "Segoe UI".into(),
        ..Default::default()
    };
    let fonts = FontResources::load(&config, &directory.0).unwrap();
    assert!(fonts.fonts[0].private);
    assert!(!fonts.fonts[1].private);
    assert_ne!(
        fonts.fonts[0].collection.as_raw(),
        fonts.fonts[1].collection.as_raw()
    );
}

#[test]
fn format_cache_is_bounded_and_badge_counts_render_after_role_and_size_changes() {
    let directory = fixtures();
    let mut fonts = FontResources::load(&choices(), &directory.0).unwrap();
    for pixels in 8..100 {
        fonts
            .layout("9999+", bounds(FontRole::Badge, pixels))
            .unwrap();
        fonts
            .layout("NAME", bounds(FontRole::Name, pixels))
            .unwrap();
    }
    assert_eq!(fonts.formats.len(), 64);
    for size in [24, 64, 128, 256] {
        for (count, maximum) in [
            (0, 99),
            (1, 99),
            (2, 99),
            (99, 99),
            (100, 99),
            (10000, 9999),
        ] {
            let mut image = PixelImage::new(size, size).unwrap();
            crate::badge::compose(
                &mut image,
                Some(&mut fonts),
                count,
                maximum,
                crate::layout::PixelRect {
                    left: 0,
                    top: 0,
                    right: size,
                    bottom: size,
                },
                crate::badge::BadgeStyle::from_config(&Config::default()),
            )
            .unwrap();
            assert_eq!(
                image
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] != 0),
                count > 1
            );
        }
    }
}

#[test]
#[ignore = "native GDI counters require an isolated serial process"]
fn private_font_reload_and_raster_resources_remain_bounded() {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS};
    let directory = fixtures();
    let count = || unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
    let draw = || {
        let mut fonts = FontResources::load(&choices(), &directory.0).unwrap();
        for role in [FontRole::Name, FontRole::Badge] {
            let layout = fonts.layout("NAME 9999+", bounds(role, 20)).unwrap();
            crate::text_raster::rasterize(&fonts.factory, &layout, 400, 80, 0xffffff).unwrap();
        }
    };
    draw();
    let initial = count();
    for _ in 0..100 {
        draw();
    }
    let final_count = count();
    eprintln!("font_resource_cycles=100 initial_gdi={initial} final_gdi={final_count}");
    assert!(final_count <= initial + 2);
}
