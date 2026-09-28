//! Pause uses the shared lazy settings worker and commits input only after save.
use crate::{
    config::{settings::save_input_paused, Config},
    keyboard::dispatch::WM_INPUT_READY,
    window_target::WindowTarget,
    worker::job::JobWorker,
};
use anyhow::Result;
use std::{path::PathBuf, sync::Arc};

pub(crate) struct PauseControl {
    worker: JobWorker<Config, Result<bool, String>>,
}

impl PauseControl {
    pub(crate) fn new(path: PathBuf, target: Arc<WindowTarget>) -> Self {
        Self {
            worker: JobWorker::new(
                "pause-settings",
                target,
                WM_INPUT_READY,
                move |expected: Config| {
                    let paused = !expected.input_paused;
                    save_input_paused(&path, &expected, paused)
                        .map(|()| paused)
                        .map_err(|error| format!("{error:#}"))
                },
            ),
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.worker.busy()
    }

    pub(crate) fn request(&mut self, config: &Config) -> Result<()> {
        if self.busy() {
            return Ok(());
        }
        self.worker.request(config.clone())
    }

    pub(crate) fn poll(&mut self) -> Option<Result<bool, String>> {
        self.worker.poll().map(|result| {
            result
                .map_err(|error| format!("{error:#}"))
                .and_then(|result| result)
        })
    }

    pub(crate) fn cancel(&mut self) {
        self.worker.cancel();
    }
}
