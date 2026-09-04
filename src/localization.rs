use std::{
    str::FromStr,
    sync::atomic::{AtomicU8, Ordering},
};

use windows::Win32::Globalization::GetUserDefaultUILanguage;

const LANGUAGE_UNINITIALIZED: u8 = 0;
const LANGUAGE_ZH_CN: u8 = 1;
const LANGUAGE_EN_US: u8 = 2;
const PRIMARY_LANGUAGE_MASK: u16 = 0x03ff;
const PRIMARY_LANGUAGE_CHINESE: u16 = 0x0004;

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(LANGUAGE_UNINITIALIZED);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    Auto,
    ZhCn,
    EnUs,
}

impl FromStr for Language {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "auto" => Ok(Self::Auto),
            "zh-cn" => Ok(Self::ZhCn),
            "en-us" => Ok(Self::EnUs),
            _ => Err(()),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedLanguage {
    ZhCn = LANGUAGE_ZH_CN,
    EnUs = LANGUAGE_EN_US,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextId {
    TrayTooltip,
    MenuConfigure,
    MenuStartup,
    MenuExit,
    ErrorTitle,
    StartupConflict,
    ConfigLoadFailed,
    ConfigWarnings,
    ConfigReloadFailed,
    AnotherInstance,
    RestartFailed,
}

pub fn set_language(language: Language) {
    let resolved = resolve_language(language);
    CURRENT_LANGUAGE.store(resolved as u8, Ordering::Release);
}

pub fn text(id: TextId) -> &'static str {
    match (current_language(), id) {
        (ResolvedLanguage::ZhCn, TextId::TrayTooltip) => "窗口切换器",
        (ResolvedLanguage::ZhCn, TextId::MenuConfigure) => "编辑配置",
        (ResolvedLanguage::ZhCn, TextId::MenuStartup) => "开机启动",
        (ResolvedLanguage::ZhCn, TextId::MenuExit) => "退出",
        (ResolvedLanguage::ZhCn, TextId::ErrorTitle) => "窗口切换器错误",
        (ResolvedLanguage::ZhCn, TextId::StartupConflict) => {
            "检测到管理员计划任务。请先以管理员身份运行窗口切换器并关闭“开机启动”，再以普通用户身份启用。"
        }
        (ResolvedLanguage::ZhCn, TextId::ConfigLoadFailed) => {
            "配置文件加载失败。当前进程将使用安全默认设置。"
        }
        (ResolvedLanguage::ZhCn, TextId::ConfigWarnings) => {
            "配置文件包含问题。无效项目已使用安全默认值。"
        }
        (ResolvedLanguage::ZhCn, TextId::ConfigReloadFailed) => {
            "配置文件修改后无法重新加载。当前进程继续使用原配置。"
        }
        (ResolvedLanguage::ZhCn, TextId::AnotherInstance) => {
            "窗口切换器已在运行，本次启动已取消。"
        }
        (ResolvedLanguage::ZhCn, TextId::RestartFailed) => {
            "配置已变更，但窗口切换器重新启动失败。"
        }
        (ResolvedLanguage::EnUs, TextId::TrayTooltip) => "Window Switcher",
        (ResolvedLanguage::EnUs, TextId::MenuConfigure) => "Configure",
        (ResolvedLanguage::EnUs, TextId::MenuStartup) => "Startup",
        (ResolvedLanguage::EnUs, TextId::MenuExit) => "Exit",
        (ResolvedLanguage::EnUs, TextId::ErrorTitle) => "Window Switcher Error",
        (ResolvedLanguage::EnUs, TextId::StartupConflict) => {
            "An administrator scheduled task already exists. Run Window Switcher as administrator and disable Startup before enabling it as a standard user."
        }
        (ResolvedLanguage::EnUs, TextId::ConfigLoadFailed) => {
            "The configuration file could not be loaded. Safe defaults will be used for this process."
        }
        (ResolvedLanguage::EnUs, TextId::ConfigWarnings) => {
            "The configuration file contains problems. Safe defaults were used for invalid settings."
        }
        (ResolvedLanguage::EnUs, TextId::ConfigReloadFailed) => {
            "The changed configuration could not be reloaded. The current configuration remains active."
        }
        (ResolvedLanguage::EnUs, TextId::AnotherInstance) => {
            "Window Switcher is already running. This launch was cancelled."
        }
        (ResolvedLanguage::EnUs, TextId::RestartFailed) => {
            "The configuration changed, but Window Switcher could not restart."
        }
    }
}

fn current_language() -> ResolvedLanguage {
    match CURRENT_LANGUAGE.load(Ordering::Acquire) {
        LANGUAGE_ZH_CN => ResolvedLanguage::ZhCn,
        LANGUAGE_EN_US => ResolvedLanguage::EnUs,
        _ => resolve_language(Language::Auto),
    }
}

fn resolve_language(language: Language) -> ResolvedLanguage {
    match language {
        Language::ZhCn => ResolvedLanguage::ZhCn,
        Language::EnUs => ResolvedLanguage::EnUs,
        Language::Auto => resolve_windows_language(unsafe { GetUserDefaultUILanguage() }),
    }
}

const fn resolve_windows_language(language_id: u16) -> ResolvedLanguage {
    if language_id & PRIMARY_LANGUAGE_MASK == PRIMARY_LANGUAGE_CHINESE {
        ResolvedLanguage::ZhCn
    } else {
        ResolvedLanguage::EnUs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_values_are_case_insensitive() {
        assert_eq!("auto".parse(), Ok(Language::Auto));
        assert_eq!("ZH_cn".parse(), Ok(Language::ZhCn));
        assert_eq!("en-US".parse(), Ok(Language::EnUs));
        assert!("zh-TW".parse::<Language>().is_err());
    }

    #[test]
    fn windows_chinese_language_ids_resolve_to_chinese() {
        assert_eq!(resolve_windows_language(0x0804), ResolvedLanguage::ZhCn);
        assert_eq!(resolve_windows_language(0x0404), ResolvedLanguage::ZhCn);
        assert_eq!(resolve_windows_language(0x0409), ResolvedLanguage::EnUs);
    }
}
