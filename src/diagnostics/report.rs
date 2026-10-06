//! Explicit, bounded allowlist. Never serialize Config, window records or logs.
use crate::{config::Config, startup::StartupState};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const MAX_REPORT_BYTES: usize = 256 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct ReportSnapshot {
    fields: Vec<(&'static str, String)>,
}
impl ReportSnapshot {
    pub(crate) fn capture(
        config: &Config,
        elevated: bool,
        pending: bool,
        startup: StartupState,
        dpi: Option<u32>,
        groups: Option<usize>,
    ) -> Self {
        let mut fields = vec![
            ("report_format", "1".into()),
            ("version", env!("CARGO_PKG_VERSION").into()),
            ("architecture", std::env::consts::ARCH.into()),
        ];
        macro_rules! value {
            ($key:literal, $value:expr) => {
                fields.push(($key, format!("{}", $value)));
            };
        }
        value!("elevated", elevated);
        value!("pending_settings", pending);
        value!(
            "startup_effective",
            match startup {
                StartupState::Pending => "pending",
                StartupState::Ready(true) => "enabled",
                StartupState::Ready(false) => "disabled",
                StartupState::Saved(_) => "saved-awaiting-restart",
                StartupState::Failed => "query-failed",
            }
        );
        value!("startup_desired", config.startup_enabled);
        value!("startup_run_level", config.startup_run_level);
        value!("input_paused", config.input_paused);
        value!("injected_events", config.injected_events);
        value!("switch_windows_enabled", config.switch_windows_enable);
        value!("switch_apps_enabled", config.switch_apps_enable);
        value!("search_enabled", config.search_enable);
        value!("search_pinyin", config.search_pinyin);
        value!("details_enabled", config.details_enable);
        value!("close_enabled", config.close_enable);
        value!("close_confirmation", config.close_confirm);
        value!("preview_enabled", config.preview_enable);
        value!("search_match", config.search_match);
        value!("search_fields", config.search_fields);
        value!("search_max_results", config.search_max_results);
        value!("search_visible_rows", config.search_visible_rows);
        value!("grouping", config.switch_apps_grouping);
        value!("order", config.switch_apps_order);
        value!("monitor_scope", config.switch_apps_monitor_filter);
        value!("include_topmost", config.switch_apps_include_topmost);
        value!(
            "include_tool_windows",
            config.switch_apps_include_tool_windows
        );
        value!(
            "include_hidden_minimized",
            config.switch_apps_include_hidden_minimized
        );
        value!(
            "title_exclusion_count",
            config.switch_apps_exclude_titles.len()
        );
        value!(
            "process_exclusion_count",
            config.switch_apps_exclude_processes.len()
        );
        value!("app_hotkey_count", config.switch_apps_hotkey.len());
        value!(
            "icon_override_count",
            config.switch_apps_override_icons.len()
        );
        value!(
            "badge_private_font_configured",
            config.badge_font_file.is_some()
        );
        value!(
            "name_private_font_configured",
            config.app_name_font_file.is_some()
        );
        value!(
            "chrome_custom_root_configured",
            config.chrome_user_data_dir.is_some()
        );
        value!(
            "edge_custom_root_configured",
            config.edge_user_data_dir.is_some()
        );
        value!("theme", config.theme);
        value!("unified_font", config.unified_font);
        value!("font_size", config.ui_font_size);
        value!("auto_restart", config.auto_restart);
        value!("restart_delay_ms", config.restart_delay_ms);
        value!("icon_query_timeout_ms", config.icon_query_timeout_ms);
        value!("metrics_enabled", config.metrics_enabled);
        value!(
            "snapshot_groups",
            groups.map_or_else(|| "not-collected".into(), |v| v.to_string())
        );
        value!(
            "panel_dpi",
            dpi.map_or_else(|| "not-collected".into(), |v| v.to_string())
        );
        value!("worker_queue_statistics", "not-collected");
        Self { fields }
    }
    pub(crate) fn runtime(
        &mut self,
        diagnostics: &super::Diagnostics,
        input: &crate::keyboard::dispatch::InputDispatch,
        snapshot_healthy: bool,
    ) {
        self.fields.extend(
            input
                .diagnostic_counts()
                .map(|(key, count)| (key, count.to_string())),
        );
        self.fields
            .push(("snapshot_worker_available", snapshot_healthy.to_string()));
        if diagnostics.enabled {
            self.fields
                .push(("restart_ack_count", diagnostics.restart_acks.to_string()));
            self.fields
                .push(("rollback_count", diagnostics.rollbacks.to_string()));
            self.fields
                .push(("input_ready", diagnostics.ready.to_string()));
        } else {
            self.fields
                .push(("runtime_counters", "not-collected".into()));
        }
    }
    pub(crate) fn render(&self) -> Result<String, String> {
        let mut text = String::from("Window Switcher diagnostic report (local, redacted)\nNo titles, queries, usernames, paths, raw INI or raw logs are included.\n\n");
        for (key, value) in &self.fields {
            text.push_str(&format!("{key} = {value}\n"));
        }
        if text.len() > MAX_REPORT_BYTES {
            return Err("report-size-limit".into());
        }
        Ok(text)
    }
    pub(crate) fn environment(&mut self) {
        let version = crate::utils::os_version_info()
            .map(|v| {
                format!(
                    "{}.{}.{}",
                    v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber
                )
            })
            .unwrap_or_else(|| "unknown".into());
        self.fields.push(("windows_version", version));
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CMONITORS, SM_REMOTESESSION,
        };
        self.fields.push((
            "rdp_session",
            (unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0).to_string(),
        ));
        self.fields.push((
            "monitor_count",
            unsafe { GetSystemMetrics(SM_CMONITORS) }.to_string(),
        ));
    }
}
pub(crate) fn report_directory() -> Result<PathBuf, String> {
    crate::utils::get_exe_folder()
        .map(|directory| directory.join("Reports"))
        .map_err(|_| "report-executable-directory-unavailable".into())
}
pub(crate) fn write_report(
    directory: &Path,
    text: &str,
    allowed: impl Fn() -> bool,
) -> Result<PathBuf, String> {
    if text.len() > MAX_REPORT_BYTES || !allowed() {
        return Err("report-cancelled-or-too-large".into());
    }
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("report-directory code={:?}", e.raw_os_error()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "report-clock")?
        .as_nanos();
    for _ in 0..8 {
        let path = directory.join(format!(
            "report-{stamp}-{}.txt",
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("report-create code={:?}", e.raw_os_error())),
        };
        let result = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        if result.is_err() || !allowed() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("report-cleanup code={:?}", e.raw_os_error()))?;
            return Err("report-write-failed-or-cancelled".into());
        }
        return Ok(path);
    }
    Err("report-name-collision".into())
}
pub(crate) fn export(
    mut snapshot: ReportSnapshot,
    allowed: impl Fn() -> bool,
) -> Result<PathBuf, String> {
    snapshot.environment();
    let text = snapshot.render()?;
    write_report(&report_directory()?, &text, allowed)
}

#[cfg(test)]
mod tests;
