//! Stable, non-sensitive reasons survive the existing worker text transport.
use super::Text;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureReason {
    ConfigOwner,
    ConfigSecurity,
    ConfigSacl,
    TaskConflict,
    TaskOwner,
    TaskElevation,
}

impl FailureReason {
    const ALL: [Self; 6] = [
        Self::ConfigOwner,
        Self::ConfigSecurity,
        Self::ConfigSacl,
        Self::TaskConflict,
        Self::TaskOwner,
        Self::TaskElevation,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ConfigOwner => "config-owner",
            Self::ConfigSecurity => "config-security",
            Self::ConfigSacl => "config-sacl",
            Self::TaskConflict => "task-conflict",
            Self::TaskOwner => "task-owner",
            Self::TaskElevation => "task-elevation",
        }
    }

    pub(crate) fn from_detail(detail: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| detail.contains(&reason.to_string()))
    }

    fn message(self, text: Text) -> &'static str {
        match self {
            Self::ConfigOwner => text.choose(
                "无法保留 INI 原所有者。请核对文件所属账户并确认迁移方案；原文件未修改。",
                "The INI owner could not be preserved. Check the owning account before migrating file permissions. The original file is unchanged."),
            Self::ConfigSecurity => text.choose(
                "INI 与候选文件的权限或组不一致。请检查文件安全设置；原文件未修改。",
                "The INI and candidate have different permissions or groups. Check file security settings. The original file is unchanged."),
            Self::ConfigSacl => text.choose(
                "INI 含审计规则或完整性标签，当前不支持自动保存。请保留文件安全设置；原文件未修改。",
                "The INI has audit rules or integrity labels that automatic saving cannot preserve. Keep its security settings. The original file is unchanged."),
            Self::TaskConflict => text.choose(
                "启动任务已被其他操作修改。请重新检查任务状态后重试。",
                "The startup task changed externally. Check its current state and retry."),
            Self::TaskOwner => text.choose(
                "同名启动任务属于其他程序或账户。请核对任务归属；未覆盖该任务。",
                "The startup task belongs to another program or account. Check its ownership; it was not overwritten."),
            Self::TaskElevation => text.choose(
                "最高权限自启动需要管理员权限。请以管理员身份重启后重试。",
                "Highest-privilege startup requires elevation. Restart as administrator and retry."),
        }
    }
}

impl fmt::Display for FailureReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[WS-REASON:{}]", self.code())
    }
}
impl std::error::Error for FailureReason {}

#[derive(Debug)]
pub(crate) struct SystemFailure(pub(crate) u32);
impl fmt::Display for SystemFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[WS-HRESULT:0x{:08X}]", self.0)
    }
}
impl std::error::Error for SystemFailure {}

pub(crate) fn system_code(detail: &str) -> Option<u32> {
    let (_, value) = detail.split_once("[WS-HRESULT:0x")?;
    let (digits, _) = value.split_once(']')?;
    if digits.len() != 8 {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

impl Text {
    pub(super) fn reason_detail(self, detail: &str) -> Option<String> {
        let reason = FailureReason::from_detail(detail)?;
        let system =
            system_code(detail).map_or(String::new(), |code| format!("\n{}", SystemFailure(code)));
        Some(format!("{}\n{reason}{system}", reason.message(self)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Language, localization::FailureKind};

    #[test]
    fn structured_reasons_survive_context_without_exposing_private_details() {
        for reason in FailureReason::ALL {
            let error = anyhow::anyhow!("private path and SID").context(reason);
            let detail = format!("outer context: {error:#}");
            for language in [Language::Chinese, Language::English] {
                let notice = Text::new(language).failure(FailureKind::Configuration, &detail);
                assert!(!notice.contains("private"));
                assert!(notice.contains(reason.code()));
                assert!(notice.contains("[WS-CONFIG]"));
                assert!(!notice.starts_with("[WS-CONFIG]"));
            }
        }
        assert_eq!(FailureReason::from_detail("unknown error"), None);
    }

    #[test]
    fn native_code_is_visible_and_does_not_consume_arbitrary_diagnostic_text() {
        let error = anyhow::anyhow!("private OS detail")
            .context(SystemFailure(0x8007051b))
            .context(FailureReason::ConfigOwner);
        let detail = format!("{error:#}");
        assert_eq!(system_code(&detail), Some(0x8007051b));
        let notice = Text::new(Language::Chinese).failure(FailureKind::Configuration, &detail);
        assert!(notice.contains("8007051B"));
        assert!(!notice.contains("private"));
        assert_eq!(system_code("[WS-HRESULT:0x8007051Bextra]"), None);
        assert_eq!(system_code("[WS-HRESULT:0x8007051B"), None);
    }
}
