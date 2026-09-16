use super::App;
use crate::{
    keyboard::state::{InputAction, InputEvent, SwitchKind},
    utils::{get_foreground_window, window_identity::WindowIdentity},
};
use anyhow::Result;
use std::time::Instant;

impl App {
    pub(super) fn drain_input(&mut self) {
        if let Err(error) = self.poll_lifecycle() {
            self.lifecycle_error(&error);
        }
        if !self.target.is_live() {
            return;
        }
        self.poll_feedback();
        self.switching.paint_dirty |= self.painter.poll_fonts();
        if self.diagnostics.tick() {
            self.input.log_summary();
        }
        if let Some(code) = crate::config::take_log_failure() {
            self.notify(self.text.error_title(), &self.text.log_failure(code), true);
        }
        if self.input_session != 0 && !self.input.permits(self.input_session) {
            self.complete_switch();
        }
        // Drain input before accepting results, so queued cancellation/release
        // wins over a late worker completion in the same UI turn.
        for received in self.input.take_timed() {
            let event = received.event;
            if !self.input.permits(event.session) {
                self.input.acknowledge(event.session);
                continue;
            }
            if self.input_session != event.session {
                self.cancel_switch_app();
                self.input_session = event.session;
                let foreground = get_foreground_window();
                self.switching.anchor = foreground.0 as usize;
                self.switching.return_focus =
                    WindowIdentity::capture(foreground, &self.snapshots.lifetimes);
            }
            if let Err(error) = self.apply_input(event, received.received) {
                error!("input stage=apply error={error:#}");
                self.input.cancel(event.session);
                self.complete_switch();
            }
        }
        self.poll_accessibility();
        if self.input_session == 0 {
            return;
        }
        let result = self
            .poll_switching()
            .and_then(|()| self.poll_search())
            .and_then(|()| self.pump_switches())
            .and_then(|()| self.flush_panel());
        if let Err(error) = result {
            error!("input stage=background-apply error={error:#}");
            self.input.cancel(self.input_session);
            self.complete_switch();
        }
    }

    fn apply_input(&mut self, event: InputEvent, received: Option<Instant>) -> Result<()> {
        match event.action {
            InputAction::Cycle(SwitchKind::Search, _) => self.start_search()?,
            InputAction::Cycle(kind, reverse) => {
                self.switching.push(kind, reverse, received)?;
                self.pump_switches()?;
            }
            InputAction::Finish(kind) => {
                self.switching.finishing = Some(kind);
                if self
                    .switching
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.refresh)
                {
                    self.snapshots.cancel();
                    self.switching.pending = None;
                }
                self.pump_switches()?;
            }
            InputAction::Cancel => self.complete_switch(),
        }
        Ok(())
    }

    pub(super) fn complete_switch(&mut self) {
        self.input.acknowledge(self.input_session);
        self.input_session = 0;
        self.switch_windows_state.modifier_released = true;
        self.cancel_switch_app();
    }
}
