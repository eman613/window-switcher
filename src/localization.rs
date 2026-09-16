use crate::{config::Language, startup::StartupState};
use std::sync::atomic::{AtomicBool, Ordering};

static CHINESE: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Copy)]
pub(crate) enum FailureKind {
    Configuration,
    Restart,
    RestartCleanup,
    Startup,
    Interface,
    Launch,
}

#[derive(Clone, Copy)]
pub(crate) struct Text {
    chinese: bool,
}

impl Text {
    pub(crate) fn new(language: Language) -> Self {
        let chinese = match language {
            Language::Chinese => true,
            Language::English => false,
            Language::Auto => {
                (unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff) == 4
            }
        };
        Self { chinese }
    }

    pub(crate) fn activate(self) {
        CHINESE.store(self.chinese, Ordering::Release);
    }

    pub(crate) fn current() -> Self {
        Self {
            chinese: CHINESE.load(Ordering::Acquire),
        }
    }

    fn choose(self, chinese: &'static str, english: &'static str) -> &'static str {
        if self.chinese {
            chinese
        } else {
            english
        }
    }

    pub(crate) fn configure(self) -> &'static str {
        self.choose("编辑配置", "Edit configuration")
    }
    pub(crate) fn exit(self) -> &'static str {
        self.choose("退出", "Exit")
    }
    pub(crate) fn pause(self, paused: bool, busy: bool) -> &'static str {
        if busy {
            self.choose("正在保存暂停设置…", "Saving pause setting…")
        } else if paused {
            self.choose("恢复快捷键", "Resume shortcuts")
        } else {
            self.choose("暂停快捷键", "Pause shortcuts")
        }
    }
    pub(crate) fn switcher_name(self) -> &'static str {
        self.choose("应用切换器", "Application switcher")
    }
    pub(crate) fn search_label(self) -> &'static str {
        self.choose("搜索窗口", "Search windows")
    }
    pub(crate) fn search_results_label(self) -> &'static str {
        self.choose("窗口结果", "Window results")
    }
    pub(crate) fn search_loading(self) -> &'static str {
        self.choose(
            "正在查找窗口… 按 Esc 取消",
            "Finding windows… Press Escape to cancel",
        )
    }
    pub(crate) fn search_failure(self) -> &'static str {
        self.choose(
            "无法完成搜索。请按 Esc 返回，检查日志后重试或重启应用。",
            "Search failed. Press Escape, check the log, and retry or restart the application.",
        )
    }
    pub(crate) fn search_count(self, shown: usize, total: usize) -> String {
        if total == 0 {
            return self
                .choose(
                    "没有匹配窗口。请修改搜索内容，或按 Esc 返回。",
                    "No matching windows. Change the search or press Escape to return.",
                )
                .into();
        }
        if self.chinese {
            format!("显示 {shown} / {total} 个窗口 · Enter 切换 · Esc 取消")
        } else {
            format!("Showing {shown} of {total} windows · Enter to switch · Escape to cancel")
        }
    }
    pub(crate) fn switcher_help(self) -> &'static str {
        self.choose("循环选择应用，松开快捷键修饰键激活。按 Esc 取消；也可点击应用图标。", "Cycle through applications and release the shortcut modifier to activate. Press Escape to cancel, or click an application icon.")
    }
    pub(crate) fn window_count(self, count: usize) -> String {
        if self.chinese {
            format!("{count} 个窗口")
        } else if count == 1 {
            "1 window".into()
        } else {
            format!("{count} windows")
        }
    }
    pub(crate) fn error_title(self) -> &'static str {
        self.choose("Window Switcher 错误", "Window Switcher error")
    }
    pub(crate) fn config_saved(self) -> &'static str {
        self.choose("设置已保存", "Settings saved")
    }
    pub(crate) fn restart_required(self) -> &'static str {
        self.choose("设置已写入 INI。自动重启已关闭，请退出并重新启动应用后生效。", "Settings were saved to the INI. Automatic restart is disabled; exit and restart the application to apply them.")
    }

    pub(crate) fn failure(self, kind: FailureKind, detail: &str) -> String {
        let (code, chinese, english) = match kind {
            FailureKind::Configuration => ("WS-CONFIG", "设置未能应用。请检查 INI 的格式、合法值和文件权限；原有配置不会被默认值覆盖。",
                "Settings could not be applied. Check the INI syntax, allowed values, and file permissions. Existing values are not replaced with defaults."),
            FailureKind::Restart => ("WS-RESTART", "配置重启未完成。当前实例将恢复服务；请检查配置及日志目录，或稍后重试。",
                "The configuration restart did not complete. The current instance will resume service. Check the configuration and log directory, or retry later."),
            FailureKind::RestartCleanup => ("WS-RESTART-CLEANUP", "候选进程停止状态尚未确认。输入交接保持暂停；请检查本应用进程状态后重新启动。",
                "The replacement process could not be confirmed stopped. Input handoff remains paused. Check the application's process state before restarting it."),
            FailureKind::Startup => ("WS-STARTUP", "开机启动状态未能确认或应用。INI 期望值已保留；请检查任务服务、权限及同名启动项。",
                "Start-at-sign-in status could not be verified or applied. Desired INI settings are retained. Check Task Scheduler, permissions, and conflicting startup entries."),
            FailureKind::Interface => ("WS-UI", "界面操作未完成。请关闭菜单后重试；问题持续时重新启动应用。",
                "The interface operation did not complete. Close the menu and retry; restart the application if the problem persists."),
            FailureKind::Launch => ("WS-LAUNCH", "应用未能启动。请确认没有运行中的实例，并检查 INI、日志目录和系统权限。",
                "The application could not start. Check for an existing instance, then verify the INI, log directory, and system permissions."),
        };
        // Raw diagnostics can contain OS-localized text or user paths. Keep the
        // default Chinese detail locally; English UI uses stable actionable
        // resources and a language-independent support code instead of mixed text.
        if self.chinese {
            format!("{chinese}\n[{code}]\n{detail}")
        } else {
            format!("{english}\n[{code}]")
        }
    }
    pub(crate) fn log_failure(self, code: i32) -> String {
        if self.chinese {
            format!("部分日志未能写入（系统代码 {code}）。请检查日志目录权限和磁盘空间，并确认日志与 INI 是不同文件。")
        } else {
            format!("Some log records could not be written (system code {code}). Check directory permissions, free space, and that the log and INI are different files.")
        }
    }

    pub(crate) fn startup(self, state: StartupState, busy: bool) -> &'static str {
        match state {
            StartupState::Pending => {
                self.choose("开机启动（检测中）", "Start at sign-in (checking)")
            }
            StartupState::Failed => {
                self.choose("开机启动（状态未知）", "Start at sign-in (unknown)")
            }
            StartupState::Saved(_) => self.choose(
                "开机启动（重启后生效）",
                "Start at sign-in (restart required)",
            ),
            StartupState::Ready(_) if busy => {
                self.choose("开机启动（保存中）", "Start at sign-in (saving)")
            }
            StartupState::Ready(_) => self.choose("开机启动", "Start at sign-in"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_language_and_pending_states_are_distinct() {
        let zh = Text::new(Language::Chinese);
        assert_eq!(
            [
                zh.configure(),
                zh.startup(StartupState::Ready(true), false),
                zh.exit()
            ],
            ["编辑配置", "开机启动", "退出"]
        );
        let en = Text::new(Language::English);
        assert_eq!(en.configure(), "Edit configuration");
        assert_ne!(
            zh.startup(StartupState::Pending, true),
            zh.startup(StartupState::Ready(false), false)
        );
        assert_ne!(
            en.startup(StartupState::Failed, false),
            en.startup(StartupState::Ready(false), false)
        );
    }

    #[test]
    fn application_failures_use_the_selected_language_and_stable_codes() {
        for kind in [
            FailureKind::Configuration,
            FailureKind::Restart,
            FailureKind::RestartCleanup,
            FailureKind::Startup,
            FailureKind::Interface,
            FailureKind::Launch,
        ] {
            let english = Text::new(Language::English).failure(kind, "底层错误 D:\\private");
            assert!(english.is_ascii());
            assert!(english.contains("[WS-"));
            assert!(!english.contains("private"));
            assert!(Text::new(Language::Chinese)
                .failure(kind, "底层错误")
                .contains("底层错误"));
        }
    }
}
