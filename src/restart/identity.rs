//! Bind the private restart channel to a process instance in this logon session.
use std::{mem::size_of, path::Path};

use anyhow::{ensure, Context, Result};
use windows::Win32::{
    Foundation::HANDLE,
    Security::{EqualSid, GetTokenInformation, TokenSessionId, TOKEN_QUERY},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::{
    process_metadata::{self, ProcessIdentity},
    utils::{token::TokenSid, HandleWrapper},
};

pub(super) fn current() -> Result<ProcessIdentity> {
    process_metadata::open_identity(std::process::id())
        .map(|(_, identity)| identity)
        .context("restart stage=identity cannot query current process")
}

pub(super) fn verify(expected: ProcessIdentity) -> Result<()> {
    let (process, actual) = process_metadata::open_identity(expected.pid)
        .context("restart stage=identity peer unavailable")?;
    ensure!(actual == expected, "restart stage=identity process reused");
    let executable = std::env::current_exe()?.canonicalize()?;
    let peer_path = process_metadata::image_path(&process)
        .context("restart stage=identity peer image unavailable")?;
    let peer_path = Path::new(&peer_path).canonicalize()?;
    ensure!(
        executable
            .to_str()
            .context("restart stage=identity invalid executable path")?
            .eq_ignore_ascii_case(
                peer_path
                    .to_str()
                    .context("restart stage=identity invalid peer path")?
            ),
        "restart stage=identity executable mismatch"
    );
    let own_token = token(unsafe { GetCurrentProcess() })?;
    let peer_token = token(process.get_handle())?;
    let own_user = TokenSid::user(own_token.get_handle())?;
    let peer_user = TokenSid::user(peer_token.get_handle())?;
    ensure!(
        unsafe { EqualSid(own_user.sid(), peer_user.sid()) }.is_ok(),
        "restart stage=identity different user"
    );
    ensure!(
        session(own_token.get_handle())? == session(peer_token.get_handle())?,
        "restart stage=identity different session"
    );
    Ok(())
}

fn token(process: HANDLE) -> Result<HandleWrapper> {
    let mut token = HandleWrapper::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, token.get_handle_mut()) }?;
    Ok(token)
}

fn session(token: HANDLE) -> Result<u32> {
    let mut session = 0u32;
    let mut length = 0;
    unsafe {
        GetTokenInformation(
            token,
            TokenSessionId,
            Some((&mut session as *mut u32).cast()),
            size_of::<u32>() as u32,
            &mut length,
        )
    }?;
    ensure!(
        length == size_of::<u32>() as u32,
        "restart stage=identity invalid session"
    );
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_rejects_reused_process_identity() {
        let current = current().unwrap();
        verify(current).unwrap();
        assert!(verify(ProcessIdentity {
            created: current.created.wrapping_add(1),
            ..current
        })
        .is_err());
        assert!(verify(ProcessIdentity { pid: 0, created: 0 }).is_err());
    }
}
