//! Bounded, cancelable subprocess IO. Called only from background workers.
use std::{
    io::{Read, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::{
    Foundation::HANDLE,
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

const OUTPUT_LIMIT: u64 = 2 * 1024 * 1024;
const CHECK_INTERVAL: Duration = Duration::from_millis(20);

#[cfg(test)]
mod tests;

pub(crate) struct CommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
}

pub(crate) fn run(
    command: &mut Command,
    input: Vec<u8>,
    timeout: Duration,
    canceled: &AtomicBool,
) -> Result<CommandOutput> {
    if canceled.load(Ordering::Acquire) {
        bail!("startup stage=command canceled");
    }
    let mut child = command
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动自启动辅助进程")?;
    let operation = (|| -> Result<CommandOutput> {
        // The native helper waits for its request on stdin. Assign it before
        // sending that request so parent exit cannot leave a mutating helper.
        let job = super::HandleWrapper::new(unsafe { CreateJobObjectW(None, None) }?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.get_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )?;
            AssignProcessToJobObject(job.get_handle(), HANDLE(child.as_raw_handle()))?;
        }
        let deadline = Instant::now() + timeout;
        let mut stdin = child.stdin.take().context("缺少命令输入管道")?;
        let stdout = child.stdout.take().context("缺少命令输出管道")?;
        let (write_tx, write_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("startup-command-write".into())
            .spawn(move || {
                let _ = write_tx.send(stdin.write_all(&input));
            })?;
        let overflow = Arc::new(AtomicBool::new(false));
        let worker_overflow = overflow.clone();
        let (read_tx, read_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("startup-command-read".into())
            .spawn(move || {
                let mut bytes = Vec::new();
                let result = stdout.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes);
                if bytes.len() as u64 > OUTPUT_LIMIT {
                    worker_overflow.store(true, Ordering::Release);
                }
                let _ = read_tx.send(result.map(|_| bytes));
            })?;
        let status = loop {
            if canceled.load(Ordering::Acquire) {
                bail!("自启动操作已取消");
            }
            if Instant::now() >= deadline {
                bail!("自启动命令超时；未确认系统状态，请检查任务服务或权限后重试");
            }
            if overflow.load(Ordering::Acquire) {
                bail!("自启动命令输出超过 2 MiB 限制");
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            thread::sleep(CHECK_INTERVAL);
        };
        let remaining = || deadline.saturating_duration_since(Instant::now());
        write_rx
            .recv_timeout(remaining())
            .context("自启动命令输入未完成")??;
        let stdout = read_rx
            .recv_timeout(remaining())
            .context("自启动命令输出未关闭")??;
        if stdout.len() as u64 > OUTPUT_LIMIT {
            bail!("自启动命令输出过大");
        }
        Ok(CommandOutput { status, stdout })
    })();
    if operation.is_err() && child.try_wait()?.is_none() {
        // This handle belongs to this invocation. No process-name kill, system
        // service termination, or unrelated descendant cleanup is performed.
        child.kill().context("无法停止本次超时的辅助进程")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                bail!("辅助进程停止确认超时");
            }
            thread::sleep(CHECK_INTERVAL);
        }
    }
    operation
}
