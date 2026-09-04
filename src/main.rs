#![windows_subsystem = "windows"]

use anyhow::{anyhow, bail, Context, Result};
use log::{info, warn};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    process::Command,
};

#[cfg(all(target_os = "windows", target_env = "gnu"))]
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

use window_switcher::{
    alert, load_config_report, set_language, start, start_with_config, text,
    utils::{is_running_as_admin, relaunch_as_admin, SingleInstance},
    AppExit, Config, ConfigDiagnostic, ConfigDiagnosticSeverity, ConfigLoadReport, ConfigSource,
    Language, TextId,
};

fn main() {
    set_language(Language::Auto);
    if let Err(err) = run() {
        alert!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    configure_dpi_awareness()?;

    let (config, source, diagnostics) = match load_config_report() {
        Ok(ConfigLoadReport {
            config,
            source,
            diagnostics,
        }) => (config, Some(source), diagnostics),
        Err(err) => {
            alert!("{}\n{err:#}", text(TextId::ConfigLoadFailed));
            (Config::default(), None, Vec::new())
        }
    };
    set_language(config.language);
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
    report_config_diagnostics(source.as_ref(), &diagnostics);
    let instance = SingleInstance::create("WindowSwitcherMutex")?;
    if !instance.is_single() {
        bail!("{}", text(TextId::AnotherInstance))
    }
    let exit = match source.as_ref() {
        Some(source) => start_with_config(&config, source)?,
        None => {
            start(&config)?;
            AppExit::Exit
        }
    };
    drop(instance);
    if exit == AppExit::Reload {
        restart_current_process()?;
    }
    Ok(())
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

fn report_config_diagnostics(source: Option<&ConfigSource>, diagnostics: &[ConfigDiagnostic]) {
    for diagnostic in diagnostics {
        match diagnostic.severity {
            ConfigDiagnosticSeverity::Info => info!("config diagnostic: {diagnostic}"),
            ConfigDiagnosticSeverity::Warning => warn!("config diagnostic: {diagnostic}"),
        }
    }

    let warnings: Vec<&ConfigDiagnostic> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .collect();
    if warnings.is_empty() {
        return;
    }

    let mut details = warnings
        .iter()
        .take(8)
        .map(|diagnostic| format!("- {diagnostic}"))
        .collect::<Vec<_>>();
    if warnings.len() > details.len() {
        details.push(format!(
            "- 另有 / additional {}",
            warnings.len() - details.len()
        ));
    }
    let path = source
        .map(|source| source.path.display().to_string())
        .unwrap_or_default();
    alert!(
        "{}\n{}\n{}",
        text(TextId::ConfigWarnings),
        path,
        details.join("\n")
    );
}

fn restart_current_process() -> Result<()> {
    let executable = std::env::current_exe().context("Failed to resolve current executable")?;
    Command::new(&executable).spawn().map_err(|err| {
        anyhow!(
            "{}\n{}\n{err}",
            text(TextId::RestartFailed),
            executable.display()
        )
    })?;
    Ok(())
}
