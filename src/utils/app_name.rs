use std::{path::Path, slice};

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};

const MAX_VERSION_INFO_BYTES: u32 = 4 * 1024 * 1024;
const MAX_DISPLAY_NAME_CHARS: usize = 256;

pub(crate) fn app_name_fallback(module_key: &str) -> String {
    let base = module_key.split("::").next().unwrap_or(module_key);
    let stem = Path::new(base)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(base);
    let stem = clean_component(stem);
    let suffix = module_key
        .split("::")
        .skip(1)
        .filter(|part| !part.is_empty())
        .map(clean_component)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let name = if suffix.is_empty() {
        stem
    } else {
        format!("{stem} ({})", suffix.join(" / "))
    };
    name.chars().take(MAX_DISPLAY_NAME_CHARS).collect()
}

pub(crate) fn try_get_app_name(module_key: &str) -> Option<String> {
    let base = module_key
        .split("::")
        .next()
        .filter(|value| !value.is_empty())?;
    let description = read_version_string(base, "FileDescription")
        .or_else(|| read_version_string(base, "ProductName"))?;
    let description = sanitize_name(&description)?;
    let suffix = module_key
        .split("::")
        .skip(1)
        .filter(|part| !part.is_empty())
        .map(clean_component)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let name = if suffix.is_empty() {
        description
    } else {
        format!("{description} ({})", suffix.join(" / "))
    };
    Some(name.chars().take(MAX_DISPLAY_NAME_CHARS).collect())
}

fn read_version_string(path: &str, value_name: &str) -> Option<String> {
    let path_w = path.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path_w.as_ptr()), None) };
    if size == 0 || size > MAX_VERSION_INFO_BYTES {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    unsafe {
        GetFileVersionInfoW(
            PCWSTR(path_w.as_ptr()),
            Some(0),
            size,
            data.as_mut_ptr().cast(),
        )
        .ok()?;
    }

    let mut languages = Vec::with_capacity(19);
    let mut translation = std::ptr::null_mut();
    let mut translation_len = 0u32;
    let translation_query = crate::utils::to_wstring(r"\VarFileInfo\Translation");
    let translation_found = unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(translation_query.as_ptr()),
            &mut translation,
            &mut translation_len,
        )
    };
    if translation_found.as_bool() && !translation.is_null() {
        let units = unsafe {
            slice::from_raw_parts(translation.cast::<u16>(), (translation_len as usize) / 2)
        };
        for pair in units.chunks_exact(2).take(8) {
            languages.push(format!("{:04X}{:04X}", pair[0], pair[1]));
        }
    }
    languages.extend(
        ["040904B0", "080404B0", "041104B0"]
            .into_iter()
            .map(str::to_string),
    );

    for language in languages {
        let query = format!(r"\StringFileInfo\{language}\{value_name}");
        let query_w = query.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let mut value = std::ptr::null_mut();
        let mut length = 0u32;
        let found = unsafe {
            VerQueryValueW(
                data.as_ptr().cast(),
                PCWSTR(query_w.as_ptr()),
                &mut value,
                &mut length,
            )
        };
        if !found.as_bool() || value.is_null() || length <= 1 {
            continue;
        }
        let units = unsafe { slice::from_raw_parts(value.cast::<u16>(), length as usize) };
        let text = String::from_utf16_lossy(units);
        if !text.trim_matches('\0').trim().is_empty() {
            return Some(text.trim_matches('\0').trim().to_string());
        }
    }
    None
}

fn sanitize_name(value: &str) -> Option<String> {
    let value = value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.chars().take(MAX_DISPLAY_NAME_CHARS).collect())
}

fn clean_component(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::app_name_fallback;

    #[test]
    fn fallback_removes_internal_identity_separator() {
        assert_eq!(
            app_name_fallback(r"C:\Apps\Browser.exe::Profile 2::app-id"),
            "Browser (Profile 2 / app-id)"
        );
    }

    #[test]
    fn fallback_handles_non_exe_and_empty_suffixes() {
        assert_eq!(app_name_fallback("demo"), "demo");
        assert_eq!(app_name_fallback("demo::"), "demo");
    }
}
