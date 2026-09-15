use std::mem::{align_of, size_of};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE},
    Security::{
        GetTokenInformation, IsValidSid, TokenIntegrityLevel, TokenUser, PSID,
        TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_USER,
    },
};

const MAX_TOKEN_BYTES: usize = 64 * 1024;

/// Owns the aligned variable-length result and its embedded SID together.
pub(crate) struct TokenSid {
    _storage: Vec<usize>,
    sid: PSID,
    sub_authorities: u8,
}

impl TokenSid {
    pub(crate) fn user(token: HANDLE) -> Result<Self> {
        Self::query(token, TokenUser, size_of::<TOKEN_USER>())
    }

    pub(crate) fn integrity(token: HANDLE) -> Result<Self> {
        Self::query(
            token,
            TokenIntegrityLevel,
            size_of::<TOKEN_MANDATORY_LABEL>(),
        )
    }

    fn query(token: HANDLE, class: TOKEN_INFORMATION_CLASS, minimum: usize) -> Result<Self> {
        let mut length = 0;
        match unsafe { GetTokenInformation(token, class, None, 0, &mut length) } {
            Err(err) if err.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
            Err(err) => return Err(err).context("token stage=size"),
            Ok(()) => bail!("token stage=size unexpected success"),
        }
        if !(minimum..=MAX_TOKEN_BYTES).contains(&(length as usize)) {
            bail!("token stage=size invalid length");
        }
        let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let capacity = (storage.len() * size_of::<usize>()) as u32;
        unsafe {
            GetTokenInformation(
                token,
                class,
                Some(storage.as_mut_ptr().cast()),
                capacity,
                &mut length,
            )
        }
        .context("token stage=query")?;
        if length < minimum as u32 || length > capacity {
            bail!("token stage=query invalid returned length");
        }
        let sid = unsafe {
            if class == TokenUser {
                (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid
            } else {
                (*storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())
                    .Label
                    .Sid
            }
        };
        let sub_authorities = validate_sid_range(&storage, length as usize, sid)?;
        if !unsafe { IsValidSid(sid) }.as_bool() {
            bail!("token stage=sid invalid SID");
        }
        Ok(Self {
            _storage: storage,
            sid,
            sub_authorities,
        })
    }

    pub(crate) fn sid(&self) -> PSID {
        self.sid
    }

    pub(crate) fn integrity_rid(&self) -> Result<u32> {
        if self.sub_authorities == 0 {
            bail!("token stage=sid missing integrity RID");
        }
        // Header + every subauthority was checked against the owned allocation.
        Ok(unsafe {
            self.sid
                .0
                .cast::<u8>()
                .add(8)
                .cast::<u32>()
                .add(self.sub_authorities as usize - 1)
                .read()
        })
    }
}

fn validate_sid_range(storage: &[usize], length: usize, sid: PSID) -> Result<u8> {
    let start = storage.as_ptr() as usize;
    let address = sid.0 as usize;
    if length > std::mem::size_of_val(storage) || !address.is_multiple_of(align_of::<u32>()) {
        bail!("token stage=sid invalid allocation or alignment");
    }
    let offset = address
        .checked_sub(start)
        .context("token stage=sid pointer before buffer")?;
    if offset.checked_add(8).is_none_or(|end| end > length) {
        bail!("token stage=sid header outside buffer");
    }
    let count = unsafe { sid.0.cast::<u8>().add(1).read() };
    if offset + 8 + count as usize * size_of::<u32>() > length {
        bail!("token stage=sid body outside buffer");
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::HandleWrapper;
    use windows::Win32::{
        Security::TOKEN_QUERY,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    #[test]
    fn token_results_are_aligned_and_own_complete_sids() {
        let mut token = HandleWrapper::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, token.get_handle_mut()) }
            .unwrap();
        let user = TokenSid::user(token.get_handle()).unwrap();
        assert!(unsafe { IsValidSid(user.sid()) }.as_bool());
        assert!(TokenSid::integrity(token.get_handle())
            .unwrap()
            .integrity_rid()
            .is_ok());
    }

    #[test]
    fn null_unaligned_outside_and_truncated_sid_ranges_are_rejected() {
        let storage = vec![0usize; 4];
        let base = storage.as_ptr() as usize;
        for address in [0, base + 1, base - 4, base + 32, usize::MAX] {
            assert!(validate_sid_range(&storage, 32, PSID(address as _)).is_err());
        }
        assert!(validate_sid_range(&storage, 4, PSID(base as _)).is_err());
    }
}
