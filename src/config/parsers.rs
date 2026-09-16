use std::{collections::HashSet, path::PathBuf};

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use log::LevelFilter;

use super::{hotkey::parse_hotkeys, Config, Hotkey};

pub(super) fn boolean(value: &str) -> Result<bool> {
    Config::to_bool(value).context("可选 yes/no、true/false、on/off、1/0")
}

pub(super) fn automatic_bool(value: &str) -> Result<Option<bool>> {
    if value == "auto" {
        Ok(None)
    } else {
        boolean(value).map(Some)
    }
}

pub(super) fn integer(value: &str, minimum: u32, maximum: u32) -> Result<u32> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| (minimum..=maximum).contains(value))
        .with_context(|| format!("可选整数 {minimum}-{maximum}"))
}

pub(super) fn automatic_integer(value: &str, minimum: u32, maximum: u32) -> Result<Option<u32>> {
    if value == "auto" {
        Ok(None)
    } else {
        integer(value, minimum, maximum)
            .map(Some)
            .with_context(|| format!("可选 auto 或整数 {minimum}-{maximum}"))
    }
}

pub(super) fn color(value: &str) -> Result<u32> {
    super::validation::parse_color(value).context("请填写 #RRGGBB 六位十六进制颜色")
}

pub(super) fn automatic_color(value: &str) -> Result<Option<u32>> {
    if value == "auto" {
        return Ok(None);
    }
    if !value.starts_with('#') {
        bail!("请填写 auto 或 #RRGGBB 六位十六进制颜色");
    }
    color(value).map(Some)
}

pub(super) fn radius(value: &str, maximum: u32) -> Result<Option<u32>> {
    automatic_integer(value, 0, maximum)
}

pub(super) fn font_family(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 128 || value.chars().any(char::is_control) {
        bail!("字体族名须为 1–128 个字符，不能包含控制字符");
    }
    Ok(value.to_owned())
}

pub(super) fn font_file(value: &str) -> Result<Option<PathBuf>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    if value.contains('\0') || value.encode_utf16().count() > 32767 {
        bail!("字体路径不能包含 NUL 或超过 32767 个 UTF-16 单元");
    }
    // Resolve relative to the active INI in the font worker, never relative to CWD.
    Ok(Some(PathBuf::from(value)))
}

pub(super) fn font_weight(value: &str) -> Result<u32> {
    let weight = integer(value, 100, 900)?;
    if !weight.is_multiple_of(100) {
        bail!("字重须为 100–900 之间的 100 的倍数");
    }
    Ok(weight)
}

pub(super) fn level(value: &str) -> Result<LevelFilter> {
    value
        .parse()
        .context("可选 off/error/warn/info/debug/trace")
}

pub(super) fn log_path(value: &str) -> Result<Option<PathBuf>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    if value.contains('\0') || value.encode_utf16().count() > 32767 {
        bail!("日志路径不可包含 NUL 且不能超过 32767 个 UTF-16 单元");
    }
    let path = PathBuf::from(value);
    Ok(Some(if path.is_absolute() {
        path
    } else {
        crate::utils::get_exe_folder()?.join(path)
    }))
}

pub(super) fn hotkeys(value: &str, id: u32, name: &str, default: &str) -> Result<Vec<Hotkey>> {
    parse_hotkeys(
        id,
        name,
        if value.trim().is_empty() {
            default
        } else {
            value
        },
    )
    .context("请使用 alt/ctrl/win + 主键，多个组合用 || 分隔")
}

pub(super) fn blacklist(value: &str) -> Result<HashSet<String>> {
    string_list(value, 260)
}

pub(super) fn titles(value: &str) -> Result<HashSet<String>> {
    string_list(value, 4096)
}

fn string_list(value: &str, item_limit: usize) -> Result<HashSet<String>> {
    let entries: Vec<_> = value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect();
    if entries.len() > 256
        || entries
            .iter()
            .any(|item| item.contains('\0') || item.encode_utf16().count() > item_limit)
    {
        bail!("列表最多 256 项，每项最多 {item_limit} 个 UTF-16 单元，以英文逗号分隔");
    }
    Ok(entries.into_iter().map(str::to_owned).collect())
}

pub(super) fn directory(value: &str) -> Result<Option<PathBuf>> {
    if value == "auto" {
        return Ok(None);
    }
    if value.trim().is_empty() || value.contains('\0') || value.encode_utf16().count() > 32767 {
        bail!("请填写 auto 或非空目录路径，最长 32767 个 UTF-16 单元");
    }
    Ok(Some(PathBuf::from(value)))
}

pub(super) fn overrides(value: &str) -> Result<IndexMap<String, String>> {
    let mut overrides = IndexMap::new();
    let mut duplicates = 0;
    for entry in value
        .split([',', ';'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let (pattern, path) = entry
            .split_once('=')
            .context("请使用 app.exe=icon.ico，多个条目以英文逗号或分号分隔")?;
        if pattern.trim().is_empty() || path.trim().is_empty() || entry.contains('\0') {
            bail!("匹配文本和图标路径不能为空或包含 NUL");
        }
        if pattern.encode_utf16().count() > 260 || path.encode_utf16().count() > 32767 {
            bail!("图标匹配文本最多 260 个 UTF-16 单元，路径最多 32767 个");
        }
        duplicates += usize::from(
            overrides
                .insert(pattern.trim().to_lowercase(), path.trim().to_owned())
                .is_some(),
        );
        if overrides.len() > 256 {
            bail!("图标覆盖规则最多 256 项");
        }
    }
    if duplicates != 0 {
        warn!("config stage=override-precedence duplicate_patterns={duplicates} first_position_last_value=true");
    }
    Ok(overrides)
}
