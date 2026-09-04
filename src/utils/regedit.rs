use anyhow::{anyhow, bail, Result};
use windows::core::PCWSTR;
use windows::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS},
    System::Registry::{
        RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD_BIG_ENDIAN, REG_SZ,
        REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    },
};

#[derive(Debug)]
pub struct RegKey {
    hkey: HKEY,
    name: PCWSTR,
}

impl RegKey {
    pub fn new_hkcu(subkey: PCWSTR, name: PCWSTR) -> Result<RegKey> {
        let mut hkey = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey,
                None,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &mut hkey as *mut _,
            )
        };
        if status != ERROR_SUCCESS {
            bail!(
                "Fail to open reg key, {:?}",
                windows::core::Error::from(status)
            );
        }
        Ok(RegKey { hkey, name })
    }

    pub fn get_value(&self) -> Result<Option<Vec<u16>>> {
        const MAX_VALUE_BYTES: usize = 1024 * 1024;
        let mut buffer = vec![0u16; 256];
        for _ in 0..8 {
            let mut size = u32::try_from(buffer.len().saturating_mul(2))?;
            let mut kind: REG_VALUE_TYPE = Default::default();
            let status = unsafe {
                RegGetValueW(
                    self.hkey,
                    None,
                    self.name,
                    RRF_RT_REG_SZ,
                    Some(&mut kind),
                    Some(buffer.as_mut_ptr().cast()),
                    Some(&mut size),
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            if status == ERROR_MORE_DATA {
                let required = usize::try_from(size)?;
                let current = buffer.len().saturating_mul(2);
                let next = required.max(current.saturating_mul(2));
                if next <= current || next > MAX_VALUE_BYTES {
                    bail!("Registry string value exceeds the safety limit");
                }
                buffer.resize(next.div_ceil(2), 0);
                continue;
            }
            if status != ERROR_SUCCESS {
                bail!(
                    "Fail to get reg value, {:?}",
                    windows::core::Error::from(status)
                );
            }
            if kind != REG_SZ {
                bail!("Registry value has unexpected type {:?}", kind);
            }
            let byte_length = usize::try_from(size)?;
            if byte_length % 2 != 0 || byte_length > buffer.len().saturating_mul(2) {
                bail!("Registry string value has an invalid size {byte_length}");
            }
            buffer.truncate(byte_length / 2);
            trim_trailing_nul(&mut buffer);
            return Ok(Some(buffer));
        }
        bail!("Registry string value size changed while querying")
    }

    pub fn get_int(&self) -> Result<u32> {
        let mut value: [u8; 4] = Default::default();
        let mut size: u32 = std::mem::size_of_val(&value) as u32;
        let mut kind: REG_VALUE_TYPE = Default::default();
        let ret = unsafe {
            RegGetValueW(
                self.hkey,
                None,
                self.name,
                RRF_RT_REG_DWORD,
                Some(&mut kind),
                Some(value.as_mut_ptr() as *mut _),
                Some(&mut size),
            )
        };
        if ret != ERROR_SUCCESS {
            bail!(
                "Fail to get reg value, {:?}",
                windows::core::Error::from(ret)
            );
        }
        if size != std::mem::size_of::<u32>() as u32 {
            bail!("Registry DWORD value has an invalid size {size}");
        }
        let value = if kind == REG_DWORD_BIG_ENDIAN {
            u32::from_be_bytes(value)
        } else {
            u32::from_le_bytes(value)
        };
        Ok(value)
    }

    pub fn set_value(&self, value: &[u8]) -> Result<()> {
        if !value.len().is_multiple_of(2) {
            bail!("Registry string value must contain UTF-16 bytes");
        }
        let mut data = value.to_vec();
        if data.len() < 2 || data[data.len() - 2..] != [0, 0] {
            data.extend_from_slice(&[0, 0]);
        }
        let status = unsafe { RegSetValueExW(self.hkey, self.name, None, REG_SZ, Some(&data)) };
        if status != ERROR_SUCCESS {
            return Err(anyhow!(
                "Fail to write reg value, {:?}",
                windows::core::Error::from(status)
            ));
        }
        Ok(())
    }

    pub fn delete_value(&self) -> Result<()> {
        let status = unsafe { RegDeleteValueW(self.hkey, self.name) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status != ERROR_SUCCESS {
            return Err(anyhow!(
                "Failed to delete reg value, {:?}",
                windows::core::Error::from(status)
            ));
        }
        Ok(())
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        if !self.hkey.is_invalid() {
            let _ = unsafe { RegCloseKey(self.hkey) };
        }
    }
}

fn trim_trailing_nul(value: &mut Vec<u16>) {
    while value.last() == Some(&0) {
        value.pop();
    }
}
