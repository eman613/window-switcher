use std::{env, os::windows::ffi::OsStrExt, path::PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use windows::{
    core::{HRESULT, PWSTR},
    Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, MAX_PATH},
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

use super::HandleWrapper;

const INITIAL_PROCESS_PATH_CAPACITY: usize = MAX_PATH as usize;
// Reserve room for the terminator at the extended Windows path limit.
const MAX_PROCESS_PATH_CAPACITY: usize = 32_768;

pub fn get_exe_folder() -> Result<PathBuf> {
    let path = current_executable_path()?;
    path.parent()
        .map(|parent| parent.to_path_buf())
        .ok_or_else(|| anyhow!("Failed to get binary folder"))
}

pub fn get_exe_path() -> Result<Vec<u16>> {
    Ok(current_executable_path()?
        .as_os_str()
        .encode_wide()
        .collect())
}

fn current_executable_path() -> Result<PathBuf> {
    env::current_exe().context("Failed to resolve current executable path")
}

pub fn get_module_path(pid: u32) -> Option<String> {
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => HandleWrapper::new(handle),
        Err(error) => {
            debug!("process path open failed pid={pid} error={error}");
            return None;
        }
    };
    let path = query_process_path(pid, |buffer| {
        // The bounded buffer always fits in the Win32 u32 character count.
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle.get_handle(),
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }?;
        Ok(length as usize)
    });
    match path {
        Ok(path) => Some(String::from_utf16_lossy(&path)),
        Err(error) => {
            debug!("process path query failed pid={pid} error={error:#}");
            None
        }
    }
}

fn query_process_path(
    pid: u32,
    mut query: impl FnMut(&mut [u16]) -> windows::core::Result<usize>,
) -> Result<Vec<u16>> {
    let mut buffer = vec![0u16; INITIAL_PROCESS_PATH_CAPACITY];
    loop {
        match query(&mut buffer) {
            Ok(length) => {
                // QueryFullProcessImageNameW excludes the terminating NUL.
                if length == 0 || length >= buffer.len() {
                    bail!(
                        "Process path API returned invalid UTF-16 length {length} for capacity {}",
                        buffer.len()
                    );
                }
                buffer.truncate(length);
                return Ok(buffer);
            }
            Err(error) if error.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0) => {
                if buffer.len() == MAX_PROCESS_PATH_CAPACITY {
                    return Err(error).with_context(|| {
                        format!(
                            "Process path exceeds the {MAX_PROCESS_PATH_CAPACITY} UTF-16 buffer limit"
                        )
                    });
                }
                let capacity = buffer
                    .len()
                    .saturating_mul(2)
                    .min(MAX_PROCESS_PATH_CAPACITY);
                debug!(
                    "process path buffer retry pid={pid} capacity_utf16={} next_capacity_utf16={capacity}",
                    buffer.len()
                );
                buffer.resize(capacity, 0);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{core::Error, Win32::Foundation::ERROR_ACCESS_DENIED};

    fn copy_path(path: &[u16], buffer: &mut [u16]) -> windows::core::Result<usize> {
        if path.len() >= buffer.len() {
            return Err(Error::from(ERROR_INSUFFICIENT_BUFFER));
        }
        buffer[..path.len()].copy_from_slice(path);
        buffer[path.len()] = 0;
        Ok(path.len())
    }

    #[test]
    fn short_paths_need_only_one_query() {
        let expected: Vec<u16> = "C:\\工具\\应用.exe".encode_utf16().collect();
        let mut calls = 0;
        let result = query_process_path(1, |buffer| {
            calls += 1;
            copy_path(&expected, buffer)
        })
        .unwrap();
        assert_eq!(result, expected);
        assert_eq!(calls, 1);
    }

    #[test]
    fn growth_preserves_utf16_and_reserves_the_terminator() {
        for length in [259, 260, 519, 520, MAX_PROCESS_PATH_CAPACITY - 1] {
            let mut expected = vec![b'x' as u16; length];
            let unicode: Vec<u16> = "中文\u{1D11E}".encode_utf16().collect();
            expected[..unicode.len()].copy_from_slice(&unicode);
            let result = query_process_path(1, |buffer| copy_path(&expected, buffer)).unwrap();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn non_buffer_errors_are_not_retried() {
        let mut calls = 0;
        let error = query_process_path(1, |_| {
            calls += 1;
            Err(Error::from(ERROR_ACCESS_DENIED))
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().code(),
            HRESULT::from_win32(ERROR_ACCESS_DENIED.0)
        );
    }

    #[test]
    fn repeated_buffer_errors_stop_at_the_safety_limit() {
        let mut capacities = Vec::new();
        let error = query_process_path(1, |buffer| {
            capacities.push(buffer.len());
            assert!(capacities.len() <= 10, "Path query did not stop retrying");
            assert!(buffer.len() <= MAX_PROCESS_PATH_CAPACITY);
            Err(Error::from(ERROR_INSUFFICIENT_BUFFER))
        })
        .unwrap_err();
        assert_eq!(capacities.last(), Some(&MAX_PROCESS_PATH_CAPACITY));
        assert!(capacities.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(error.to_string().contains("buffer limit"));
    }

    #[test]
    fn invalid_success_lengths_are_rejected_without_retry() {
        for length in [
            0,
            INITIAL_PROCESS_PATH_CAPACITY,
            INITIAL_PROCESS_PATH_CAPACITY + 1,
        ] {
            let mut calls = 0;
            let error = query_process_path(1, |_| {
                calls += 1;
                Ok(length)
            })
            .unwrap_err();
            assert_eq!(calls, 1);
            assert!(error.to_string().contains("invalid UTF-16 length"));
        }
    }
}
