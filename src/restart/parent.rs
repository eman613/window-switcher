use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    elevation::ElevatedProcess,
    handshake::{self, Step},
    protocol::{self, Signal},
    transport::{OwnedChild, Streams},
    Decision, ParentCommand, ParentEvent, RestartController, ACCEPTED, CANCELED, CHILD_ARGUMENT,
    TICK, WM_RESTART,
};
use crate::{
    config::reload::{self, ConfigCandidate},
    window_target::WindowTarget,
};
use anyhow::{bail, Context, Result};

pub(super) fn start(
    candidate: ConfigCandidate,
    path: PathBuf,
    timeout_ms: u32,
    latest: Arc<AtomicU64>,
    target: Arc<WindowTarget>,
    elevate: bool,
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
        .spawn(move || worker.run(timeout_ms, elevate))
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
    fn run(mut self, timeout_ms: u32, elevate: bool) {
        if elevate {
            let mut child = None;
            let result = (|| -> Result<()> {
                self.prepare()?;
                let process = ElevatedProcess::launch(timeout_ms, self.target.window_id())?;
                // User time on the secure desktop is not a handoff timeout.
                self.deadline = Instant::now() + Duration::from_millis(timeout_ms.into());
                debug!("restart stage=elevated-launched child_pid={}", process.id());
                child = Some(OwnedChild::elevated(process));
                let streams = child.as_mut().unwrap().connect(|| self.check())?;
                self.handoff(&mut child, streams)
            })();
            self.finish(result, child);
            return;
        }
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
        let result = (|| -> Result<()> {
            self.prepare()?;
            let (owned, streams) = OwnedChild::spawn(command?)?;
            child = Some(owned);
            self.handoff(&mut child, streams)
        })();
        self.finish(result, child);
    }

    fn finish(self, result: Result<()>, mut child: Option<OwnedChild>) {
        if let Err(error) = result {
            warn!("restart stage=failed error={error:#}");
            self.decision.cancel();
            let cleanup = child.as_mut().map_or(Ok(()), OwnedChild::abort);
            let safe_to_resume = cleanup.is_ok();
            let message = match cleanup {
                Ok(()) => format!("配置重启未完成：{error:#}；旧实例保留，可自动重试"),
                Err(cleanup) => {
                    warn!("restart stage=cleanup-failed error={cleanup:#}");
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
            if !safe_to_resume {
                if let Some(child) = child.as_mut() {
                    self.await_cleanup(child);
                }
            }
        }
    }

    fn await_cleanup(&self, child: &mut OwnedChild) {
        // A standard parent cannot terminate an elevated child. Its bounded
        // abort wait may end before the child finishes startup or resumes its
        // UI thread. Keep ownership until exit is confirmed, then let the old
        // UI restore input (or finish a pending user exit).
        while self.target.is_live() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    info!("restart stage=cleanup-confirmed child_pid={}", child.id());
                    let _ = self.event(ParentEvent::Failed {
                        message: "候选进程已退出；旧实例正在恢复，可重新尝试".into(),
                        safe_to_resume: true,
                    });
                    return;
                }
                Ok(None) => thread::sleep(TICK),
                Err(error) => {
                    error!("restart stage=cleanup-wait error={error:#}");
                    return;
                }
            }
        }
    }

    fn prepare(&self) -> Result<()> {
        self.check()?;
        reload::preflight(&self.candidate, &self.path).context("restart stage=preflight")
    }

    fn handoff(&self, owned: &mut Option<OwnedChild>, streams: Streams) -> Result<()> {
        self.check()?;
        let child = owned.as_mut().unwrap();
        info!(
            "restart stage=spawn generation={} child_pid={}",
            self.candidate.generation,
            child.id()
        );
        let output = protocol::reader(streams.reader)?;
        let mut input = streams.writer;
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
                    child.id()
                );
                self.event(ParentEvent::Ready)
            }
            Step::Done => self.event(ParentEvent::Done),
        })?;
        // A user exit before UI acceptance wins; the child still treats EOF as
        // failure. Old UI teardown is delayed until final ownership is acknowledged.
        loop {
            match self.decision.state() {
                ACCEPTED => break,
                CANCELED => bail!("restart stage=commit canceled before acceptance"),
                _ => {}
            }
            // Acceptance and timeout/exit compete for one terminal decision.
            // Closing the old UI after acceptance must never kill the new owner.
            if let Err(error) = self.check().and_then(|()| {
                if child.try_wait()?.is_some() {
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
        Signal::Accepted.write(&mut input)?;
        self.expect(&output, child, Signal::AcceptedAck)?;
        child.keep = true;
        self.event(ParentEvent::Complete)?;
        Ok(())
    }

    fn check(&self) -> Result<()> {
        if self.decision.state() == CANCELED
            || (self.decision.state() != ACCEPTED && !self.target.is_live())
        {
            bail!("restart stage=cancel 用户退出或窗口已关闭");
        }
        if Instant::now() >= self.deadline {
            bail!("restart stage=timeout 已达到配置交接时间上限");
        }
        if self.decision.state() != ACCEPTED
            && self.latest.load(Ordering::Acquire) != self.candidate.generation
        {
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
            if let Some(exit) = child.try_wait()? {
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

#[cfg(test)]
mod tests;
