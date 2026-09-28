use super::Text;
use crate::{
    config::{
        quick::QuickSetting, MonitorFilter, RunLevel, SearchField, SearchMatch, SwitchOrder, Theme,
    },
    trayicon::quick_settings::QuickSettingsGroup,
};

impl Text {
    pub(crate) fn quick_group(self, group: QuickSettingsGroup) -> &'static str {
        match group {
            QuickSettingsGroup::Appearance => self.choose("外观", "Appearance"),
            QuickSettingsGroup::Windows => {
                self.choose("应用切换与搜索范围", "Application and search scope")
            }
            QuickSettingsGroup::Search => self.choose("搜索设置", "Search settings"),
            QuickSettingsGroup::Startup => self.choose("开机启动", "Startup"),
        }
    }

    pub(crate) fn quick_setting(self, setting: QuickSetting) -> &'static str {
        match setting {
            QuickSetting::Theme(Theme::Auto) => self.choose("跟随系统", "Follow system theme"),
            QuickSetting::Theme(Theme::Light) => self.choose("浅色", "Light"),
            QuickSetting::Theme(Theme::Dark) => self.choose("深色", "Dark"),
            QuickSetting::Names => {
                self.choose("显示选中应用名称", "Show selected application name")
            }
            QuickSetting::Badges => self.choose("显示窗口数量角标", "Show window count badges"),
            QuickSetting::Preview => self.choose("显示窗口预览", "Show window preview"),
            QuickSetting::Minimized => self.choose("包含最小化窗口", "Include minimized windows"),
            QuickSetting::Topmost => self.choose("包含置顶窗口", "Include topmost windows"),
            QuickSetting::HiddenMinimized => {
                self.choose("包含隐藏的最小化窗口", "Include hidden minimized windows")
            }
            QuickSetting::Scope(MonitorFilter::All) => self.choose("全部显示器", "All monitors"),
            QuickSetting::Scope(MonitorFilter::Panel) => {
                self.choose("面板所在显示器", "Panel monitor")
            }
            QuickSetting::Scope(MonitorFilter::Foreground) => {
                self.choose("前台窗口所在显示器", "Foreground window monitor")
            }
            QuickSetting::Order(SwitchOrder::Existing) => {
                self.choose("保持现有顺序", "Keep existing order")
            }
            QuickSetting::Order(SwitchOrder::Mru) => {
                self.choose("最近使用优先", "Most recently used first")
            }
            QuickSetting::SearchEnabled => self.choose("启用搜索快捷键", "Enable search shortcut"),
            QuickSetting::Match(SearchMatch::Fuzzy) => self.choose("模糊匹配", "Fuzzy match"),
            QuickSetting::Match(SearchMatch::Contains) => self.choose("包含文字", "Contains text"),
            QuickSetting::Match(SearchMatch::Prefix) => self.choose("开头匹配", "Prefix match"),
            QuickSetting::Field(SearchField::App) => {
                self.choose("搜索应用名称", "Search application names")
            }
            QuickSetting::Field(SearchField::Title) => {
                self.choose("搜索窗口标题", "Search window titles")
            }
            QuickSetting::Field(SearchField::Exe) => {
                self.choose("搜索程序文件名", "Search executable names")
            }
            QuickSetting::StartupLevel(RunLevel::Inherit) => self.choose(
                "保留已有启动权限策略",
                "Keep existing startup privilege policy",
            ),
            QuickSetting::StartupLevel(RunLevel::Standard) => {
                self.choose("开机启动使用普通权限", "Standard startup permissions")
            }
            QuickSetting::StartupLevel(RunLevel::Highest) => self.choose(
                "开机启动使用最高可用权限",
                "Highest available startup permissions",
            ),
        }
    }

    pub(crate) fn apply_settings(self) -> &'static str {
        self.choose("应用已保存设置", "Apply saved settings")
    }

    pub(crate) fn startup_effective(self, state: crate::startup::StartupState) -> &'static str {
        use crate::startup::StartupState;
        match state {
            StartupState::Ready(true) | StartupState::Saved(true) => {
                self.choose("系统启动入口：已启用", "System startup entry: enabled")
            }
            StartupState::Ready(false) | StartupState::Saved(false) => {
                self.choose("系统启动入口：未启用", "System startup entry: disabled")
            }
            StartupState::Pending => {
                self.choose("系统启动入口：检测中", "System startup entry: checking")
            }
            StartupState::Failed => {
                self.choose("系统启动入口：未确认", "System startup entry: unknown")
            }
        }
    }

    pub(crate) fn settings_pending(self) -> &'static str {
        self.choose(
            "以下勾选项已保存，等待应用",
            "Checked settings are saved, pending application",
        )
    }

    pub(crate) fn settings_busy(self) -> &'static str {
        self.choose("正在处理设置…", "Processing settings…")
    }

    pub(crate) fn settings_saved_detail(self, pending: bool, automatic: bool) -> &'static str {
        if !pending {
            self.choose("当前已使用此设置", "These settings are already active")
        } else if automatic {
            self.choose(
                "已保存；正在等待自动重启应用",
                "Saved; waiting for automatic restart to apply",
            )
        } else {
            self.choose(
                "已保存；请使用托盘菜单“应用已保存设置”生效",
                "Saved; choose Apply saved settings from the tray menu",
            )
        }
    }
}
