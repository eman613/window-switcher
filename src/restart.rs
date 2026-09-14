use std::{
    os::windows::process::CommandExt,
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{ERROR_INVALID_PARAMETER, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_SYNCHRONIZE},
};

use crate::utils::HandleWrapper;

const PARENT_ARGUMENT: &str = "--restart-parent";
const PARENT_EXIT_TIMEOUT_MS: u32 = 15_000;

/// Called before opening the INI or acquiring the single-instance mutex.
pub fn wait_for_restart_parent() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(position) = arguments
        .iter()
        .position(|argument| argument == PARENT_ARGUMENT)
    else {
        return Ok(());
    };
    let parent = arguments
        .get(position + 1)
        .and_then(|argument| argument.to_str())
        .context("自动重启缺少父进程编号")?
        .parse::<u32>()
        .context("自动重启父进程编号无效")?;
    if parent == 0 || parent == std::process::id() {
        bail!("自动重启父进程编号无效");
    }
    let handle = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent) } {
        Ok(handle) => HandleWrapper::new(handle),
        // The original process may have exited before this process was scheduled.
        Err(err) if err.code() == ERROR_INVALID_PARAMETER.to_hresult() => return Ok(()),
        Err(err) => return Err(err).context("无法等待旧进程退出；请手动重新启动应用"),
    };
    match unsafe { WaitForSingleObject(handle.get_handle(), PARENT_EXIT_TIMEOUT_MS) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => bail!("旧进程在 15 秒内未退出；已取消新实例，请检查旧进程后重新启动"),
        WAIT_FAILED => Err(windows::core::Error::from_win32()).context("等待旧进程退出失败"),
        status => bail!("等待旧进程返回异常状态 {}", status.0),
    }
}

pub(crate) fn spawn_replacement() -> Result<u32> {
    let executable = std::env::current_exe().context("无法获取当前程序路径")?;
    let mut command = Command::new(&executable);
    command
        .arg(PARENT_ARGUMENT)
        .arg(std::process::id().to_string())
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(directory) = executable.parent() {
        command.current_dir(directory);
    }
    let child = command
        .spawn()
        .with_context(|| format!("无法重启 '{}'；当前进程继续运行", executable.display()))?;
    let pid = child.id();
    info!(
        "config stage=restart parent_pid={} child_pid={pid}",
        std::process::id()
    );
    Ok(pid)
}
