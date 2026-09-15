use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use anyhow::{bail, Result};
use windows::{
    core::w,
    Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
};

use super::transaction::{Backend, Entry};
use crate::{
    config::{reload::load_snapshot, Config},
    utils::{
        scheduled_task::{compare_task, query_task},
        RegKey,
    },
};

pub(super) struct WindowsBackend {
    pub(super) timeout: Duration,
    pub(super) canceled: Arc<AtomicBool>,
    pub(super) path: PathBuf,
    pub(super) configuration: Config,
}

impl Backend for WindowsBackend {
    fn read(&mut self, entry: Entry) -> Result<Option<String>> {
        match entry {
            Entry::Task => query_task(self.timeout, &self.canceled),
            Entry::Run => match RegKey::new_hkcu(
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                w!("Window Switcher"),
            ) {
                Ok(key) => key.get_string(),
                Err(error)
                    if error
                        .downcast_ref::<windows::core::Error>()
                        .is_some_and(|error| {
                            [
                                ERROR_FILE_NOT_FOUND.to_hresult(),
                                ERROR_PATH_NOT_FOUND.to_hresult(),
                            ]
                            .contains(&error.code())
                        }) =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            },
        }
    }

    fn exchange(
        &mut self,
        entry: Entry,
        expected: Option<&str>,
        replacement: Option<&str>,
    ) -> Result<Option<String>> {
        match entry {
            Entry::Task => compare_task(expected, replacement, self.timeout, &self.canceled),
            Entry::Run => {
                let key = RegKey::writable_hkcu(
                    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                    w!("Window Switcher"),
                )?;
                key.compare_string(expected, replacement)?;
                key.get_string()
            }
        }
    }

    fn current(&self) -> Result<()> {
        if self.canceled.load(std::sync::atomic::Ordering::Acquire) {
            bail!("自启动操作已取消");
        }
        let loaded = load_snapshot(&crate::config::read_config_bytes(&self.path)?, &self.path)?;
        let current = &loaded.config;
        if current.startup_enabled != self.configuration.startup_enabled
            || current.startup_run_level != self.configuration.startup_run_level
            || current.startup_battery_policy != self.configuration.startup_battery_policy
        {
            bail!("INI 自启动期望已变化；停止应用旧版本");
        }
        Ok(())
    }
}
