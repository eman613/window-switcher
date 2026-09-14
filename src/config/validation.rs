use anyhow::{bail, Context, Result};
use ini::Ini;
use log::LevelFilter;

use super::{hotkey::parse_hotkeys, Config, SWITCH_APPS_HOTKEY_ID, SWITCH_WINDOWS_HOTKEY_ID};

pub(super) fn validate_values(ini: &Ini) -> Result<()> {
    for (section, properties) in ini {
        let section = section.unwrap_or_default();
        for (key, value) in properties {
            match (section, key) {
                ("", "trayicon" | "auto_restart")
                | ("switch-windows" | "switch-apps", "ignore_minimal")
                | ("switch-apps", "enable" | "show_badge") => {
                    if Config::to_bool(value).is_none() {
                        bail!("[{section}] {key} 无效；可选 yes/no、true/false、on/off、1/0，原值未修改");
                    }
                }
                ("switch-windows" | "switch-apps", "only_current_desktop") => {
                    if value != "auto" && Config::to_bool(value).is_none() {
                        bail!("[{section}] {key} 无效；可选 auto、yes 或 no，原值未修改");
                    }
                }
                ("", "restart_delay_ms") => validate_range(section, key, value, 200, 10_000)?,
                ("switch-apps", "badge_max") => validate_range(section, key, value, 2, 9999)?,
                ("switch-apps", "badge_font_size") => validate_range(section, key, value, 8, 24)?,
                ("switch-apps", "badge_color" | "badge_text_color") => {
                    if parse_color(value).is_none() {
                        bail!(
                            "[{section}] {key} 无效；请填写 #RRGGBB 六位十六进制颜色，原值未修改"
                        );
                    }
                }
                ("switch-windows" | "switch-apps", "hotkey") if !value.trim().is_empty() => {
                    let id = if section == "switch-apps" {
                        SWITCH_APPS_HOTKEY_ID
                    } else {
                        SWITCH_WINDOWS_HOTKEY_ID
                    };
                    parse_hotkeys(id, section, value)
                        .with_context(|| format!("[{section}] hotkey 无效；请使用 alt/ctrl/win + 主键，多个组合用 || 分隔"))?;
                }
                ("switch-apps", "override_icons") => validate_icon_overrides(value)?,
                ("log", "level") if value.parse::<LevelFilter>().is_err() => {
                    bail!("[log] level 无效；可选 off/error/warn/info/debug/trace，原值未修改");
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn validate_range(section: &str, key: &str, value: &str, minimum: u32, maximum: u32) -> Result<()> {
    if !value
        .parse::<u32>()
        .is_ok_and(|value| (minimum..=maximum).contains(&value))
    {
        bail!("[{section}] {key} 无效；可选整数 {minimum}-{maximum}，原值未修改");
    }
    Ok(())
}

pub(super) fn parse_color(value: &str) -> Option<u32> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

fn validate_icon_overrides(value: &str) -> Result<()> {
    for entry in value
        .split([',', ';'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let (pattern, path) = entry
            .split_once('=')
            .context("[switch-apps] override_icons 无效；请使用 app.exe=icon.ico，原值未修改")?;
        if pattern.trim().is_empty() || path.trim().is_empty() {
            bail!("[switch-apps] override_icons 的匹配文本或图标路径为空，原值未修改");
        }
    }
    Ok(())
}
