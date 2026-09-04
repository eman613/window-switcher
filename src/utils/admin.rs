use super::HandleWrapper;

use anyhow::{anyhow, bail, Result};
use std::path::Path;
use windows::core::{w, PCWSTR};
use windows::Win32::{
    Foundation::HANDLE,
    Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenElevation,
        TokenElevationType, TokenElevationTypeFull, TokenIntegrityLevel, TOKEN_ELEVATION,
        TOKEN_ELEVATION_TYPE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    },
    System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    },
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};

const SECURITY_MANDATORY_HIGH_RID: u32 = 0x00003000;
const SECURITY_MANDATORY_SYSTEM_RID: u32 = 0x00004000;

pub fn is_running_as_admin() -> Result<bool> {
    let process = unsafe { GetCurrentProcess() };
    is_elevated(process)
        .map_err(|err| anyhow!("Failed to verify if the program is running as admin, {err}"))
}

pub fn is_process_elevated(pid: u32) -> Option<bool> {
    let process = HandleWrapper::new(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?,
    );
    get_process_elevation_info(process.get_handle()).ok()
}

pub fn relaunch_as_admin() -> Result<()> {
    let exe_path = std::env::current_exe()
        .map_err(|err| anyhow!("Failed to get executable path for elevation, {err}"))?;
    let exe_path_string = exe_path.to_string_lossy();
    let exe_path_wide = super::to_wstring(&exe_path_string);
    let directory = exe_path.parent().unwrap_or_else(|| Path::new("."));
    let directory_string = directory.to_string_lossy();
    let directory_wide = super::to_wstring(&directory_string);

    let result = unsafe {
        ShellExecuteW(
            None,
            w!("runas"),
            PCWSTR(exe_path_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR(directory_wide.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code <= 32 {
        bail!("Failed to relaunch as administrator, ShellExecuteW code {code}");
    }
    Ok(())
}

fn get_process_elevation_info(process: HANDLE) -> Result<bool> {
    unsafe {
        let mut token = HandleWrapper::default();
        OpenProcessToken(process, TOKEN_QUERY, token.get_handle_mut())?;

        query_token_elevated(token.get_handle())
    }
}

unsafe fn query_token_elevated(token: HANDLE) -> Result<bool> {
    let mut ret_len = 0u32;

    let mut elevation = TOKEN_ELEVATION::default();
    GetTokenInformation(
        token,
        TokenElevation,
        Some(&mut elevation as *mut _ as *mut _),
        std::mem::size_of::<TOKEN_ELEVATION>() as u32,
        &mut ret_len,
    )?;

    let mut elevation_type = TOKEN_ELEVATION_TYPE(0);
    GetTokenInformation(
        token,
        TokenElevationType,
        Some(&mut elevation_type as *mut _ as *mut _),
        std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
        &mut ret_len,
    )?;

    let mut buf = [0u8; 512];
    GetTokenInformation(
        token,
        TokenIntegrityLevel,
        Some(buf.as_mut_ptr() as *mut _),
        buf.len() as u32,
        &mut ret_len,
    )?;

    let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
    let sid = label.Label.Sid;
    if sid.0.is_null() {
        return Err(anyhow!("SID is null"));
    }
    let sub_auth_count = *GetSidSubAuthorityCount(sid);
    let rid = *GetSidSubAuthority(sid, (sub_auth_count - 1).into());

    Ok(matches!(
        rid,
        SECURITY_MANDATORY_HIGH_RID | SECURITY_MANDATORY_SYSTEM_RID
    ) && elevation.TokenIsElevated != 0
        && elevation_type == TokenElevationTypeFull)
}

pub fn is_elevated(handle: HANDLE) -> Result<bool> {
    get_process_elevation_info(handle)
}
