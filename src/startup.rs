use anyhow::Result;
use windows::core::{w, PCWSTR};

use crate::{
    localization::{text, TextId},
    utils::{
        create_scheduled_task, delete_scheduled_task, get_exe_path, scheduled_task_state, RegKey,
    },
};

const TASK_NAME: &str = "WindowSwitcher";
const HKEY_RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const HKEY_NAME: PCWSTR = w!("Window Switcher");

#[derive(Default)]
pub struct Startup {
    pub is_admin: bool,
    pub is_enable: bool,
    pub exe_path: Vec<u16>,
}

impl Startup {
    pub fn init(is_admin: bool, desired: Option<bool>) -> Result<Self> {
        let exe_path = get_exe_path()?;
        let is_enable = if is_admin {
            let state = scheduled_task_state(TASK_NAME, &String::from_utf16_lossy(&exe_path))?;
            state.exists && state.owned && state.enabled
        } else {
            reg_is_enable(&exe_path)?
        };
        let mut startup = Self {
            is_admin,
            is_enable,
            exe_path,
        };
        if let Some(desired) = desired {
            if desired != startup.is_enable {
                startup.set_enabled(desired)?;
            }
        }
        Ok(startup)
    }

    pub fn toggle(&mut self) -> Result<()> {
        self.set_enabled(!self.is_enable)
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        if self.is_admin {
            self.set_admin_enabled(enabled)
        } else {
            self.set_user_enabled(enabled)
        }
    }

    fn set_admin_enabled(&mut self, enabled: bool) -> Result<()> {
        let exe_path = String::from_utf16_lossy(&self.exe_path);
        let state = scheduled_task_state(TASK_NAME, &exe_path)?;
        if state.exists && !state.owned {
            alert!("{}", text(TextId::StartupConflict));
            return Ok(());
        }
        if !enabled {
            let registry_was_enabled = reg_is_enable(&self.exe_path)?;
            if registry_was_enabled {
                reg_disable(&self.exe_path)?;
            }
            if state.exists {
                if let Err(error) = delete_scheduled_task(TASK_NAME) {
                    if registry_was_enabled {
                        let _ = reg_enable(&self.exe_path);
                    }
                    return Err(error);
                }
            }
            self.is_enable = false;
            return Ok(());
        }

        let registry_was_enabled = reg_is_enable(&self.exe_path)?;
        if registry_was_enabled {
            reg_disable(&self.exe_path)?;
        }
        if let Err(error) = create_scheduled_task(TASK_NAME, &exe_path) {
            if registry_was_enabled {
                let _ = reg_enable(&self.exe_path);
            }
            return Err(error);
        }
        let verified = scheduled_task_state(TASK_NAME, &exe_path)
            .map(|task| task.exists && task.owned && task.enabled);
        if !matches!(verified, Ok(true)) {
            let _ = delete_scheduled_task(TASK_NAME);
            if registry_was_enabled {
                let _ = reg_enable(&self.exe_path);
            }
            return Err(verified.err().unwrap_or_else(|| {
                anyhow::anyhow!("Scheduled task was not enabled after creation")
            }));
        }
        self.is_enable = true;
        Ok(())
    }

    fn set_user_enabled(&mut self, enabled: bool) -> Result<()> {
        if enabled {
            let state = scheduled_task_state(TASK_NAME, &String::from_utf16_lossy(&self.exe_path))?;
            if state.exists {
                alert!("{}", text(TextId::StartupConflict));
                return Ok(());
            }
            reg_enable(&self.exe_path)?;
            if !reg_is_enable(&self.exe_path)? {
                let _ = reg_disable(&self.exe_path);
                return Err(anyhow::anyhow!("Registry startup value was not persisted"));
            }
            self.is_enable = true;
        } else {
            reg_disable(&self.exe_path)?;
            self.is_enable = false;
        }
        Ok(())
    }
}

fn reg_key() -> Result<RegKey> {
    RegKey::open_hkcu_read(HKEY_RUN, HKEY_NAME)
}

fn reg_is_enable(exe_path: &[u16]) -> Result<bool> {
    let key = reg_key()?;
    let value = match key.get_value()? {
        Some(value) => value,
        None => return Ok(false),
    };
    let actual = String::from_utf16_lossy(&value);
    let expected = String::from_utf16_lossy(exe_path);
    Ok(same_run_path(&actual, &expected))
}

fn reg_enable(exe_path: &[u16]) -> Result<()> {
    let key = RegKey::new_hkcu(HKEY_RUN, HKEY_NAME)?;
    let path = String::from_utf16_lossy(exe_path);
    let quoted = format!("\"{path}\"");
    let path_utf16 = quoted.encode_utf16().collect::<Vec<_>>();
    let bytes = unsafe { path_utf16.align_to::<u8>().1 };
    key.set_value(bytes)?;
    Ok(())
}

fn reg_disable(exe_path: &[u16]) -> Result<()> {
    let key = RegKey::new_hkcu(HKEY_RUN, HKEY_NAME)?;
    let value = key.get_value()?;
    let Some(value) = value else {
        return Ok(());
    };
    let actual = String::from_utf16_lossy(&value);
    let expected = String::from_utf16_lossy(exe_path);
    if !same_run_path(&actual, &expected) {
        return Ok(());
    }
    key.delete_value()?;
    Ok(())
}

fn same_run_path(actual: &str, expected: &str) -> bool {
    actual
        .trim()
        .trim_matches('"')
        .replace('/', "\\")
        .eq_ignore_ascii_case(
            expected
                .trim()
                .trim_matches('"')
                .replace('/', "\\")
                .as_str(),
        )
}
