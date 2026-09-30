use super::Text;

impl Text {
    pub(crate) fn search_truncated_hint(self) -> &'static str {
        self.choose("结果较多，请缩小查询", "More matches: refine search")
    }

    pub(crate) fn search_results_label(self) -> &'static str {
        self.choose("打开的窗口", "Open windows")
    }
    pub(crate) fn search_help_toggle(self) -> &'static str {
        self.choose("显示或隐藏快捷键帮助", "Show or hide keyboard help")
    }
    pub(crate) fn search_keyboard_help(self) -> &'static str {
        self.choose(
            "Tab / ↑↓ 选择    Enter 切换    Esc 关闭",
            "Tab / ↑↓ Navigate    Enter Switch    Esc Dismiss",
        )
    }
    pub(crate) fn search_loading(self) -> &'static str {
        self.choose("正在查找窗口…", "Finding windows…")
    }
    pub(crate) fn search_failed_title(self) -> &'static str {
        self.choose(
            "搜索未完成，按 Esc 后重试",
            "Search unavailable. Press Escape and retry.",
        )
    }
    pub(crate) fn search_empty_title(self) -> &'static str {
        self.choose("没有匹配的窗口", "No matching windows")
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
