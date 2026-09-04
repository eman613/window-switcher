#![windows_subsystem = "windows"]

use anyhow::{anyhow, bail, Result};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

#[cfg(all(target_os = "windows", target_env = "gnu"))]
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

use window_switcher::{
    alert, load_config, start,
    utils::{is_running_as_admin, relaunch_as_admin, SingleInstance},
    Config,
};

fn main() {
    if let Err(err) = run() {
        alert!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    configure_dpi_awareness()?;

    let config = match load_config() {
        Ok(config) => config,
        Err(err) => {
            alert!("Failed to load configuration. Default settings will be used.\n{err}");
            Config::default()
        }
    };
    if config.run_as_admin && !is_running_as_admin()? {
        relaunch_as_admin()?;
        return Ok(());
    }

    if let Some(log_file) = &config.log_file {
        let file = prepare_log_file(log_file).map_err(|err| {
            anyhow!(
                "Failed to prepare log file at {}, {err}",
                log_file.display()
            )
        })?;
        simple_logging::log_to(file, config.log_level);
    }
    let instance = SingleInstance::create("WindowSwitcherMutex")?;
    if !instance.is_single() {
        bail!("Another instance is running. This instance will abort.")
    }
    start(&config)
}

#[cfg(all(target_os = "windows", target_env = "gnu"))]
fn configure_dpi_awareness() -> Result<()> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
        .map_err(|err| anyhow!("Failed to enable Per-Monitor V2 DPI awareness: {err}"))
}

#[cfg(not(all(target_os = "windows", target_env = "gnu")))]
fn configure_dpi_awareness() -> Result<()> {
    Ok(())
}

fn prepare_log_file(path: &Path) -> std::io::Result<File> {
    if path.exists() {
        OpenOptions::new().append(true).open(path)
    } else {
        File::create(path)
    }
}
