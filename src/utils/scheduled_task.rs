use std::{process::Command, sync::atomic::AtomicBool, time::Duration};

use anyhow::{bail, Result};

use super::command;

mod helper;
pub(crate) mod xml;
pub(crate) use helper::current_user_sid;

pub(crate) const TASK_NAME: &str = "WindowSwitcher";
const HELPER_ARGUMENT: &str = "--startup-helper";

pub fn run_startup_helper() -> Option<i32> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments
        .first()
        .is_none_or(|value| value != HELPER_ARGUMENT)
    {
        return None;
    }
    if arguments.len() != 1 {
        return Some(2);
    }
    match helper::execute() {
        Ok(()) => Some(0),
        Err(error) => {
            // Only a bounded error string, never XML, usernames, or commands.
            let code = error
                .downcast_ref::<windows::core::Error>()
                .map_or(0, |error| error.code().0);
            let message = format!("自启动系统操作失败（系统代码 0x{:08X}）；请检查权限、任务服务及启动项是否被外部修改", code as u32);
            let _ = helper::write_text(&mut std::io::stdout(), Some(&message));
            Some(1)
        }
    }
}

pub(crate) fn query_task(timeout: Duration, canceled: &AtomicBool) -> Result<Option<String>> {
    invoke(None, timeout, canceled)
}

pub(crate) fn compare_task(
    expected: Option<&str>,
    replacement: Option<&str>,
    timeout: Duration,
    canceled: &AtomicBool,
) -> Result<Option<String>> {
    invoke(Some((expected, replacement)), timeout, canceled)
}

fn invoke(
    change: Option<(Option<&str>, Option<&str>)>,
    timeout: Duration,
    canceled: &AtomicBool,
) -> Result<Option<String>> {
    let mut input = b"WST1".to_vec();
    input.push(u8::from(change.is_some()));
    if let Some((expected, replacement)) = change {
        helper::write_text(&mut input, expected)?;
        helper::write_text(&mut input, replacement)?;
    }
    let output = command::run(
        Command::new(std::env::current_exe()?).arg(HELPER_ARGUMENT),
        input,
        timeout,
        canceled,
    )?;
    let result = helper::read_text(&mut output.stdout.as_slice())?;
    if !output.status.success() {
        bail!(
            "{}",
            result.unwrap_or_else(|| "自启动辅助进程失败，系统状态未确认".into())
        );
    }
    Ok(result)
}
