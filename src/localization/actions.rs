use super::Text;
impl Text {
    pub(crate) fn close_window(self) -> &'static str {
        if self.chinese {
            "关闭选中窗口 (Ctrl+W)"
        } else {
            "Close selected window (Ctrl+W)"
        }
    }
    pub(crate) fn close_yes(self) -> &'static str {
        if self.chinese {
            "确认关闭"
        } else {
            "Confirm close"
        }
    }
    pub(crate) fn close_cancel(self) -> &'static str {
        if self.chinese {
            "取消关闭 (Esc)"
        } else {
            "Cancel close (Esc)"
        }
    }
    pub(crate) fn close_question(self, title: &str) -> String {
        if self.chinese {
            format!("关闭“{title}”？")
        } else {
            format!("Close “{title}”?")
        }
    }
    pub(crate) fn close_requested(self) -> &'static str {
        if self.chinese {
            "已请求关闭；是否保存由目标应用决定。"
        } else {
            "Close requested; the target application handles unsaved changes."
        }
    }
    pub(crate) fn close_failed(self) -> &'static str {
        if self.chinese {
            "无法请求关闭：窗口已失效或访问受限。请刷新后重试。"
        } else {
            "Unable to request close: stale window or access denied. Refresh and retry."
        }
    }
    pub(crate) fn report_label(self) -> &'static str {
        if self.chinese {
            "导出诊断报告"
        } else {
            "Export diagnostic report"
        }
    }
    pub(crate) fn report_open(self) -> &'static str {
        if self.chinese {
            "打开最近报告"
        } else {
            "Open latest report"
        }
    }
    pub(crate) fn report_open_failed(self) -> &'static str {
        if self.chinese {
            "报告已保存，但打开失败。请检查 Reports 中的文件及默认文本查看器，可从托盘重试打开。"
        } else {
            "Report saved, but opening failed. Check the file in Reports and your default text viewer, then retry from the tray."
        }
    }
    pub(crate) fn report_failed(self) -> &'static str {
        if self.chinese {
            "报告操作失败，请检查程序目录下 Reports 的访问权限及默认文本查看器后重试。"
        } else {
            "Report operation failed. Check access to Reports beside the application and your default text viewer, then retry."
        }
    }
}
