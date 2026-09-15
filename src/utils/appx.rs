use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Component, Path, PathBuf},
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS},
        Storage::Packaging::Appx::{GetPackagePathByFullName, GetPackagesByPackageFamily},
    },
};
use xml::{reader::XmlEvent, EventReader};

pub(crate) fn package_directory(family: &str) -> Option<PathBuf> {
    if family.is_empty() || family.contains('\0') || family.encode_utf16().count() > 255 {
        return None;
    }
    let family = super::to_wstring(family);
    let (mut count, mut length) = (0, 0);
    if unsafe {
        GetPackagesByPackageFamily(PCWSTR(family.as_ptr()), &mut count, None, &mut length, None)
    } != ERROR_INSUFFICIENT_BUFFER
        || count == 0
        || count > 256
        || length == 0
        || length > 262144
    {
        return None;
    }
    let mut names = vec![PWSTR::null(); count as usize];
    let mut buffer = vec![0u16; length as usize];
    if unsafe {
        GetPackagesByPackageFamily(
            PCWSTR(family.as_ptr()),
            &mut count,
            Some(names.as_mut_ptr()),
            &mut length,
            Some(PWSTR(buffer.as_mut_ptr())),
        )
    } != ERROR_SUCCESS
        || count as usize > names.len()
        || length as usize > buffer.len()
    {
        return None;
    }
    let mut full_names = Vec::new();
    for name in names.into_iter().take(count as usize) {
        let offset = (name.0 as usize).checked_sub(buffer.as_ptr() as usize)?;
        if offset % 2 != 0 {
            return None;
        }
        let tail = buffer.get(offset / 2..length as usize)?;
        let end = tail.iter().position(|c| *c == 0)?;
        full_names.push(String::from_utf16(&tail[..end]).ok()?);
    }
    full_names.sort();
    for name in full_names {
        let name = super::to_wstring(&name);
        let mut length = 0;
        if unsafe { GetPackagePathByFullName(PCWSTR(name.as_ptr()), &mut length, None) }
            != ERROR_INSUFFICIENT_BUFFER
            || !(2..=32768).contains(&length)
        {
            continue;
        }
        let mut path = vec![0u16; length as usize];
        if unsafe {
            GetPackagePathByFullName(
                PCWSTR(name.as_ptr()),
                &mut length,
                Some(PWSTR(path.as_mut_ptr())),
            )
        } != ERROR_SUCCESS
            || length < 2
            || length as usize > path.len()
            || path[length as usize - 1] != 0
        {
            continue;
        }
        if let Ok(path) = String::from_utf16(&path[..length as usize - 1]) {
            return Some(PathBuf::from(path));
        }
    }
    None
}

pub(crate) fn executable_logo(executable: &Path) -> Option<PathBuf> {
    for directory in executable.parent()?.ancestors().take(8) {
        if !directory.join("AppxManifest.xml").is_file() {
            continue;
        }
        let relative = executable.strip_prefix(directory).ok()?.to_str()?;
        return package_logo(directory, Some(relative));
    }
    None
}

pub(crate) fn package_logo(directory: &Path, executable: Option<&str>) -> Option<PathBuf> {
    const MANIFEST_LIMIT: u64 = 4 * 1024 * 1024;
    let file = File::open(directory.join("AppxManifest.xml")).ok()?;
    if file.metadata().ok()?.len() > MANIFEST_LIMIT {
        return None;
    }
    let reader = EventReader::new(BufReader::new(file.take(MANIFEST_LIMIT)));
    let mut elements = Vec::new();
    let mut matched = false;
    for event in reader {
        match event.ok()? {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                if elements.len() >= 32 {
                    return None;
                }
                elements.push(name.local_name);
                if elements.iter().map(String::as_str).eq([
                    "Package",
                    "Applications",
                    "Application",
                ]) {
                    matched = executable.is_none_or(|exe| {
                        attributes.iter().any(|a| {
                            a.name.local_name == "Executable"
                                && a.value
                                    .replace('/', "\\")
                                    .eq_ignore_ascii_case(&exe.replace('/', "\\"))
                        })
                    });
                } else if matched
                    && elements.iter().map(String::as_str).eq([
                        "Package",
                        "Applications",
                        "Application",
                        "VisualElements",
                    ])
                {
                    for key in ["Square44x44Logo", "Square30x30Logo", "SmallLogo"] {
                        if let Some(logo) = attributes
                            .iter()
                            .find(|a| a.name.local_name == key)
                            .and_then(|a| resolve_logo(directory, &a.value))
                        {
                            return Some(logo);
                        }
                    }
                }
            }
            XmlEvent::EndElement { .. } => {
                elements.pop()?;
            }
            _ => {}
        }
    }
    None
}

fn resolve_logo(base: &Path, value: &str) -> Option<PathBuf> {
    let value = Path::new(value);
    if !value
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let original = base.join(value);
    let stem = original.file_stem()?.to_str()?;
    let extension = original.extension()?.to_str()?;
    for suffix in ["targetsize-256", "targetsize-128", "scale-200", "scale-100"] {
        let path = original.with_file_name(format!("{stem}.{suffix}.{extension}"));
        if path.is_file() {
            return Some(path);
        }
    }
    original.is_file().then_some(original)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_pointer_inputs_and_logo_traversal_are_rejected() {
        assert!(package_directory("").is_none());
        assert!(package_directory("bad\0family").is_none());
        assert!(resolve_logo(Path::new(r"Q:\Apps"), r"..\icon.png").is_none());
        assert!(resolve_logo(Path::new(r"Q:\Apps"), r"C:\icon.png").is_none());
    }

    #[test]
    fn manifest_logo_resolution_supports_original_files_and_nested_executables() {
        let directory = crate::config::test_support::TestDirectory::new();
        let assets = directory.0.join("Assets");
        std::fs::create_dir(&assets).unwrap();
        std::fs::write(assets.join("Logo.png"), b"fixture").unwrap();
        std::fs::write(directory.0.join("AppxManifest.xml"), r#"<Package><Applications><Application Executable="folder\fixture.exe"><VisualElements Square44x44Logo="Assets\Logo.png"/></Application></Applications></Package>"#).unwrap();
        assert_eq!(
            package_logo(&directory.0, Some(r"FOLDER\FIXTURE.EXE")),
            Some(assets.join("Logo.png"))
        );
        assert_eq!(
            executable_logo(&directory.0.join("folder").join("fixture.exe")),
            Some(assets.join("Logo.png"))
        );
        assert!(package_logo(&directory.0, Some("other.exe")).is_none());
        std::fs::write(assets.join("Logo.targetsize-256.png"), b"fixture").unwrap();
        assert_eq!(
            package_logo(&directory.0, None),
            Some(assets.join("Logo.targetsize-256.png"))
        );
    }
}
