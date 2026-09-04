use super::{query_token_information, HandleWrapper};

use anyhow::{anyhow, bail, Result};
use std::{
    mem::{size_of, MaybeUninit},
    path::Path,
};
use windows::core::{w, PCWSTR};
use windows::Win32::{
    Foundation::HANDLE,
    Security::{
        GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, IsValidSid,
        TokenElevation, TokenElevationType, TokenElevationTypeFull, TokenIntegrityLevel,
        TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY,
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

fn query_token_elevated(token: HANDLE) -> Result<bool> {
    let elevation: TOKEN_ELEVATION = query_fixed_token_information(token, TokenElevation)?;
    let elevation_type: TOKEN_ELEVATION_TYPE =
        query_fixed_token_information(token, TokenElevationType)?;
    let (buffer, buffer_len) = query_token_information(token, TokenIntegrityLevel)?;

    if buffer_len < size_of::<TOKEN_MANDATORY_LABEL>() {
        bail!("Token integrity information is shorter than TOKEN_MANDATORY_LABEL");
    }

    let buffer_start = buffer.as_ptr() as usize;
    let buffer_end = buffer_start
        .checked_add(buffer_len)
        .ok_or_else(|| anyhow!("Token integrity information address overflow"))?;
    let label = unsafe { &*(buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()) };
    let sid = label.Label.Sid;
    let sid_address = sid.0 as usize;
    if sid.0.is_null() || sid_address < buffer_start || sid_address >= buffer_end {
        bail!("Token integrity SID is null or outside the returned buffer");
    }
    if unsafe { !IsValidSid(sid).as_bool() } {
        bail!("Token integrity SID is invalid");
    }
    let sid_length = unsafe { GetLengthSid(sid) } as usize;
    if sid_length == 0
        || sid_address
            .checked_add(sid_length)
            .is_none_or(|end| end > buffer_end)
    {
        bail!("Token integrity SID exceeds the returned buffer");
    }

    let sub_auth_count_ptr = unsafe { GetSidSubAuthorityCount(sid) };
    if sub_auth_count_ptr.is_null()
        || (sub_auth_count_ptr as usize) < buffer_start
        || (sub_auth_count_ptr as usize) >= buffer_end
    {
        bail!("Token integrity SID sub-authority count is invalid");
    }
    let sub_auth_count = unsafe { *sub_auth_count_ptr };
    if sub_auth_count == 0 {
        bail!("Token integrity SID has no sub-authorities");
    }

    let rid_ptr = unsafe { GetSidSubAuthority(sid, u32::from(sub_auth_count - 1)) };
    if rid_ptr.is_null()
        || (rid_ptr as usize) < buffer_start
        || (rid_ptr as usize)
            .checked_add(size_of::<u32>())
            .is_none_or(|end| end > buffer_end)
    {
        bail!("Token integrity SID RID is invalid");
    }
    let rid = unsafe { *rid_ptr };

    Ok(matches!(
        rid,
        SECURITY_MANDATORY_HIGH_RID | SECURITY_MANDATORY_SYSTEM_RID
    ) && elevation.TokenIsElevated != 0
        && elevation_type == TokenElevationTypeFull)
}

fn query_fixed_token_information<T: Copy>(
    token: HANDLE,
    information_class: TOKEN_INFORMATION_CLASS,
) -> Result<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut return_length = 0u32;
    let length = u32::try_from(size_of::<T>())?;
    unsafe {
        GetTokenInformation(
            token,
            information_class,
            Some(value.as_mut_ptr().cast()),
            length,
            &mut return_length,
        )?;
    }
    if return_length < length {
        bail!("Token information returned {return_length} bytes, expected {length}");
    }
    Ok(unsafe { value.assume_init() })
}

pub fn is_elevated(handle: HANDLE) -> Result<bool> {
    get_process_elevation_info(handle)
}
