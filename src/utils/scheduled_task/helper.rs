use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use windows::{
    core::{BSTR, PWSTR},
    Win32::{
        Foundation::{LocalFree, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HLOCAL},
        Security::{Authorization::ConvertSidToStringSidW, TOKEN_QUERY},
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            TaskScheduler::{
                ITaskFolder, ITaskService, TaskScheduler, TASK_CREATE, TASK_CREATE_OR_UPDATE,
                TASK_LOGON_INTERACTIVE_TOKEN,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
            Variant::VARIANT,
        },
    },
};

use super::{xml::TaskDefinition, TASK_NAME};
use crate::utils::{com::ComApartment, is_running_as_admin, token::TokenSid, HandleWrapper};

const ABSENT: u32 = u32::MAX;

pub(super) fn write_text(writer: &mut impl Write, text: Option<&str>) -> Result<()> {
    let size = text.map_or(ABSENT, |text| text.len() as u32);
    writer.write_all(&size.to_le_bytes())?;
    if let Some(text) = text {
        writer.write_all(text.as_bytes())?;
    }
    Ok(())
}

pub(super) fn read_text(reader: &mut impl Read) -> Result<Option<String>> {
    let mut size = [0; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size);
    if size == ABSENT {
        return Ok(None);
    }
    if size > 1024 * 1024 {
        bail!("计划任务协议内容过大");
    }
    let mut bytes = vec![0; size as usize];
    reader.read_exact(&mut bytes)?;
    Ok(Some(
        String::from_utf8(bytes).context("计划任务协议不是 UTF-8")?,
    ))
}

/// Private helper entry. It does not acquire the app mutex, install hooks,
/// initialize the tray, or read/write user INI files.
pub(super) fn execute() -> Result<()> {
    let mut input = std::io::stdin();
    let mut header = [0; 5];
    input.read_exact(&mut header)?;
    if &header[..4] != b"WST1" || header[4] > 1 {
        bail!("自启动辅助协议无效");
    }
    let request = if header[4] == 1 {
        Some((read_text(&mut input)?, read_text(&mut input)?))
    } else {
        None
    };
    let _com = ComApartment::sta()?;
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }?;
    let empty = VARIANT::default();
    unsafe { service.Connect(&empty, &empty, &empty, &empty) }?;
    let folder = unsafe { service.GetFolder(&BSTR::from("\\")) }?;
    if let Some((expected, replacement)) = request {
        if query(&folder)? != expected {
            bail!("计划任务已被外部修改；本次操作取消");
        }
        let executable = std::env::current_exe()?.to_string_lossy().into_owned();
        let user = current_user_sid()?;
        for xml in [expected.as_ref(), replacement.as_ref()]
            .into_iter()
            .flatten()
        {
            TaskDefinition::parse(xml)?.require_owner(&executable, &user)?;
        }
        match replacement {
            Some(xml) => {
                if TaskDefinition::parse(&xml)?.policy.highest && !is_running_as_admin()? {
                    bail!("最高权限自启动需要以管理员身份运行应用；不会自动提权");
                }
                unsafe {
                    folder.RegisterTask(
                        &BSTR::from(TASK_NAME),
                        &BSTR::from(xml),
                        if expected.is_none() {
                            TASK_CREATE.0
                        } else {
                            TASK_CREATE_OR_UPDATE.0
                        },
                        &VARIANT::from(user.as_str()),
                        &empty,
                        TASK_LOGON_INTERACTIVE_TOKEN,
                        &empty,
                    )
                }?;
            }
            None if expected.is_some() => unsafe { folder.DeleteTask(&BSTR::from(TASK_NAME), 0) }?,
            None => {}
        }
    }
    let result = query(&folder)?;
    write_text(&mut std::io::stdout(), result.as_deref())?;
    Ok(())
}

fn query(folder: &ITaskFolder) -> Result<Option<String>> {
    match unsafe { folder.GetTask(&BSTR::from(TASK_NAME)) } {
        Ok(task) => Ok(Some(unsafe { task.Xml() }?.to_string())),
        Err(error)
            if [
                ERROR_FILE_NOT_FOUND.to_hresult(),
                ERROR_PATH_NOT_FOUND.to_hresult(),
            ]
            .contains(&error.code()) =>
        {
            Ok(None)
        }
        Err(error) => Err(error).context("计划任务状态未知；请检查任务服务和当前用户权限"),
    }
}

pub(crate) fn current_user_sid() -> Result<String> {
    let mut token = HandleWrapper::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, token.get_handle_mut()) }?;
    let sid = TokenSid::user(token.get_handle())?;
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid.sid(), &mut text) }?;
    struct SidString(PWSTR);
    impl Drop for SidString {
        fn drop(&mut self) {
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0 .0.cast()))) };
        }
    }
    let owned = SidString(text);
    String::from_utf16(unsafe { owned.0.as_wide() }).context("用户 SID 无效")
}
