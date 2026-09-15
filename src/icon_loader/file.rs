//! Read once, validate dimensions/offsets, then decode those exact bytes.
use super::native::OwnedIcon;
use std::{fs::File, io::Read, path::Path};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconFromResourceEx, LR_DEFAULTCOLOR};

const SOURCE_LIMIT: u64 = 16 * 1024 * 1024;
const MAX_DIMENSION: u32 = 1024;

pub(crate) fn load(path: &Path) -> Option<OwnedIcon> {
    let file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length == 0 || length > SOURCE_LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length as usize).ok()?;
    file.take(SOURCE_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > SOURCE_LIMIT {
        return None;
    }
    let resource = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png_dimensions(&bytes)?;
        bytes.as_slice()
    } else {
        ico_resource(&bytes)?
    };
    unsafe { CreateIconFromResourceEx(resource, true, 0x30000, 0, 0, LR_DEFAULTCOLOR) }
        .ok()
        .map(OwnedIcon)
}

fn little(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn big(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n"
        || big(bytes, 8)? != 13
        || bytes.get(12..16)? != b"IHDR"
    {
        return None;
    }
    let (width, height) = (big(bytes, 16)?, big(bytes, 20)?);
    if !(1..=MAX_DIMENSION).contains(&width)
        || !(1..=MAX_DIMENSION).contains(&height)
        || bytes.get(26..28)? != [0, 0]
        || *bytes.get(28)? > 1
    {
        return None;
    }
    let mut offset = 8usize;
    let mut image_data = false;
    loop {
        let length = big(bytes, offset)? as usize;
        let end = offset.checked_add(length)?.checked_add(12)?;
        let chunk = bytes.get(offset + 4..end - 4)?;
        if crc32(chunk) != big(bytes, end - 4)? {
            return None;
        }
        let kind = chunk.get(..4)?;
        if kind == b"IDAT" {
            image_data = true;
        }
        if kind == b"IEND" {
            return (length == 0 && image_data && end == bytes.len()).then_some((width, height));
        }
        offset = end;
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn ico_resource(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..4)? != [0, 0, 1, 0] {
        return None;
    }
    let count = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    if count == 0 || count > 256 {
        return None;
    }
    let table_end = 6 + count * 16;
    bytes.get(..table_end)?;
    let mut selected: Option<(&[u8], u32)> = None;
    for index in 0..count {
        let entry = 6 + index * 16;
        let dimension = |at: usize| {
            if bytes[at] == 0 {
                256
            } else {
                u32::from(bytes[at])
            }
        };
        let (width, height) = (dimension(entry), dimension(entry + 1));
        let length = little(bytes, entry + 8)? as usize;
        let offset = little(bytes, entry + 12)? as usize;
        if length == 0 || offset < table_end {
            return None;
        }
        let resource = bytes.get(offset..offset.checked_add(length)?)?;
        if resource.starts_with(b"\x89PNG") {
            if png_dimensions(resource)? != (width, height) {
                return None;
            }
        } else if !valid_dib(resource, width, height) {
            return None;
        }
        let area = width * height;
        if selected.is_none_or(|(_, previous)| area > previous) {
            selected = Some((resource, area));
        }
    }
    selected.map(|(resource, _)| resource)
}

fn valid_dib(bytes: &[u8], width: u32, height: u32) -> bool {
    let Some(header) = little(bytes, 0).filter(|size| matches!(size, 40 | 108 | 124)) else {
        return false;
    };
    if little(bytes, 4) != Some(width)
        || little(bytes, 8) != Some(height * 2)
        || bytes.get(12..14) != Some(&[1, 0])
        || little(bytes, 16) != Some(0)
    {
        return false;
    }
    let Some(bits) = bytes
        .get(14..16)
        .and_then(|v| v.try_into().ok())
        .map(u16::from_le_bytes)
    else {
        return false;
    };
    if ![1, 4, 8, 16, 24, 32].contains(&bits) {
        return false;
    }
    let colors = if bits <= 8 {
        let declared = little(bytes, 32).unwrap_or(0);
        if declared > 1u32 << bits {
            return false;
        }
        if declared == 0 {
            1u32 << bits
        } else {
            declared
        }
    } else {
        0
    };
    let stride = (u64::from(width) * u64::from(bits)).div_ceil(32) * 4;
    let mask = if bits == 32 {
        0
    } else {
        u64::from(width).div_ceil(32) * 4 * u64::from(height)
    };
    u64::from(header) + u64::from(colors) * 4 + stride * u64::from(height) + mask
        <= bytes.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_icon_tables_and_oversized_pngs_never_reach_native_decode() {
        assert!(ico_resource(&[0, 0, 1, 0, 255, 255]).is_none());
        let mut bytes = vec![0; 22];
        bytes[..6].copy_from_slice(&[0, 0, 1, 0, 1, 0]);
        bytes[14..18].copy_from_slice(&100u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(ico_resource(&bytes).is_none());
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&u32::MAX.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        assert!(png_dimensions(&png).is_none());
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }

    #[test]
    fn validated_png_and_ico_resources_keep_alpha_without_reopening_the_file() {
        let directory = crate::config::test_support::TestDirectory::new();
        let mut bitmap = vec![0u8; 64];
        bitmap[..4].copy_from_slice(&40u32.to_le_bytes());
        bitmap[4..8].copy_from_slice(&2u32.to_le_bytes());
        bitmap[8..12].copy_from_slice(&4u32.to_le_bytes());
        bitmap[12..16].copy_from_slice(&[1, 0, 32, 0]);
        bitmap[40..56]
            .copy_from_slice(&[255, 0, 0, 255, 0, 0, 255, 128, 0, 0, 0, 0, 0, 255, 0, 255]);
        let mut ico = vec![0, 0, 1, 0, 1, 0, 2, 2, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(bitmap.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&bitmap);
        let ico_path = directory.0.join("fixture.ICO");
        std::fs::write(&ico_path, &ico).unwrap();
        let icon = load(&ico_path).unwrap();
        let pixels = super::super::native::rasterize(icon.0, 2).unwrap();
        assert!(pixels
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] == 0));
        assert!(pixels
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] > 0 && pixel[3] < 255));
        assert!(pixels.data.as_chunks::<4>().0.contains(&[255, 0, 0, 255]));

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let start = png.len();
            png.extend_from_slice(kind);
            png.extend_from_slice(data);
            png.extend_from_slice(&crc32(&png[start..]).to_be_bytes());
        };
        chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        // A zlib stream containing one stored block: filter 0 and a red RGBA pixel.
        let raw = [0u8, 255, 0, 0, 128];
        let mut adler_a = 1u32;
        let mut adler_b = 0u32;
        for byte in raw {
            adler_a = (adler_a + u32::from(byte)) % 65521;
            adler_b = (adler_b + adler_a) % 65521;
        }
        let mut zlib = vec![0x78, 0x01, 0x01, 5, 0, 250, 255];
        zlib.extend_from_slice(&raw);
        zlib.extend_from_slice(&((adler_b << 16) | adler_a).to_be_bytes());
        chunk(b"IDAT", &zlib);
        chunk(b"IEND", &[]);
        assert_eq!(png_dimensions(&png), Some((1, 1)));
        let png_path = directory.0.join("fixture.png");
        std::fs::write(&png_path, &png).unwrap();
        let icon = load(&png_path).unwrap();
        let pixels = super::super::native::rasterize(icon.0, 1).unwrap();
        assert!((127..=129).contains(&pixels.data[3]));
        assert!(pixels.data[2] > 0 && pixels.data[2] <= pixels.data[3]);
        *png.last_mut().unwrap() ^= 1;
        std::fs::write(&png_path, &png).unwrap();
        assert!(load(&png_path).is_none());
    }
}
