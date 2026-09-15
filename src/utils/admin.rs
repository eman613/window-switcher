use super::{token::TokenSid, HandleWrapper};

use anyhow::{anyhow, bail, Result};
use windows::Win32::{
    Foundation::HANDLE,
    Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
    System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

const SECURITY_MANDATORY_HIGH_RID: u32 = 0x00003000;

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

    if ret_len as usize != std::mem::size_of::<TOKEN_ELEVATION>() {
        bail!("token stage=elevation invalid length");
    }
    let rid = TokenSid::integrity(token)?.integrity_rid()?;
    Ok(classify_elevation(elevation.TokenIsElevated != 0, rid))
}

fn classify_elevation(elevated: bool, integrity_rid: u32) -> bool {
    // TokenElevationTypeDefault means no linked token, not "not elevated".
    elevated && integrity_rid >= SECURITY_MANDATORY_HIGH_RID
}

pub fn is_elevated(handle: HANDLE) -> Result<bool> {
    get_process_elevation_info(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn high_system_and_default_tokens_use_actual_elevation() {
        assert!(classify_elevation(true, 0x3000));
        assert!(classify_elevation(true, 0x4000));
        assert!(!classify_elevation(false, 0x3000));
        assert!(!classify_elevation(true, 0x2000));
        assert!(is_running_as_admin().is_ok());
        assert!(is_process_elevated(std::process::id()).is_some());
    }
}
