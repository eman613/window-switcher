//! Handle-based file identity and non-overwriting namespace operations.
use std::{
    fs::{File, OpenOptions},
    io,
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf},
};

use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        FileDispositionInfo, FileIdInfo, FileRenameInfo, GetFileInformationByHandleEx, GetFileType,
        SetFileInformationByHandle, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TYPE_DISK,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileIdentity {
    volume: u64,
    id: [u8; 16],
}

pub(super) fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

impl FileIdentity {
    pub(super) fn archive_key(self) -> String {
        format!(
            "{:016x}-{}",
            self.volume,
            self.id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    }

    pub(super) fn read(file: &File) -> io::Result<Self> {
        let mut info = FILE_ID_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                handle(file),
                FileIdInfo,
                &mut info as *mut _ as _,
                size_of::<FILE_ID_INFO>() as u32,
            )
        }?;
        if info.FileId.Identifier == [0; 16] {
            return Err(io::Error::other("文件系统未提供稳定文件身份"));
        }
        Ok(Self {
            volume: info.VolumeSerialNumber,
            id: info.FileId.Identifier,
        })
    }
}

pub(super) fn require_regular(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || unsafe { GetFileType(handle(file)) } != FILE_TYPE_DISK
    {
        return Err(io::Error::other(
            "配置及日志必须是普通文件，不支持重解析点或设备",
        ));
    }
    Ok(())
}

/// Pin every directory component while using a pathname. No ancestor may be
/// replaced under an identity check. Reparse components fail closed.
pub(super) struct PinnedPath {
    pub(super) path: PathBuf,
    _directories: Vec<File>,
}

impl PinnedPath {
    pub(super) fn new(path: &Path) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("文件缺少父目录"))?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("文件缺少名称"))?;
        if name.encode_wide().any(|c| c == b':' as u16 || c == 0) {
            return Err(io::Error::other("配置及日志不支持备用数据流或空字符"));
        }
        let mut current = PathBuf::new();
        let mut directories = Vec::new();
        for component in parent.components() {
            current.push(component);
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            if matches!(component, Component::ParentDir) {
                return Err(io::Error::other("请使用不包含父目录跳转的文件路径"));
            }
            let directory = OpenOptions::new()
                // Attribute-only handles do not participate in delete sharing.
                // LIST_DIRECTORY supplies read-data access so omitting SHARE_DELETE
                // actually prevents an ancestor from being renamed/replaced.
                .access_mode(FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0)
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(&current)?;
            let metadata = directory.metadata()?;
            if !metadata.is_dir()
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            {
                return Err(io::Error::other("配置及日志目录不支持重解析路径"));
            }
            directories.push(directory);
        }
        Ok(Self {
            path,
            _directories: directories,
        })
    }
}

/// The caller owns DELETE access. Both rename and cleanup act on that exact
/// handle, never on a pathname that another writer can substitute.
pub(super) fn rename_no_replace(file: &File, destination: &Path) -> io::Result<()> {
    let name: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let offset = offset_of!(FILE_RENAME_INFO, FileName);
    let name_bytes = (name.len() - 1)
        .checked_mul(2)
        .ok_or_else(|| io::Error::other("路径过长"))?;
    let length = offset
        .checked_add(name_bytes + 2)
        .ok_or_else(|| io::Error::other("路径过长"))?
        .max(size_of::<FILE_RENAME_INFO>());
    let api_length = u32::try_from(length).map_err(|_| io::Error::other("路径过长"))?;
    // usize storage guarantees FILE_RENAME_INFO alignment on x86/x64/ARM64.
    let mut buffer = vec![0usize; length.div_ceil(size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<u8>().add(offset).cast(),
            name.len(),
        );
        SetFileInformationByHandle(handle(file), FileRenameInfo, info.cast(), api_length)?;
    }
    Ok(())
}

pub(super) fn delete_owned(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe {
        SetFileInformationByHandle(
            handle(file),
            FileDispositionInfo,
            &info as *const _ as _,
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )?;
    }
    Ok(())
}
