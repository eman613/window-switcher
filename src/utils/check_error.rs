use windows::core::Error;
use windows::Win32::Foundation::{GetLastError, SetLastError, ERROR_SUCCESS};

#[allow(unused)]
#[inline]
/// Only for ambiguous-zero APIs such as Get/SetWindowLongPtrW. BOOL,
/// HRESULT, handles and GDI+ Status must be checked using their own contracts.
pub fn check_error<F, R>(mut f: F) -> windows::core::Result<R>
where
    F: FnMut() -> R,
{
    unsafe {
        SetLastError(ERROR_SUCCESS);
        let result = f();
        let error = GetLastError();
        if error == ERROR_SUCCESS {
            Ok(result)
        } else {
            Err(Error::from_hresult(error.to_hresult()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;

    #[test]
    fn ambiguous_zero_clears_stale_error_and_reports_the_current_failure() {
        unsafe { SetLastError(ERROR_ACCESS_DENIED) };
        assert_eq!(check_error(|| 0isize).unwrap(), 0);
        let failure = check_error(|| {
            unsafe { SetLastError(ERROR_ACCESS_DENIED) };
            0isize
        })
        .unwrap_err();
        assert_eq!(failure.code(), ERROR_ACCESS_DENIED.to_hresult());
    }
}
