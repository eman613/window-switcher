//! Preserve the original owner on an empty candidate across token elevation.
use std::fs::File;

use anyhow::{ensure, Context, Result};
use windows::{
    core::BOOL,
    Win32::Security::{
        Authorization::{SetSecurityInfo, SE_FILE_OBJECT},
        EqualSid, GetSecurityDescriptorOwner, IsValidSid, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID,
    },
};

use super::file_identity::handle;
use crate::localization::FailureReason;

pub(super) fn preserve_owner(
    candidate: &File,
    original_security: &[u32],
    candidate_security: &[u32],
) -> Result<()> {
    let original = owner(original_security)?;
    let created = owner(candidate_security)?;
    if unsafe { EqualSid(original, created) }.is_ok() {
        return Ok(());
    }
    // This is the newly created, still-empty candidate. The original file and
    // both DACLs remain untouched. Windows enforces whether this SID is assignable;
    // no privilege is enabled and the caller rechecks the complete descriptor.
    unsafe {
        SetSecurityInfo(
            handle(candidate),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(original),
            None,
            None,
            None,
        )
    }
    .ok()
    .map_err(|error| {
        warn!(
            "config stage=metadata-owner hresult={:#010x}",
            error.code().0
        );
        let code = error.code().0 as u32;
        anyhow::Error::new(error)
            .context(crate::localization::SystemFailure(code))
            .context(FailureReason::ConfigOwner)
    })?;
    debug!("config stage=metadata candidate-owner-preserved");
    Ok(())
}

fn owner(security: &[u32]) -> Result<PSID> {
    let mut sid = PSID::default();
    let mut defaulted = BOOL::default();
    unsafe {
        GetSecurityDescriptorOwner(
            PSECURITY_DESCRIPTOR(security.as_ptr().cast_mut().cast()),
            &mut sid,
            &mut defaulted,
        )
    }
    .context("无法读取 INI 文件所有者")?;
    ensure!(
        !sid.0.is_null() && unsafe { IsValidSid(sid) }.as_bool(),
        "INI 文件所有者无效；原文件未修改"
    );
    Ok(sid)
}
