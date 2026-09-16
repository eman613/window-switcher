//! AUMID profile matching reads only the configured root's direct directories.
use crate::{
    config::Config,
    utils::browser::{self, BrowserPaths},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

struct Catalog {
    root: PathBuf,
    root_token: String,
    is_default: bool,
    profiles: Vec<String>,
}

struct CachedCatalog {
    expires: Instant,
    value: Option<Catalog>,
}

pub(super) struct ProfileResolver {
    paths: BrowserPaths,
    ttl: Duration,
    catalogs: HashMap<PathBuf, CachedCatalog>,
}

fn token(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '.')
        .collect()
}

impl Catalog {
    fn directory(&self, profile: &str) -> Option<PathBuf> {
        let directory = self.root.join(profile).canonicalize().ok()?;
        (directory.parent() == Some(self.root.as_path())).then_some(directory)
    }
    fn read(root: &Path, default: Option<&Path>) -> Option<Self> {
        let canonical = root.canonicalize().ok()?;
        let root_token = token(root.file_name()?.to_str()?);
        if root_token.is_empty() {
            return None;
        }
        let mut profiles = Vec::new();
        for path in browser::sorted_directory(root)? {
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if browser::profile_directory(name).is_some() && !token(name).is_empty() {
                    profiles.push(name.to_owned());
                }
            }
        }
        let is_default = default
            .and_then(|path| path.canonicalize().ok())
            .is_some_and(|path| path == canonical);
        Some(Self {
            root: canonical,
            root_token,
            is_default,
            profiles,
        })
    }

    fn profile(&self, aumid: &str, prefix: &str) -> Option<&str> {
        let suffix = aumid.strip_prefix(prefix)?;
        let suffix = if let Some(app) = suffix.strip_prefix("._crx_") {
            let end = app.find('.').unwrap_or(app.len());
            let id = &app[..end];
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            &app[end..]
        } else {
            suffix
        };
        let wanted = if suffix.is_empty() && self.is_default {
            "Default"
        } else {
            suffix
                .strip_prefix('.')?
                .strip_prefix(&self.root_token)?
                .strip_prefix('.')?
        };
        let mut matches = self.profiles.iter().filter(|name| token(name) == wanted);
        let first = matches.next()?;
        matches.next().is_none().then_some(first.as_str())
    }
}

impl ProfileResolver {
    pub(super) fn new(config: &Config, ini_dir: &Path) -> Self {
        Self {
            paths: BrowserPaths::new(config, ini_dir),
            ttl: Duration::from_millis(config.metadata_ttl_ms.into()),
            catalogs: HashMap::new(),
        }
    }

    pub(super) fn resolve(
        &mut self,
        executable: &str,
        aumid: &str,
    ) -> Option<(Arc<str>, Arc<str>)> {
        let prefix = match browser::exe_name(executable).to_ascii_lowercase().as_str() {
            "chrome.exe" => "Chrome",
            "msedge.exe" => "MSEdge",
            _ => return None,
        };
        let root = self.paths.root(executable)?.to_path_buf();
        let now = Instant::now();
        if self
            .catalogs
            .get(&root)
            .is_none_or(|entry| entry.expires <= now)
        {
            let value = Catalog::read(&root, self.paths.default_root(executable));
            if value.is_none() {
                warn!("grouping stage=profile-directory unavailable; app-id fallback");
            }
            self.catalogs.insert(
                root.clone(),
                CachedCatalog {
                    expires: now + self.ttl,
                    value,
                },
            );
        }
        let catalog = self.catalogs.get(&root)?.value.as_ref()?;
        let Some(profile) = catalog.profile(aumid, prefix) else {
            debug!("grouping stage=profile identity-unavailable-or-ambiguous; app-id fallback");
            return None;
        };
        // Reject junctions/reparse targets that leave the configured direct root.
        let Some(directory) = catalog.directory(profile) else {
            warn!("grouping stage=profile-directory missing-or-outside-root; app-id fallback");
            return None;
        };
        Some((
            format!(
                "profile|{}|{}",
                executable.to_lowercase(),
                directory.to_string_lossy().to_lowercase()
            )
            .into(),
            Arc::from(profile),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_tokens_follow_chromium_and_ambiguous_names_never_guess() {
        let mut catalog = Catalog {
            root: PathBuf::new(),
            root_token: "UserData".into(),
            is_default: true,
            profiles: vec!["Default".into(), "Profile 1".into(), "Work.2".into()],
        };
        assert_eq!(catalog.profile("Chrome", "Chrome"), Some("Default"));
        assert_eq!(
            catalog.profile("MSEdge.UserData.Profile1", "MSEdge"),
            Some("Profile 1")
        );
        assert_eq!(
            catalog.profile("Chrome._crx_abc123.UserData.Work.2", "Chrome"),
            Some("Work.2")
        );
        assert!(catalog.profile("Chrome.Other.Profile1", "Chrome").is_none());
        assert!(catalog.profile("Chrome.UserData...", "Chrome").is_none());
        catalog.profiles.push("Profile1".into());
        assert!(catalog
            .profile("Chrome.UserData.Profile1", "Chrome")
            .is_none());
        catalog.is_default = false;
        assert!(catalog.profile("Chrome", "Chrome").is_none());
    }

    #[test]
    fn only_configured_directories_supply_profile_identity() {
        let directory = crate::config::test_support::TestDirectory::new();
        let root = directory.0.join("Custom Data");
        std::fs::create_dir_all(root.join("Profile 7")).unwrap();
        let config = Config {
            chrome_user_data_dir: Some(PathBuf::from("Custom Data")),
            edge_user_data_dir: Some(PathBuf::from("missing-edge")),
            ..Default::default()
        };
        let mut resolver = ProfileResolver::new(&config, &directory.0);
        let found = resolver
            .resolve(r"Q:\chrome.exe", "Chrome.CustomData.Profile7")
            .unwrap();
        assert_eq!(&*found.1, "Profile 7");
        assert!(found.0.contains("profile 7"));
        assert!(resolver
            .resolve(r"Q:\chrome.exe", "Chrome.UserData.Profile7")
            .is_none());
        assert!(resolver
            .resolve(r"Q:\other.exe", "Chrome.CustomData.Profile7")
            .is_none());
        assert!(resolver
            .resolve(r"Q:\msedge.exe", "MSEdge.CustomData.Profile7")
            .is_none());
    }

    #[test]
    fn catalog_accepts_default_only_at_the_default_root_and_rejects_escape_or_missing() {
        let directory = crate::config::test_support::TestDirectory::new();
        let root = directory.0.join("User Data");
        std::fs::create_dir_all(root.join("Default")).unwrap();
        std::fs::create_dir_all(directory.0.join("Outside")).unwrap();
        let catalog = Catalog::read(&root, Some(&root)).unwrap();
        assert_eq!(catalog.profile("Chrome", "Chrome"), Some("Default"));
        assert_eq!(
            catalog.profile("MSEdge._crx_abcdef", "MSEdge"),
            Some("Default")
        );
        assert!(catalog.directory("Default").is_some());
        assert!(catalog.directory("Missing").is_none());
        assert!(catalog.directory("../Outside").is_none());
        assert!(Catalog::read(&root, None)
            .unwrap()
            .profile("Chrome", "Chrome")
            .is_none());
        let file = directory.0.join("not-a-directory");
        std::fs::write(&file, b"fixture").unwrap();
        assert!(Catalog::read(&file, None).is_none());
    }
}
