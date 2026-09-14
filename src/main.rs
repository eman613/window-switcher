#![windows_subsystem = "windows"]

use anyhow::{bail, Context, Result};

use window_switcher::{
    alert, load_config, prepare_log_file, start, utils::SingleInstance, wait_for_restart_parent,
};

fn main() {
    if let Err(err) = run() {
        alert!("{err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    wait_for_restart_parent()?;
    let instance = SingleInstance::create("WindowSwitcherMutex")?;
    if !instance.is_single() {
        bail!("应用已经在运行，本次启动已取消。")
    }
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
        );
    }

    let loaded = load_config()?;
    if let Some(log_file) = &loaded.config.log_file {
        let file = prepare_log_file(log_file).with_context(|| {
            format!(
                "无法写入日志 '{}'，请检查目录和权限；INI 原值未重置",
                log_file.display()
            )
        })?;
        simple_logging::log_to(file, loaded.config.log_level);
    }
    log::info!(
        "config stage=loaded path={} supplemented={} pid={}",
        loaded.path.display(),
        loaded.migrated,
        std::process::id()
    );
    start(&loaded)
}
