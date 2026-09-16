use super::App;
use anyhow::{ensure, Result};

impl App {
    pub(super) fn toggle_pause(&mut self) -> Result<()> {
        ensure!(
            self.lifecycle.can_change_settings(),
            "应用正在交接或退出；暂停状态未修改"
        );
        if self.pause.busy() {
            return Ok(());
        }
        self.complete_switch();
        self.pause.request(&self.config)?;
        debug!("pause stage=save-requested");
        Ok(())
    }

    pub(super) fn poll_pause(&mut self) {
        if self.input.take_pause_request() {
            if let Err(error) = self.toggle_pause() {
                self.report_config_error(&format!("{error:#}"));
            }
        }
        match self.pause.poll() {
            Some(Ok(paused)) => {
                self.input.set_paused(paused);
                self.complete_switch();
                for event in self.input.take() {
                    self.input.acknowledge(event.session);
                }
                self.config.input_paused = paused;
                info!("pause stage=applied paused={paused} persisted=true");
            }
            Some(Err(message)) => {
                warn!("pause stage=save-failed state-preserved=true");
                self.report_config_error(&message);
            }
            None => {}
        }
    }
}
