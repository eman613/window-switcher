use super::Text;

impl Text {
    pub(crate) fn search_results_label(self) -> &'static str {
        self.choose("打开的窗口", "Open windows")
    }
    pub(crate) fn search_input_hint(self) -> &'static str {
        self.choose("输入应用名或窗口标题", "Type an app or window title")
    }
    pub(crate) fn search_clear(self) -> &'static str {
        self.choose("清空搜索", "Clear search")
    }
    pub(crate) fn search_close(self) -> &'static str {
        self.choose("关闭搜索（Esc）", "Close search (Escape)")
    }
    pub(crate) fn search_keyboard_help(self) -> &'static str {
        self.choose(
            "↑↓ 选择   Enter 切换   Esc 关闭",
            "↑↓ Select   Enter Switch   Esc Close",
        )
    }
    pub(crate) fn search_loading(self) -> &'static str {
        self.choose("正在查找窗口…", "Finding windows…")
    }
    pub(crate) fn search_cancel_hint(self) -> &'static str {
        self.choose(
            "可继续输入，或按 Esc 关闭",
            "Keep typing, or press Escape to close",
        )
    }
    pub(crate) fn search_failed_title(self) -> &'static str {
        self.choose("搜索未完成", "Search unavailable")
    }
    pub(crate) fn search_failure(self) -> &'static str {
        self.choose(
            "按 Esc 返回后重试。问题持续时，请检查日志或重新启动应用。",
            "Press Escape and retry. If the problem persists, check the log or restart the app.",
        )
    }
    pub(crate) fn search_empty_title(self) -> &'static str {
        self.choose("没有匹配的窗口", "No matching windows")
    }
    pub(crate) fn search_empty_hint(self) -> &'static str {
        self.choose(
            "尝试更短的关键词，或清空搜索查看已打开的窗口。",
            "Try a shorter query, or clear the search to see open windows.",
        )
    }
    pub(crate) fn search_count(self, shown: usize, total: usize) -> String {
        if shown < total {
            if self.chinese {
                format!("显示 {shown} / {total} 个结果")
            } else {
                format!("{shown} of {total} results")
            }
        } else if self.chinese {
            format!("{total} 个结果")
        } else if total == 1 {
            "1 result".into()
        } else {
            format!("{total} results")
        }
    }
    pub(crate) fn search_window_meta(
        self,
        elevated: Option<bool>,
        minimized: bool,
    ) -> &'static str {
        match (elevated == Some(true), minimized) {
            (true, true) => self.choose("管理员 · 最小化", "Admin · Minimized"),
            (true, false) => self.choose("管理员", "Administrator"),
            (false, true) => self.choose("最小化", "Minimized"),
            (false, false) => "",
        }
    }
}
