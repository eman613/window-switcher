//! Checks ordinary attributes, streams, owner/group/DACL and integrity labels
//! before copying user bytes. Privileged audit SACLs are not queried or preserved;
//! this is not a guarantee of parity for every Windows security metadata field.
use std::{
    fs::File,
    mem::{offset_of, size_of, size_of_val},
};

use anyhow::{bail, Context, Result};
use windows::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Security::{
        GetKernelObjectSecurity, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION,
        LABEL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    },
    Storage::FileSystem::{
        FileBasicInfo, FileStreamInfo, GetFileInformationByHandleEx, SetFileInformationByHandle,
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_SYSTEM, FILE_BASIC_INFO,
        FILE_STREAM_INFO,
    },
};

use super::file_identity::handle;

// This bound is a safety invariant, not a user setting.
const MAX_SECURITY_BYTES: usize = 256 * 1024;

pub(super) struct PreservedMetadata {
    basic: FILE_BASIC_INFO,
    security: Vec<u32>,
}

impl PreservedMetadata {
    pub(super) fn capture(original: &File, candidate: &File) -> Result<Self> {
        let basic = basic_information(original)?;
        require_supported_attributes(basic.FileAttributes)?;
        require_supported_attributes(basic_information(candidate)?.FileAttributes)?;
        require_single_stream(original)?;
        let security = security_information(original)?;
        if security != security_information(candidate)? {
            bail!("INI 与候选文件的所有者、权限或完整性标签不一致；不降低文件权限，原文件未修改");
        }
        Ok(Self { basic, security })
    }

    pub(super) fn apply(&self, original: &File, candidate: &File) -> Result<()> {
        if security_information(original)? != self.security
            || security_information(candidate)? != self.security
            || basic_information(original)?.FileAttributes != self.basic.FileAttributes
        {
            bail!("INI 元数据在补全期间变化；已取消保存，原文件未修改");
        }
        require_single_stream(original)?;
        let basic = FILE_BASIC_INFO {
            CreationTime: self.basic.CreationTime,
            LastAccessTime: self.basic.LastAccessTime,
            // LastWriteTime/ChangeTime intentionally reflect this content update.
            FileAttributes: self.basic.FileAttributes,
            ..Default::default()
        };
        unsafe {
            SetFileInformationByHandle(
                handle(candidate),
                FileBasicInfo,
                &basic as *const _ as _,
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        }
        .context("无法保留 INI 创建时间和文件属性；原文件未修改")?;
        Ok(())
    }
}

fn basic_information(file: &File) -> Result<FILE_BASIC_INFO> {
    let mut basic = FILE_BASIC_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileBasicInfo,
            &mut basic as *mut _ as _,
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .context("无法读取 INI 文件属性")?;
    Ok(basic)
}

fn require_supported_attributes(attributes: u32) -> Result<()> {
    let supported = FILE_ATTRIBUTE_ARCHIVE.0
        | FILE_ATTRIBUTE_HIDDEN.0
        | FILE_ATTRIBUTE_NORMAL.0
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED.0
        | FILE_ATTRIBUTE_SYSTEM.0;
    if attributes & !supported != 0 {
        bail!("INI 含只读、加密、压缩或其他特殊文件属性；无法无损补全，原文件未修改");
    }
    Ok(())
}

fn require_single_stream(file: &File) -> Result<()> {
    // A plain unnamed stream is much smaller than this. A larger result or any
    // additional stream is unsupported and fails closed without parsing it.
    let mut storage = [0u64; 64];
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileStreamInfo,
            storage.as_mut_ptr().cast(),
            size_of_val(&storage) as u32,
        )
    }
    .context("无法核验 INI 数据流；原文件未修改")?;
    let stream = unsafe { &*storage.as_ptr().cast::<FILE_STREAM_INFO>() };
    let expected: Vec<u16> = "::$DATA".encode_utf16().collect();
    if stream.NextEntryOffset != 0 || stream.StreamNameLength as usize != expected.len() * 2 {
        bail!("INI 含附加数据流；无法无损补全，原文件未修改");
    }
    let name = unsafe {
        std::slice::from_raw_parts(
            storage
                .as_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_STREAM_INFO, StreamName))
                .cast::<u16>(),
            expected.len(),
        )
    };
    if name != expected {
        bail!("INI 数据流类型不受支持；原文件未修改");
    }
    Ok(())
}

fn security_information(file: &File) -> Result<Vec<u32>> {
    // OWNER/GROUP/DACL and mandatory integrity labels are readable with
    // READ_CONTROL. Do not request or enable privileged SACL access here.
    let requested = OWNER_SECURITY_INFORMATION
        | GROUP_SECURITY_INFORMATION
        | DACL_SECURITY_INFORMATION
        | LABEL_SECURITY_INFORMATION;
    let mut needed = 0;
    let probe = unsafe { GetKernelObjectSecurity(handle(file), requested.0, None, 0, &mut needed) };
    if !probe
        .as_ref()
        .is_err_and(|err| err.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult())
    {
        probe.context("无法查询 INI 安全描述符长度")?;
        bail!("INI 安全描述符长度响应异常");
    }
    if needed == 0 || needed as usize > MAX_SECURITY_BYTES {
        bail!("INI 安全描述符超过安全上限；原文件未修改");
    }
    let mut bytes = vec![0u32; (needed as usize).div_ceil(size_of::<u32>())];
    let capacity = (bytes.len() * size_of::<u32>()) as u32;
    unsafe {
        GetKernelObjectSecurity(
            handle(file),
            requested.0,
            Some(PSECURITY_DESCRIPTOR(bytes.as_mut_ptr().cast())),
            capacity,
            &mut needed,
        )
    }
    .context("无法读取 INI 安全描述符；原文件未修改")?;
    if needed == 0 || needed > capacity {
        bail!("INI 安全描述符返回长度无效");
    }
    bytes.truncate((needed as usize).div_ceil(size_of::<u32>()));
    Ok(bytes)
}

#[cfg(test)]
pub(super) fn change_test_dacl_protection(file: &File) {
    use windows::Win32::Security::{
        GetSecurityDescriptorControl, SetKernelObjectSecurity, PROTECTED_DACL_SECURITY_INFORMATION,
        SE_DACL_PROTECTED, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    let mut security = security_information(file).unwrap();
    let descriptor = PSECURITY_DESCRIPTOR(security.as_mut_ptr().cast());
    let mut control = 0;
    let mut revision = 0;
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.unwrap();
    // CI temporary files can already have protected DACLs. Always change the
    // protection state instead of assuming inheritance is initially enabled.
    let protection = if control & SE_DACL_PROTECTED.0 == 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };
    unsafe {
        SetKernelObjectSecurity(
            handle(file),
            DACL_SECURITY_INFORMATION | protection,
            descriptor,
        )
    }
    .unwrap();
    assert_ne!(
        security_information(file).unwrap(),
        security,
        "DACL fixture must change the original security descriptor"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_ENCRYPTED,
    };

    #[test]
    fn unsupported_attributes_are_rejected_without_downgrading_them() {
        for flags in [FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_COMPRESSED] {
            assert!(require_supported_attributes(flags.0).is_err());
        }
        assert!(
            require_supported_attributes(FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_ARCHIVE.0)
                .is_ok()
        );
    }
}
