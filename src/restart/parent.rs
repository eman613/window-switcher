use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use super::{
    handshake::{self, Step},
    protocol::{self, Signal},
    Decision, ParentCommand, ParentEvent, RestartController, ACCEPTED, CANCELED, CHILD_ARGUMENT,
    TICK, WM_RESTART,
};
use crate::{
    config::reload::{self, ConfigCandidate},
    window_target::WindowTarget,
};

pub(super) fn start(
    candidate: ConfigCandidate,
    path: PathBuf,
    timeout_ms: u32,
    latest: Arc<AtomicU64>,
    target: Arc<WindowTarget>,
) -> Result<RestartController> {
    let (event_tx, events) = mpsc::sync_channel(4);
    let (commands, command_rx) = mpsc::sync_channel(2);
    let decision = Arc::new(Decision::default());
    let worker = Worker {
        candidate: candidate.clone(),
        path,
        latest,
        target,
        decision: decision.clone(),
        deadline: Instant::now() + Duration::from_millis(timeout_ms.into()),
        events: event_tx,
        commands: command_rx,
    };
    thread::Builder::new()
        .name("config-restart".into())
        .spawn(move || worker.run(timeout_ms))
        .context("无法启动配置交接工作者；旧实例继续运行")?;
    Ok(RestartController {
        candidate,
        events,
        commands,
        decision,
    })
}

struct Worker {
    candidate: ConfigCandidate,
    path: PathBuf,
    latest: Arc<AtomicU64>,
    target: Arc<WindowTarget>,
    decision: Arc<Decision>,
    deadline: Instant,
    events: SyncSender<ParentEvent>,
    commands: Receiver<ParentCommand>,
}

impl Worker {
    fn run(self, timeout_ms: u32) {
        let command = (|| -> Result<Command> {
            let executable = std::env::current_exe()?;
            let mut command = Command::new(&executable);
            command
                .args([
                    CHILD_ARGUMENT,
                    &std::process::id().to_string(),
                    &timeout_ms.to_string(),
                ])
                .current_dir(executable.parent().context("程序缺少目录")?);
            Ok(command)
        })();
        self.run_command(command);
    }

    fn run_command(self, command: Result<Command>) {
        let mut child = None;
        let result = self.handoff(command, &mut child);
        if let Err(error) = result {
            self.decision.cancel();
            let cleanup = child.as_mut().map_or(Ok(()), OwnedChild::abort);
            let safe_to_resume = cleanup.is_ok();
            let message = match cleanup {
                Ok(()) => format!("配置重启未完成：{error:#}；旧实例保留，可自动重试"),
                Err(cleanup) => {
                    format!("配置交接清理失败：{cleanup:#}；尚未恢复旧实例输入，避免重复接管")
                }
            };
            warn!(
                "restart stage=rollback generation={} child_stopped={safe_to_resume}",
                self.candidate.generation
            );
            let _ = self.event(ParentEvent::Failed {
                message,
                safe_to_resume,
            });
        }
    }

    fn handoff(&self, command: Result<Command>, owned: &mut Option<OwnedChild>) -> Result<()> {
        self.check()?;
        reload::preflight(&self.candidate, &self.path).context("restart stage=preflight")?;
        let child = command?
            .creation_flags(CREATE_NO_WINDOW.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("restart stage=spawn")?;
        *owned = Some(OwnedChild { child, keep: false });
        let child = owned.as_mut().unwrap();
        info!(
            "restart stage=spawn generation={} child_pid={}",
            self.candidate.generation,
            child.child.id()
        );
        let output = protocol::reader(child.child.stdout.take().context("缺少子进程输出管道")?)?;
        let mut input = child.child.stdin.take().context("缺少子进程输入管道")?;
        let bytes = self.candidate.contents.clone();
        let (sent, snapshot_sent) = mpsc::sync_channel(1);
        // A full pipe must not hide a hung preflight from the timeout/cancel loop.
        thread::Builder::new()
            .name("restart-snapshot".into())
            .spawn(move || {
                let result = protocol::write_snapshot(&mut input, &bytes);
                let _ = sent.send(result.map(|()| input));
            })?;
        let mut input = self.wait(&snapshot_sent, child, "snapshot")??;
        handshake::negotiate(|step| match step {
            Step::Wait(signal) => self.expect(&output, child, signal),
            Step::CheckCurrent => self.require_current(),
            Step::Suspend => self.event(ParentEvent::Suspend),
            Step::WaitUi(command) => self.command(child, command),
            Step::Send(signal) => signal.write(&mut input),
            Step::Ready => {
                info!(
                    "restart stage=active-ack generation={} child_pid={}",
                    self.candidate.generation,
                    child.child.id()
                );
                self.event(ParentEvent::Ready)
            }
            Step::Done => self.event(ParentEvent::Done),
        })?;
        // Keep ownership of the child until the UI accepts the ACK. A user exit
        // processed first sets canceled and the child is terminated before exit.
        loop {
            match self.decision.state() {
                ACCEPTED => break,
                CANCELED => bail!("restart stage=commit canceled before acceptance"),
                _ => {}
            }
            // Acceptance and timeout/exit compete for one terminal decision.
            // Closing the old UI after acceptance must never kill the new owner.
            if let Err(error) = self.check().and_then(|()| {
                if child.child.try_wait()?.is_some() {
                    bail!("restart stage=commit child exited before acceptance");
                }
                Ok(())
            }) {
                if self.decision.cancel() || self.decision.state() == CANCELED {
                    return Err(error);
                }
                continue;
            }
            thread::sleep(TICK);
        }
        child.keep = true;
        Ok(())
    }

    fn check(&self) -> Result<()> {
        if self.decision.state() == CANCELED || !self.target.is_live() {
            bail!("restart stage=cancel 用户退出或窗口已关闭");
        }
        if Instant::now() >= self.deadline {
            bail!("restart stage=timeout 已达到配置交接时间上限");
        }
        if self.latest.load(Ordering::Acquire) != self.candidate.generation {
            bail!("restart stage=superseded 已保存更新的配置");
        }
        Ok(())
    }

    fn require_current(&self) -> Result<()> {
        self.check()?;
        if !self.candidate.matches_disk(Path::new(&self.path))? {
            bail!("restart stage=superseded 磁盘配置已变化");
        }
        Ok(())
    }

    fn wait<T>(&self, receiver: &Receiver<T>, child: &mut OwnedChild, phase: &str) -> Result<T> {
        loop {
            self.check()
                .with_context(|| format!("restart phase={phase}"))?;
            match receiver.recv_timeout(TICK) {
                Ok(value) => return Ok(value),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("restart phase={phase} 通道已关闭")
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(exit) = child.child.try_wait()? {
                bail!("restart phase={phase} child_exit={exit}");
            }
        }
    }

    fn expect(
        &self,
        receiver: &Receiver<Result<Signal>>,
        child: &mut OwnedChild,
        expected: Signal,
    ) -> Result<()> {
        let actual = self.wait(receiver, child, &format!("{expected:?}"))??;
        if actual != expected {
            bail!("restart stage=protocol expected={expected:?} received={actual:?}");
        }
        Ok(())
    }

    fn command(&self, child: &mut OwnedChild, expected: ParentCommand) -> Result<()> {
        let actual = self.wait(&self.commands, child, &format!("{expected:?}"))?;
        if actual != expected {
            bail!("restart stage=ui-protocol unexpected command");
        }
        Ok(())
    }

    fn event(&self, event: ParentEvent) -> Result<()> {
        self.events
            .try_send(event)
            .context("restart stage=ui-notify")?;
        // The existing UI timer drains this bounded channel if PostMessage fails.
        self.target.try_post(WM_RESTART);
        Ok(())
    }
}

struct OwnedChild {
    child: Child,
    keep: bool,
}

impl OwnedChild {
    fn abort(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        self.child.kill().context("无法停止本次候选进程")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                bail!("候选进程停止确认超时");
            }
            thread::sleep(TICK);
        }
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.keep {
            let _ = self.abort();
        }
    }
}

#[cfg(test)]
mod tests;
