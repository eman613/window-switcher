use std::{
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc::{Receiver, SyncSender},
        Arc,
    },
    time::Duration,
};

use anyhow::{bail, Result};

use crate::{config::reload::ConfigCandidate, window_target::WindowTarget};

mod child;
mod handshake;
mod parent;
mod protocol;

pub(crate) use child::{ChildEvent, ChildSession};
pub(crate) const CHILD_ARGUMENT: &str = "--restart-child";
pub(crate) const WM_RESTART: u32 = 6004;
const TICK: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub(crate) enum ParentEvent {
    Suspend,
    Ready,
    Done,
    Failed {
        message: String,
        safe_to_resume: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParentCommand {
    Suspended,
    Commit,
}

const UNDECIDED: u8 = 0;
const CANCELED: u8 = 1;
const ACCEPTED: u8 = 2;

#[derive(Default)]
struct Decision(AtomicU8);

impl Decision {
    fn cancel(&self) -> bool {
        self.0
            .compare_exchange(UNDECIDED, CANCELED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn accept(&self) -> bool {
        self.0
            .compare_exchange(UNDECIDED, ACCEPTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn state(&self) -> u8 {
        self.0.load(Ordering::Acquire)
    }
}

pub(crate) struct RestartController {
    pub(crate) candidate: ConfigCandidate,
    events: Receiver<ParentEvent>,
    commands: SyncSender<ParentCommand>,
    decision: Arc<Decision>,
}

impl RestartController {
    pub(crate) fn start(
        candidate: ConfigCandidate,
        path: std::path::PathBuf,
        timeout_ms: u32,
        latest: Arc<std::sync::atomic::AtomicU64>,
        target: Arc<WindowTarget>,
    ) -> Result<Self> {
        parent::start(candidate, path, timeout_ms, latest, target)
    }

    pub(crate) fn next_event(&self) -> Option<ParentEvent> {
        self.events.try_recv().ok()
    }

    fn send(&self, command: ParentCommand) -> Result<()> {
        if self.decision.state() != UNDECIDED {
            bail!("restart stage=control canceled");
        }
        self.commands
            .try_send(command)
            .map_err(|_| anyhow::anyhow!("restart stage=control unavailable"))
    }

    pub(crate) fn suspended(&self) -> Result<()> {
        self.send(ParentCommand::Suspended)
    }

    pub(crate) fn commit(&self) -> Result<()> {
        self.send(ParentCommand::Commit)
    }

    pub(crate) fn cancel(&self) {
        self.decision.cancel();
    }

    pub(crate) fn accept(&self) -> bool {
        self.decision.accept()
    }
}

impl Drop for RestartController {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_acceptance_and_worker_cancellation_have_exactly_one_winner() {
        for _ in 0..128 {
            let decision = Arc::new(Decision::default());
            let worker = decision.clone();
            let thread = std::thread::spawn(move || worker.cancel());
            let accepted = decision.accept();
            assert_ne!(accepted, thread.join().unwrap());
            assert_eq!(decision.state(), if accepted { ACCEPTED } else { CANCELED });
            assert!(!decision.cancel());
            assert!(!decision.accept());
        }
    }
}
