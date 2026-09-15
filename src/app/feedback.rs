use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::Mutex;

use super::App;

#[derive(Default)]
struct PopupState {
    active: bool,
    pending: Option<String>,
}

#[derive(Default)]
pub(super) struct Feedback {
    popup: Arc<Mutex<PopupState>>,
    pub(super) retry_at: Option<Instant>,
    pub(super) retry_count: u32,
}

impl App {
    pub(super) fn set_trayicon(&mut self) {
        self.feedback.retry_at = None;
        if let Some(trayicon) = self.trayicon.as_mut() {
            match trayicon.register(self.hwnd) {
                Ok(()) => {
                    self.feedback.retry_count = 0;
                    info!("trayicon stage=registered");
                }
                Err(_) if !trayicon.exist() && self.feedback.retry_count < 5 => {
                    self.feedback.retry_count += 1;
                    self.feedback.retry_at = Some(Instant::now() + Duration::from_secs(3));
                    warn!(
                        "trayicon stage=register retry={}",
                        self.feedback.retry_count
                    );
                }
                Err(_) => warn!("trayicon stage=register unavailable"),
            }
        }
    }

    pub(super) fn poll_feedback(&mut self) {
        if self
            .feedback
            .retry_at
            .is_some_and(|at| Instant::now() >= at)
        {
            self.set_trayicon();
        }
    }

    pub(super) fn report_config_error(&mut self, message: &str) {
        self.report_failure(crate::localization::FailureKind::Configuration, message);
    }

    pub(super) fn report_failure(&mut self, kind: crate::localization::FailureKind, detail: &str) {
        error!("application stage=operation-failed");
        self.notify(
            self.text.error_title(),
            &self.text.failure(kind, detail),
            true,
        );
    }

    pub(super) fn notify(&mut self, title: &str, message: &str, error: bool) {
        if self
            .trayicon
            .as_ref()
            .is_some_and(|tray| tray.notify(title, message, error).is_ok())
        {
            return;
        }
        let mut state = self.feedback.popup.lock();
        state.pending = Some(message.chars().take(4096).collect());
        if state.active {
            return;
        }
        state.active = true;
        drop(state);
        let pending = self.feedback.popup.clone();
        let title = self.text.error_title();
        if let Err(error) = std::thread::Builder::new()
            .name("app-notification".into())
            .spawn(move || loop {
                let message = {
                    let mut state = pending.lock();
                    match state.pending.take() {
                        Some(message) => message,
                        None => {
                            state.active = false;
                            return;
                        }
                    }
                };
                crate::macros::message_box_with_title(title, &message);
            })
        {
            self.feedback.popup.lock().active = false;
            error!("notification stage=worker error={error}");
        }
    }
}
