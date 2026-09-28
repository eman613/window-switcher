use std::{path::PathBuf, sync::Arc};

use anyhow::{ensure, Result};

use super::App;
use crate::{
    config::{quick::QuickSetting, settings, Config, LoadedConfig},
    localization::FailureKind,
    window_target::WindowTarget,
    worker::job::JobWorker,
};

enum SettingsRequest {
    Save {
        expected: Box<Config>,
        setting: QuickSetting,
    },
    Read {
        elevate: bool,
    },
}

enum SettingsAction {
    Saved,
    Restart { elevate: bool },
}
struct SettingsResult {
    loaded: LoadedConfig,
    action: SettingsAction,
}

pub(super) struct QuickSettingsState {
    worker: JobWorker<SettingsRequest, Result<SettingsResult>>,
    saved: Option<Config>,
}

impl QuickSettingsState {
    pub(super) fn new(path: PathBuf, target: Arc<WindowTarget>) -> Self {
        Self {
            worker: JobWorker::new(
                "quick-settings",
                target,
                super::WM_USER_CONFIG_CHANGED,
                move |request| match request {
                    SettingsRequest::Save { expected, setting } => Ok(SettingsResult {
                        loaded: settings::save_quick(&path, &expected, setting)?,
                        action: SettingsAction::Saved,
                    }),
                    SettingsRequest::Read { elevate } => Ok(SettingsResult {
                        loaded: settings::read_current(&path)?,
                        action: SettingsAction::Restart { elevate },
                    }),
                },
            ),
            saved: None,
        }
    }

    pub(super) fn configuration<'a>(&'a self, running: &'a Config) -> &'a Config {
        self.saved.as_ref().unwrap_or(running)
    }

    pub(super) fn pending(&self, running: &Config) -> bool {
        self.saved.as_ref().is_some_and(|saved| saved != running)
    }

    pub(super) fn busy(&self) -> bool {
        self.worker.busy()
    }
    pub(super) fn cancel(&mut self) {
        self.worker.cancel();
    }

    pub(super) fn pause_applied(&mut self, paused: bool) {
        if let Some(saved) = &mut self.saved {
            saved.input_paused = paused;
        }
    }

    pub(super) fn remember_saved(&mut self, configuration: Config) {
        self.saved = Some(configuration);
    }
}

impl App {
    fn require_settings_ready(&self) -> Result<()> {
        ensure!(
            self.lifecycle.can_change_settings()
                && !self.startup.busy()
                && !self.pause.busy()
                && !self.quick_settings.busy(),
            "设置操作正在进行；请稍后重试"
        );
        Ok(())
    }

    pub(super) fn change_setting(&mut self, setting: QuickSetting) -> Result<()> {
        self.require_settings_ready()?;
        let expected = Box::new(self.quick_settings.configuration(&self.config).clone());
        self.complete_switch();
        self.quick_settings
            .worker
            .request(SettingsRequest::Save { expected, setting })?;
        let (section, key) = setting.key();
        debug!("settings stage=save-request section={section} key={key}");
        Ok(())
    }

    pub(super) fn read_saved_settings(&mut self, elevate: bool) -> Result<()> {
        self.require_settings_ready()?;
        ensure!(!elevate || !self.is_admin, "当前已经以管理员权限运行");
        self.complete_switch();
        self.quick_settings
            .worker
            .request(SettingsRequest::Read { elevate })
    }

    pub(super) fn toggle_startup(&mut self) -> Result<()> {
        self.require_settings_ready()?;
        self.startup
            .toggle(self.quick_settings.configuration(&self.config))
    }

    pub(super) fn poll_settings(&mut self) {
        let Some(result) = self.quick_settings.worker.poll() else {
            return;
        };
        match result.and_then(|result| result) {
            Ok(SettingsResult { loaded, action }) => {
                self.quick_settings.saved = Some(loaded.config);
                match action {
                    SettingsAction::Saved => {
                        let pending = self.quick_settings.pending(&self.config);
                        self.notify(
                            self.text.config_saved(),
                            self.text
                                .settings_saved_detail(pending, self.config.auto_restart),
                            false,
                        );
                    }
                    SettingsAction::Restart { elevate } => {
                        if let Err(error) = self.restart_saved(loaded.contents.into(), elevate) {
                            self.report_failure(FailureKind::Restart, &format!("{error:#}"));
                        }
                    }
                }
            }
            Err(error) => self.report_config_error(&format!("{error:#}")),
        }
    }
}
