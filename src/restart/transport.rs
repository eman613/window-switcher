use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use super::{elevation::ElevatedProcess, TICK};

pub(super) struct Streams {
    pub(super) reader: Box<dyn Read + Send>,
    pub(super) writer: Box<dyn Write + Send>,
}

enum Process {
    Current(Child),
    Elevated(ElevatedProcess),
}

pub(super) struct OwnedChild {
    process: Process,
    pub(super) keep: bool,
}

impl OwnedChild {
    pub(super) fn spawn(mut command: Command) -> Result<(Self, Streams)> {
        let child = command
            .creation_flags(CREATE_NO_WINDOW.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("restart stage=spawn")?;
        let mut owned = Self {
            process: Process::Current(child),
            keep: false,
        };
        let Process::Current(child) = &mut owned.process else {
            unreachable!()
        };
        let streams = Streams {
            reader: Box::new(child.stdout.take().context("缺少子进程输出管道")?),
            writer: Box::new(child.stdin.take().context("缺少子进程输入管道")?),
        };
        Ok((owned, streams))
    }

    pub(super) fn elevated(process: ElevatedProcess) -> Self {
        Self {
            process: Process::Elevated(process),
            keep: false,
        }
    }

    pub(super) fn connect(&mut self, check: impl Fn() -> Result<()>) -> Result<Streams> {
        match &mut self.process {
            Process::Elevated(child) => child.connect(check),
            Process::Current(_) => bail!("restart stage=connect ordinary pipes already attached"),
        }
    }

    pub(super) fn id(&self) -> u32 {
        match &self.process {
            Process::Current(child) => child.id(),
            Process::Elevated(child) => child.id(),
        }
    }

    pub(super) fn try_wait(&mut self) -> Result<Option<String>> {
        match &mut self.process {
            Process::Current(child) => Ok(child.try_wait()?.map(|status| status.to_string())),
            Process::Elevated(child) => child.try_wait(),
        }
    }

    pub(super) fn abort(&mut self) -> Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        match &mut self.process {
            Process::Current(child) => child.kill().context("无法停止本次候选进程")?,
            Process::Elevated(child) => child.disconnect(),
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.try_wait()?.is_none() {
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
