use anyhow::{anyhow, bail, Result};
use std::mem::size_of;
use windows::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Security::{GetTokenInformation, TOKEN_INFORMATION_CLASS},
};

const MAX_TOKEN_INFORMATION_SIZE: usize = 1024 * 1024;

pub(crate) fn query_token_information(
    token: windows::Win32::Foundation::HANDLE,
    information_class: TOKEN_INFORMATION_CLASS,
) -> Result<(Vec<usize>, usize)> {
    let mut required_length = 0u32;
    let initial_result =
        unsafe { GetTokenInformation(token, information_class, None, 0, &mut required_length) };
    if let Err(err) = initial_result {
        if !is_insufficient_buffer(&err) {
            return Err(err.into());
        }
    }

    let mut byte_length = usize::try_from(required_length)?;
    if byte_length == 0 || byte_length > MAX_TOKEN_INFORMATION_SIZE {
        bail!("Invalid token information size {byte_length}");
    }

    for _ in 0..3 {
        let word_size = size_of::<usize>();
        let word_count = byte_length
            .checked_add(word_size - 1)
            .ok_or_else(|| anyhow!("Token information size overflow"))?
            / word_size;
        let mut buffer = vec![0usize; word_count];
        let buffer_length = buffer
            .len()
            .checked_mul(word_size)
            .ok_or_else(|| anyhow!("Token information buffer size overflow"))?;
        let mut return_length = 0u32;
        let result = unsafe {
            GetTokenInformation(
                token,
                information_class,
                Some(buffer.as_mut_ptr().cast()),
                u32::try_from(buffer_length)?,
                &mut return_length,
            )
        };
        match result {
            Ok(()) => {
                let returned_length = usize::try_from(return_length)?;
                if returned_length > buffer_length {
                    byte_length = returned_length;
                    continue;
                }
                return Ok((buffer, returned_length));
            }
            Err(err) if is_insufficient_buffer(&err) => {
                let reported_length = usize::try_from(return_length)?;
                let expanded_length = buffer_length.saturating_mul(2);
                byte_length = reported_length.max(expanded_length);
                if byte_length <= buffer_length || byte_length > MAX_TOKEN_INFORMATION_SIZE {
                    bail!("Token information exceeds the safety limit");
                }
            }
            Err(err) => return Err(err.into()),
        }
    }
    bail!("Token information size changed while querying")
}

fn is_insufficient_buffer(error: &windows::core::Error) -> bool {
    error.code() == windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
}
