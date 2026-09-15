use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use windows::Win32::{
    Foundation::WAIT_TIMEOUT,
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

use super::{
    protocol::{self, Signal},
    CHILD_ARGUMENT, TICK, WM_RESTART,
};
use crate::{config::reload::ConfigCandidate, utils::HandleWrapper, window_target::WindowTarget};

#[derive(Debug)]
pub(crate) enum ChildEvent {
    Activate,
    Commit,
    Abort,
}

pub(crate) struct ChildSession {
    events: Receiver<ChildEvent>,
    replies: SyncSender<Signal>,
    target: Arc<Mutex<Option<Arc<WindowTarget>>>>,
    canceled: Arc<AtomicBool>,
}

impl ChildSession {
    pub(crate) fn accept() -> Result<Option<(Self, Vec<u8>)>> {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        if arguments.is_empty() {
            return Ok(None);
        }
        if arguments.len() != 3 || arguments[0] != CHILD_ARGUMENT {
            bail!("启动参数无效；请直接运行 window-switcher.exe");
        }
        let parent = arguments[1]
            .to_str()
            .context("父进程编号无效")?
            .parse::<u32>()?;
        let timeout = arguments[2]
            .to_str()
            .context("交接时间无效")?
            .parse::<u32>()?;
        if parent == 0 || parent == std::process::id() || !(1000..=60000).contains(&timeout) {
            bail!("restart stage=arguments invalid parent or timeout");
        }
        let (snapshot_tx, snapshot_rx) = mpsc::sync_channel(1);
        let (signal_tx, signals) = mpsc::sync_channel(8);
        thread::Builder::new()
            .name("restart-parent-pipe".into())
            .spawn(move || {
                let mut input = std::io::stdin();
                let snapshot = protocol::read_snapshot(&mut input);
                let failed = snapshot.is_err();
                if snapshot_tx.send(snapshot).is_err() || failed {
                    return;
                }
                loop {
                    let signal = Signal::read(&mut input);
                    let failed = signal.is_err();
                    if signal_tx.send(signal).is_err() || failed {
                        break;
                    }
                }
            })?;
        let contents = snapshot_rx
            .recv_timeout(Duration::from_millis(timeout.into()))
            .context("restart stage=snapshot timeout")??;
        let (events_tx, events) = mpsc::sync_channel(4);
        let (replies, replies_rx) = mpsc::sync_channel(4);
        let target = Arc::new(Mutex::new(None));
        let canceled = Arc::new(AtomicBool::new(false));
        let worker = ChildWorker {
            signals,
            replies: replies_rx,
            events: events_tx,
            target: target.clone(),
            canceled: canceled.clone(),
            candidate: ConfigCandidate {
                generation: 0,
                contents: contents.clone().into(),
            },
            path: crate::config::get_config_path()?,
            deadline: Instant::now() + Duration::from_millis(timeout.into()),
        };
        thread::Builder::new()
            .name("restart-child-control".into())
            .spawn(move || {
                if let Err(err) = worker.run(parent) {
                    error!("restart stage=child-abort error={err:#}");
                    worker.canceled.store(true, Ordering::Release);
                    let _ = worker.post(ChildEvent::Abort);
                }
            })?;
        Ok(Some((
            Self {
                events,
                replies,
                target,
                canceled,
            },
            contents,
        )))
    }

    pub(crate) fn attach(&self, target: Arc<WindowTarget>) {
        *self.target.lock() = Some(target);
    }

    pub(crate) fn next_event(&self) -> Option<ChildEvent> {
        if self.canceled.load(Ordering::Acquire) {
            Some(ChildEvent::Abort)
        } else {
            self.events.try_recv().ok()
        }
    }

    pub(crate) fn prepared(&self) -> Result<()> {
        self.reply(Signal::Prepared)
    }
    pub(crate) fn active(&self) -> Result<()> {
        self.reply(Signal::Active)
    }
    pub(crate) fn committed(&self) -> Result<()> {
        self.reply(Signal::Committed)
    }

    fn reply(&self, signal: Signal) -> Result<()> {
        self.replies
            .try_send(signal)
            .context("restart stage=child-reply")
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

struct ChildWorker {
    signals: Receiver<Result<Signal>>,
    replies: Receiver<Signal>,
    events: SyncSender<ChildEvent>,
    target: Arc<Mutex<Option<Arc<WindowTarget>>>>,
    canceled: Arc<AtomicBool>,
    candidate: ConfigCandidate,
    path: PathBuf,
    deadline: Instant,
}

impl ChildWorker {
    fn run(&self, parent: u32) -> Result<()> {
        let parent =
            HandleWrapper::new(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent) }?);
        let mut output = std::io::stdout();
        self.reply(&parent, Signal::Prepared)?;
        Signal::Prepared.write(&mut output)?;
        self.command(&parent, Signal::Activate)?;
        self.require_current()?;
        self.post(ChildEvent::Activate)?;
        self.reply(&parent, Signal::Active)?;
        self.require_current()?;
        Signal::Active.write(&mut output)?;
        self.command(&parent, Signal::Commit)?;
        self.require_current()?;
        self.post(ChildEvent::Commit)?;
        self.reply(&parent, Signal::Committed)?;
        Signal::Committed.write(&mut output)?;
        // A committed child survives normal EOF / parent exit. Before commit,
        // either condition aborts. The parent still owns and can cancel this PID
        // until its UI has accepted the committed ACK.
        loop {
            match self.signals.recv_timeout(TICK) {
                Ok(Ok(Signal::Abort)) => bail!("restart stage=parent user exit"),
                Ok(Ok(_)) => bail!("restart stage=protocol unexpected post-commit command"),
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.canceled.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    if unsafe { WaitForSingleObject(parent.get_handle(), 0) } != WAIT_TIMEOUT {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn require_current(&self) -> Result<()> {
        if !self.candidate.matches_disk(&self.path)? {
            bail!("restart stage=child-superseded");
        }
        Ok(())
    }

    fn wait<T>(&self, receiver: &Receiver<T>, parent: &HandleWrapper) -> Result<T> {
        loop {
            if self.canceled.load(Ordering::Acquire) || Instant::now() >= self.deadline {
                bail!("restart stage=child-wait canceled or timed out");
            }
            if unsafe { WaitForSingleObject(parent.get_handle(), 0) } != WAIT_TIMEOUT {
                bail!("restart stage=parent exited before commit");
            }
            match receiver.recv_timeout(TICK) {
                Ok(value) => return Ok(value),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("restart stage=child-channel closed")
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn reply(&self, parent: &HandleWrapper, expected: Signal) -> Result<()> {
        if self.wait(&self.replies, parent)? != expected {
            bail!("restart stage=child-ui unexpected reply");
        }
        Ok(())
    }

    fn command(&self, parent: &HandleWrapper, expected: Signal) -> Result<()> {
        if self.wait(&self.signals, parent)?? != expected {
            bail!("restart stage=child-command unexpected or canceled");
        }
        Ok(())
    }

    fn post(&self, event: ChildEvent) -> Result<()> {
        self.events
            .try_send(event)
            .context("restart stage=child-ui-notify")?;
        if let Some(target) = self.target.lock().as_ref() {
            target.try_post(WM_RESTART);
        }
        Ok(())
    }
}
