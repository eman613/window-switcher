use anyhow::{bail, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA},
    System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
        RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    },
};

#[derive(Debug)]
pub struct RegKey {
    hkey: HKEY,
    name: Vec<u16>,
}

impl RegKey {
    pub fn new_hkcu(subkey: PCWSTR, name: PCWSTR) -> Result<Self> {
        Self::open(subkey, name, false)
    }
    pub(crate) fn writable_hkcu(subkey: PCWSTR, name: PCWSTR) -> Result<Self> {
        Self::open(subkey, name, true)
    }

    fn open(subkey: PCWSTR, name: PCWSTR, writable: bool) -> Result<Self> {
        let mut hkey = HKEY::default();
        let result = unsafe {
            if writable {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    subkey,
                    None,
                    None,
                    REG_OPTION_NON_VOLATILE,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    None,
                    &mut hkey,
                    None,
                )
            } else {
                RegOpenKeyExW(HKEY_CURRENT_USER, subkey, None, KEY_QUERY_VALUE, &mut hkey)
            }
        };
        result.ok().context("无法打开当前用户注册表项")?;
        let name = unsafe { name.as_wide() }
            .iter()
            .copied()
            .chain(Some(0))
            .collect();
        Ok(Self { hkey, name })
    }

    fn name(&self) -> PCWSTR {
        PCWSTR(self.name.as_ptr())
    }

    pub fn get_value(&self) -> Result<Option<Vec<u16>>> {
        for _ in 0..4 {
            let mut size = 0;
            let status = unsafe {
                RegGetValueW(
                    self.hkey,
                    None,
                    self.name(),
                    RRF_RT_REG_SZ,
                    None,
                    None,
                    Some(&mut size),
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            status.ok().context("无法查询注册表字符串长度或类型")?;
            if size > 65536 || !size.is_multiple_of(2) {
                bail!("注册表字符串长度无效");
            }
            let mut buffer = vec![0u16; size as usize / 2 + 1];
            let mut capacity = (buffer.len() * 2) as u32;
            let status = unsafe {
                RegGetValueW(
                    self.hkey,
                    None,
                    self.name(),
                    RRF_RT_REG_SZ,
                    None,
                    Some(buffer.as_mut_ptr().cast()),
                    Some(&mut capacity),
                )
            };
            if status == ERROR_MORE_DATA {
                continue;
            }
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            status.ok().context("无法读取注册表字符串")?;
            if !capacity.is_multiple_of(2) || capacity as usize > buffer.len() * 2 {
                bail!("注册表返回了无效字符串长度");
            }
            buffer.truncate(capacity as usize / 2);
            if buffer.last() == Some(&0) {
                buffer.pop();
            }
            return Ok(Some(buffer));
        }
        bail!("注册表字符串持续变化；请稍后重试")
    }

    pub(crate) fn get_string(&self) -> Result<Option<String>> {
        self.get_value()?
            .map(|value| {
                if value.contains(&0) {
                    bail!("注册表字符串含嵌入 NUL");
                }
                String::from_utf16(&value).context("注册表字符串不是有效 UTF-16")
            })
            .transpose()
    }

    pub fn get_int(&self) -> Result<u32> {
        let mut value = [0u8; 4];
        let mut size = 4;
        unsafe {
            RegGetValueW(
                self.hkey,
                None,
                self.name(),
                RRF_RT_REG_DWORD,
                None,
                Some(value.as_mut_ptr().cast()),
                Some(&mut size),
            )
        }
        .ok()
        .context("无法读取注册表 DWORD")?;
        if size != 4 {
            bail!("注册表 DWORD 长度无效");
        }
        Ok(u32::from_le_bytes(value))
    }

    pub(crate) fn compare_string(
        &self,
        expected: Option<&str>,
        replacement: Option<&str>,
    ) -> Result<()> {
        if self.get_string()?.as_deref() != expected {
            bail!("自启动注册表值已被外部修改；本次操作取消");
        }
        match replacement {
            Some(value) => {
                if value.contains('\0') {
                    bail!("自启动命令不可包含 NUL");
                }
                let bytes: Vec<_> = value
                    .encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes)
                    .collect();
                unsafe { RegSetValueExW(self.hkey, self.name(), None, REG_SZ, Some(&bytes)) }
                    .ok()
                    .context("无法写入自启动注册表值")?;
            }
            None => {
                let status = unsafe { RegDeleteValueW(self.hkey, self.name()) };
                if status != ERROR_FILE_NOT_FOUND {
                    status.ok().context("无法删除本应用自启动注册表值")?;
                }
            }
        }
        Ok(())
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.hkey) };
    }
}

#[cfg(test)]
mod tests;
