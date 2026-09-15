//! Browser identities and deterministic, bounded profile/shortcut lookup.
use crate::config::Config;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::HWND,
        Storage::EnhancedStorage::PKEY_AppUserModel_ID,
        System::Com::CoTaskMemFree,
        UI::Shell::{
            FOLDERID_LocalAppData,
            PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
            SHGetKnownFolderPath, KF_FLAG_DEFAULT,
        },
    },
};

#[derive(Clone)]
pub(crate) struct BrowserPaths {
    chrome: Option<PathBuf>,
    edge: Option<PathBuf>,
}

impl BrowserPaths {
    pub(crate) fn new(config: &Config, ini_dir: &Path) -> Self {
        let local = local_app_data();
        Self::with_local(config, ini_dir, local.as_deref())
    }

    fn with_local(config: &Config, ini_dir: &Path, local: Option<&Path>) -> Self {
        let resolve = |value: &Option<PathBuf>, suffix: &str| match value {
            Some(path) if path.is_absolute() => Some(path.clone()),
            Some(path) => Some(ini_dir.join(path)),
            None => local.map(|path| path.join(suffix)),
        };
        Self {
            chrome: resolve(&config.chrome_user_data_dir, r"Google\Chrome\User Data"),
            edge: resolve(&config.edge_user_data_dir, r"Microsoft\Edge\User Data"),
        }
    }

    pub(crate) fn root(&self, executable: &str) -> Option<&Path> {
        match exe_name(executable).to_ascii_lowercase().as_str() {
            "chrome.exe" => self.chrome.as_deref(),
            "msedge.exe" => self.edge.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn profile_icon(&self, executable: &str, profile: &str) -> Option<PathBuf> {
        let profile = profile_directory(profile)?;
        let filename = if exe_name(executable).eq_ignore_ascii_case("chrome.exe") {
            "Google Profile.ico"
        } else {
            "Edge Profile.ico"
        };
        Some(self.root(executable)?.join(profile).join(filename))
    }
}

fn local_app_data() -> Option<PathBuf> {
    let raw: PWSTR =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) }.ok()?;
    let value = unsafe { raw.to_string() }.ok().map(PathBuf::from);
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    value
}

pub(crate) fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

pub(crate) fn group_key(path: &Arc<str>, hwnd: HWND) -> Arc<str> {
    let chrome = exe_name(path).eq_ignore_ascii_case("chrome.exe");
    let edge = exe_name(path).eq_ignore_ascii_case("msedge.exe");
    if !chrome && !edge {
        return path.clone();
    }
    let Some(aumid) = get_aumid(hwnd) else {
        return path.clone();
    };
    let (profile, app) = if chrome {
        parse_chrome(&aumid)
    } else {
        parse_edge(&aumid)
    };
    match (profile, app) {
        (Some(profile), Some(app)) => format!("{path}::{profile}::{app}").into(),
        (None, Some(app)) if edge => format!("{path}::appx::{app}").into(),
        (None, Some(app)) => format!("{path}::Default::{app}").into(),
        (Some(profile), None) => format!("{path}::{profile}").into(),
        _ => path.clone(),
    }
}

fn get_aumid(hwnd: HWND) -> Option<String> {
    let store: IPropertyStore = unsafe { SHGetPropertyStoreForWindow(hwnd) }.ok()?;
    let property = unsafe { store.GetValue(&PKEY_AppUserModel_ID) }.ok()?;
    let value = property.to_string();
    (value.encode_utf16().count() <= 4096).then_some(value)
}

fn component(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 1024
        && !value
            .chars()
            .any(|c| matches!(c, '\\' | '/' | ':' | '\0') || c.is_control()))
    .then_some(value)
}

fn parse_edge(aumid: &str) -> (Option<&str>, Option<&str>) {
    if let Some(package) = aumid.strip_suffix("!App").and_then(component) {
        return (None, Some(package));
    }
    (
        aumid.strip_prefix("MSEdge.UserData.").and_then(component),
        None,
    )
}

fn parse_chrome(aumid: &str) -> (Option<&str>, Option<&str>) {
    if let Some((_, suffix)) = aumid.split_once("_crx_") {
        let (app, profile) = suffix
            .split_once(".UserData.")
            .unwrap_or((suffix, "Default"));
        if let (Some(app), Some(profile)) = (component(app), component(profile)) {
            return ((profile != "Default").then_some(profile), Some(app));
        }
        return (None, None);
    }
    (
        aumid.strip_prefix("Chrome.UserData.").and_then(component),
        None,
    )
}

pub(crate) fn profile_directory(profile: &str) -> Option<String> {
    let profile = component(profile)?;
    if let Some(number) = profile.strip_prefix("Profile") {
        if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("Profile {number}"));
        }
    }
    Some(profile.to_owned())
}

pub(crate) fn sorted_directory(path: &Path) -> Option<Vec<PathBuf>> {
    const LIMIT: usize = 4096;
    let entries: Vec<_> = std::fs::read_dir(path)
        .ok()?
        .take(LIMIT + 1)
        .collect::<std::io::Result<Vec<_>>>()
        .ok()?;
    if entries.len() > LIMIT {
        warn!("icon stage=directory limit-exceeded");
        return None;
    }
    let mut paths: Vec<_> = entries.into_iter().map(|entry| entry.path()).collect();
    paths.sort_by_cached_key(|path| {
        (
            path.as_os_str().to_string_lossy().to_lowercase(),
            path.clone(),
        )
    });
    Some(paths)
}

pub(crate) fn pwa_shortcut(root: &Path, profile: &str, app_id: &str) -> Option<PathBuf> {
    let app_id = component(app_id)?;
    let directory = root
        .join(profile_directory(profile)?)
        .join("Web Applications");
    for path in sorted_directory(&directory)? {
        let name = path.file_name()?.to_str()?;
        let identity = name.strip_prefix("_crx_").unwrap_or(name);
        if identity.eq_ignore_ascii_case(app_id) && path.is_dir() {
            return sorted_directory(&path)?.into_iter().find(|path| {
                path.extension()
                    .is_some_and(|v| v.eq_ignore_ascii_case("lnk"))
                    && path.is_file()
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_identity_and_profile_paths_do_not_accept_traversal() {
        assert_eq!(
            parse_chrome("Chrome._crx_abcdefghijklmnop.UserData.Profile1"),
            (Some("Profile1"), Some("abcdefghijklmnop"))
        );
        assert_eq!(parse_chrome("Chrome._crx_.UserData.Profile1"), (None, None));
        assert_eq!(
            parse_edge("MSEdge.UserData.Profile2"),
            (Some("Profile2"), None)
        );
        assert_eq!(
            parse_edge("package_family!App"),
            (None, Some("package_family"))
        );
        for bad in ["..", r"..\Default", "C:foo", "bad::id"] {
            assert!(profile_directory(bad).is_none());
        }
        assert_eq!(profile_directory("Profile1").as_deref(), Some("Profile 1"));
        assert_eq!(profile_directory("Profile").as_deref(), Some("Profile"));
    }
    #[test]
    fn configured_directories_resolve_against_ini_and_default_uses_known_folder() {
        let config = Config {
            chrome_user_data_dir: Some(PathBuf::from("profiles")),
            edge_user_data_dir: Some(PathBuf::from(r"Q:\Edge")),
            ..Default::default()
        };
        let paths = BrowserPaths::with_local(
            &config,
            Path::new(r"R:\config"),
            Some(Path::new(r"S:\Local")),
        );
        assert_eq!(
            paths.root(r"Z:\chrome.exe"),
            Some(Path::new(r"R:\config\profiles"))
        );
        assert_eq!(paths.root(r"Z:\msedge.exe"), Some(Path::new(r"Q:\Edge")));
        assert!(paths.root(r"Z:\notchrome.exe").is_none());
        let paths = BrowserPaths::with_local(
            &Config::default(),
            Path::new(r"R:\config"),
            Some(Path::new(r"S:\Local")),
        );
        assert_eq!(
            paths.root("chrome.exe"),
            Some(Path::new(r"S:\Local\Google\Chrome\User Data"))
        );
    }

    #[test]
    fn shortcuts_require_exact_application_identity_and_are_sorted() {
        let directory = crate::config::test_support::TestDirectory::new();
        let app = directory
            .0
            .join("Profile 1")
            .join("Web Applications")
            .join("_crx_abcdef");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("z.lnk"), b"fixture").unwrap();
        std::fs::write(app.join("a.LNK"), b"fixture").unwrap();
        assert!(pwa_shortcut(&directory.0, "Profile1", "ace").is_none());
        assert_eq!(
            pwa_shortcut(&directory.0, "Profile1", "ABCDEF"),
            Some(app.join("a.LNK"))
        );
        assert!(pwa_shortcut(&directory.0, "..", "abcdef").is_none());
    }
}
