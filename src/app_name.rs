//! Application metadata belongs to the icon worker, never the input/UI path.
use crate::config::Config;
use anyhow::{ensure, Context, Result};
use indexmap::IndexMap;
use std::{
    ffi::c_void,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    core::{w, HSTRING},
    Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
};

const MAX_VERSION_BYTES: u32 = 1024 * 1024;
const MAX_NAME_UNITS: usize = 1024;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct NameCache {
    entries: IndexMap<Arc<str>, (Arc<str>, Instant)>,
    limit: usize,
    ttl: Duration,
    bytes: usize,
}

impl NameCache {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            entries: IndexMap::new(),
            limit: config.metadata_cache_limit as usize,
            ttl: Duration::from_millis(config.metadata_ttl_ms.into()),
            bytes: 0,
        }
    }

    pub(crate) fn resolve(&mut self, group: &Arc<str>) -> Arc<str> {
        if let Some((name, when)) = self.entries.shift_remove(group) {
            self.bytes -= group.len() + name.len();
            if when.elapsed() < self.ttl {
                self.bytes += group.len() + name.len();
                self.entries.insert(group.clone(), (name.clone(), when));
                return name;
            }
        }
        let path = Path::new(group.split("::").next().unwrap_or(group));
        let name: Arc<str> = match description(path) {
            Ok(name) => name.into(),
            Err(error) => {
                trace!("app-name stage=file-description fallback={error:#}");
                fallback(path).into()
            }
        };
        let bytes = group.len() + name.len();
        while self.entries.len() >= self.limit || self.bytes + bytes > MAX_CACHE_BYTES {
            let Some((key, (old, _))) = self.entries.shift_remove_index(0) else {
                break;
            };
            self.bytes -= key.len() + old.len();
        }
        if bytes <= MAX_CACHE_BYTES {
            self.bytes += bytes;
            self.entries
                .insert(group.clone(), (name.clone(), Instant::now()));
        }
        name
    }
}

fn fallback(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn description(path: &Path) -> Result<String> {
    let path = HSTRING::from(path.as_os_str());
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    ensure!(
        (1..=MAX_VERSION_BYTES).contains(&size),
        "missing-or-oversized-version"
    );
    // DWORD-aligned storage also makes the returned translation/string views aligned.
    let mut data = vec![0u32; (size as usize).div_ceil(4)];
    unsafe { GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()) }
        .context("version-read")?;
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), size as usize) };
    let mut translations = Vec::new();
    if let Some((pointer, length)) = query(bytes, w!(r"\VarFileInfo\Translation"), 1) {
        let values = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) };
        for value in values.as_chunks::<4>().0.iter().take(64) {
            translations.push((
                u16::from_le_bytes([value[0], value[1]]),
                u16::from_le_bytes([value[2], value[3]]),
            ));
        }
    }
    translations.extend([(0x0409, 0x04b0), (0x0409, 0x04e4)]);
    for (language, codepage) in translations {
        let key = HSTRING::from(format!(
            r"\StringFileInfo\{language:04x}{codepage:04x}\FileDescription"
        ));
        let Some((pointer, length)) = query(bytes, windows::core::PCWSTR(key.as_ptr()), 2) else {
            continue;
        };
        if length == 0 || length > MAX_NAME_UNITS + 1 || !(pointer as usize).is_multiple_of(2) {
            continue;
        }
        let value = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), length) };
        let value = &value[..value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len())];
        let name = String::from_utf16_lossy(value);
        let name = name.trim();
        if !name.is_empty() && !name.chars().any(char::is_control) {
            return Ok(name.to_owned());
        }
    }
    anyhow::bail!("no-usable-file-description")
}

fn query(data: &[u8], key: windows::core::PCWSTR, unit: usize) -> Option<(*const c_void, usize)> {
    let mut pointer = std::ptr::null_mut();
    let mut length = 0u32;
    if !unsafe { VerQueryValueW(data.as_ptr().cast(), key, &mut pointer, &mut length) }.as_bool() {
        return None;
    }
    let start = pointer as usize;
    let offset = start.checked_sub(data.as_ptr() as usize)?;
    let bytes = (length as usize).checked_mul(unit)?;
    (offset.checked_add(bytes)? <= data.len()).then_some((pointer, length as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_metadata_uses_executable_and_cache_is_bounded() {
        let mut cache = NameCache::new(&Config {
            metadata_cache_limit: 16,
            ..Default::default()
        });
        for index in 0..100 {
            let group = Arc::from(format!(r"C:\missing\app-{index}.exe::profile::app-id"));
            assert_eq!(&*cache.resolve(&group), format!("app-{index}.exe"));
        }
        assert_eq!(cache.entries.len(), 16);
        assert!(cache.bytes < MAX_CACHE_BYTES);
    }
}
