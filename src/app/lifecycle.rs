use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;

use super::App;
use crate::{
    config::{
        watch::{ConfigEvent, ConfigWatcher},
        LoadedConfig,
    },
    keyboard::InputActivation,
    localization::FailureKind,
    restart::{ChildEvent, ChildSession, ParentEvent, RestartController},
    utils::SingleInstance,
};

#[derive(Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Running,
    Preparing,
    Prepared,
    Activating,
    Active,
    Pausing,
    Suspended,
    Restoring,
}

#[derive(Default)]
pub(super) struct Lifecycle {
    pub(super) activation: Option<Arc<InputActivation>>,
    instance: Option<SingleInstance>,
    child: Option<ChildSession>,
    restart: Option<RestartController>,
    loaded: Option<LoadedConfig>,
    phase: Phase,
    services_started: bool,
    exit_requested: bool,
    restore_generation: Option<u64>,
    started: Option<Instant>,
}

impl Lifecycle {
    pub(super) fn new(
        instance: SingleInstance,
        child: Option<ChildSession>,
        loaded: &LoadedConfig,
    ) -> Self {
        Self {
            phase: if child.is_some() {
                Phase::Preparing
            } else {
                Phase::Running
            },
            instance: Some(instance),
            child,
            loaded: Some(loaded.clone()),
            started: Some(Instant::now()),
            ..Default::default()
        }
    }

    pub(super) fn is_replacement(&self) -> bool {
        self.child.is_some()
    }

    pub(super) fn can_change_settings(&self) -> bool {
        self.phase == Phase::Running && !self.exit_requested && self.restart.is_none()
    }

    pub(super) fn attach(&self, target: Arc<crate::window_target::WindowTarget>) {
        if let Some(child) = &self.child {
            child.attach(target);
        }
    }
}

impl App {
    pub(super) fn poll_lifecycle(&mut self) -> Result<()> {
        if self.lifecycle.activation.is_none() {
            return Ok(());
        }
        if self.lifecycle.phase == Phase::Preparing {
            self.lifecycle.child.as_ref().unwrap().prepared()?;
            self.lifecycle.phase = Phase::Prepared;
        }
        while let Some(event) = self
            .lifecycle
            .child
            .as_ref()
            .and_then(ChildSession::next_event)
        {
            match event {
                ChildEvent::Abort => {
                    self.request_exit();
                    return Ok(());
                }
                ChildEvent::Activate if self.lifecycle.phase == Phase::Prepared => {
                    self.lifecycle
                        .instance
                        .as_mut()
                        .context("缺少单实例所有权")?
                        .acquire()?;
                    self.activation().request(true)?;
                    self.lifecycle.phase = Phase::Activating;
                }
                ChildEvent::Commit if self.lifecycle.phase == Phase::Active => {
                    self.lifecycle.phase = Phase::Running;
                    self.start_services();
                    self.lifecycle.child.as_ref().unwrap().committed()?;
                }
                _ => anyhow::bail!("restart stage=child invalid lifecycle transition"),
            }
        }
        if self.lifecycle.phase == Phase::Activating && self.activation().acknowledged(true) {
            self.log_input_ready();
            self.lifecycle.child.as_ref().unwrap().active()?;
            self.lifecycle.phase = Phase::Active;
        }
        while let Some(event) = self
            .lifecycle
            .restart
            .as_ref()
            .and_then(RestartController::next_event)
        {
            match event {
                ParentEvent::Suspend if !self.lifecycle.exit_requested => {
                    self.activation().request(false)?;
                    self.lifecycle.phase = Phase::Pausing;
                }
                ParentEvent::Ready if !self.lifecycle.exit_requested => {
                    self.diagnostics.restart_ack();
                    self.lifecycle.restart.as_ref().unwrap().commit()?;
                }
                ParentEvent::Done if !self.lifecycle.exit_requested => {
                    if !self.lifecycle.restart.as_ref().unwrap().accept() {
                        continue;
                    }
                    let restart = self.lifecycle.restart.take().unwrap();
                    if let Some(watcher) = &self.config_watcher {
                        watcher.complete(restart.candidate.generation, true);
                    }
                    info!(
                        "restart stage=accepted generation={}",
                        restart.candidate.generation
                    );
                    self.finish_exit();
                    return Ok(());
                }
                ParentEvent::Failed {
                    message,
                    safe_to_resume,
                } => {
                    self.diagnostics.rollback();
                    if !safe_to_resume {
                        self.report_failure(FailureKind::RestartCleanup, &message);
                        continue;
                    }
                    let restart = self.lifecycle.restart.take().unwrap();
                    if self.lifecycle.exit_requested {
                        self.finish_exit();
                        return Ok(());
                    }
                    self.lifecycle.restore_generation = Some(restart.candidate.generation);
                    self.lifecycle.phase = Phase::Restoring;
                    self.report_failure(FailureKind::Restart, &message);
                }
                _ => {
                    if let Some(restart) = &self.lifecycle.restart {
                        restart.cancel();
                    }
                }
            }
        }
        if self.lifecycle.phase == Phase::Pausing && self.activation().acknowledged(false) {
            self.cancel_switch_app();
            for event in self.input.take() {
                self.input.acknowledge(event.session);
            }
            self.input_session = 0;
            self.switch_windows_state.modifier_released = true;
            self.lifecycle
                .instance
                .as_mut()
                .context("缺少旧实例所有权")?
                .release()?;
            self.lifecycle.phase = Phase::Suspended;
            self.lifecycle
                .restart
                .as_ref()
                .context("交接已取消")?
                .suspended()?;
            info!(
                "restart stage=input-suspended pid={} generation={}",
                std::process::id(),
                self.lifecycle
                    .restart
                    .as_ref()
                    .unwrap()
                    .candidate
                    .generation
            );
        }
        if self.lifecycle.phase == Phase::Restoring {
            self.lifecycle
                .instance
                .as_mut()
                .context("缺少旧实例所有权")?
                .acquire()?;
            if !self.activation().acknowledged(true) {
                self.activation().request(true)?;
                return Ok(());
            }
            if let Some(generation) = self.lifecycle.restore_generation.take() {
                if let Some(watcher) = &self.config_watcher {
                    watcher.complete(generation, false);
                }
                info!("restart stage=restored generation={generation}");
            }
            self.lifecycle.phase = Phase::Running;
        }
        if self.lifecycle.phase == Phase::Running && !self.lifecycle.exit_requested {
            self.start_services();
            self.poll_startup();
            self.poll_config();
        }
        Ok(())
    }

    fn activation(&self) -> &InputActivation {
        self.lifecycle.activation.as_deref().unwrap()
    }

    fn poll_config(&mut self) {
        if self.startup.busy() || self.pause.busy() || self.lifecycle.restart.is_some() {
            return;
        }
        let event = self
            .config_watcher
            .as_ref()
            .and_then(ConfigWatcher::next_event);
        match event {
            Some(ConfigEvent::Candidate(candidate)) => {
                let generation = candidate.generation;
                let loaded = self.lifecycle.loaded.as_ref().unwrap();
                let latest = self.config_watcher.as_ref().unwrap().latest();
                self.diagnostics.begin_restart();
                match RestartController::start(
                    candidate,
                    loaded.path.clone(),
                    self.config.config_restart_timeout_ms,
                    latest,
                    self.target.clone(),
                ) {
                    Ok(restart) => self.lifecycle.restart = Some(restart),
                    Err(error) => {
                        self.diagnostics.rollback();
                        self.config_watcher
                            .as_ref()
                            .unwrap()
                            .complete(generation, false);
                        self.report_failure(FailureKind::Restart, &format!("{error:#}"));
                    }
                }
            }
            Some(ConfigEvent::Invalid(message)) => self.report_config_error(&message),
            None => {}
        }
    }

    fn start_services(&mut self) {
        if self.lifecycle.services_started {
            return;
        }
        self.lifecycle.services_started = true;
        if self.lifecycle.child.is_none() {
            self.log_input_ready();
        }
        let Some(loaded) = self.lifecycle.loaded.clone() else {
            return;
        };
        if self.config.auto_restart {
            match ConfigWatcher::start(&loaded, self.target.clone()) {
                Ok(watcher) => self.config_watcher = Some(watcher),
                Err(error) => self.report_config_error(&format!("{error:#}")),
            }
        }
        match crate::startup::Startup::start(
            &self.config,
            loaded.path,
            self.target.clone(),
            self.is_admin,
        ) {
            Ok(startup) => self.startup = startup,
            Err(error) => {
                self.startup.state = crate::startup::StartupState::Failed;
                self.report_failure(FailureKind::Startup, &format!("{error:#}"));
            }
        }
        self.set_trayicon();
    }

    fn poll_startup(&mut self) {
        use crate::startup::StartupUpdate;
        match self.startup.poll() {
            Some(StartupUpdate::Inspected(result)) => {
                self.diagnostics.auxiliary_completed(result.is_ok());
                match result {
                    Ok(enabled) => info!("startup stage=auxiliary-complete enabled={enabled}"),
                    Err(error) => self.report_failure(FailureKind::Startup, &error),
                }
            }
            Some(StartupUpdate::Saved(Ok(()))) if !self.config.auto_restart => self.notify(
                self.text.config_saved(),
                self.text.restart_required(),
                false,
            ),
            Some(StartupUpdate::Saved(Err(error))) => self.report_config_error(&error),
            _ => {}
        }
    }

    pub(super) fn log_input_ready(&mut self) {
        info!(
            "startup stage=input-ready pid={} app_elapsed_us={}",
            std::process::id(),
            self.lifecycle
                .started
                .map_or(0, |started| started.elapsed().as_micros())
        );
        self.diagnostics.input_ready();
    }

    pub(super) fn request_exit(&mut self) {
        self.lifecycle.exit_requested = true;
        self.startup.cancel();
        self.pause.cancel();
        self.config_watcher.take();
        if let Some(activation) = &self.lifecycle.activation {
            let _ = activation.request(false);
        }
        self.cancel_switch_app();
        if let Some(restart) = &self.lifecycle.restart {
            restart.cancel();
        } else {
            self.finish_exit();
        }
    }

    pub(super) fn lifecycle_error(&mut self, error: &anyhow::Error) {
        self.report_failure(FailureKind::Restart, &format!("{error:#}"));
        if let Some(restart) = &self.lifecycle.restart {
            restart.cancel();
        }
        if self.lifecycle.child.is_some() && !self.lifecycle.services_started {
            self.request_exit();
        }
    }

    fn finish_exit(&self) {
        self.target.close();
        unsafe { PostQuitMessage(0) };
    }
}
