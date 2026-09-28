use crate::{
    config::Config,
    icon_cache::{CachedIcon, IconKey, SourceStamp},
    utils::{
        appx,
        browser::{pwa_shortcut, BrowserPaths},
    },
    window_snapshot::lifetimes::WindowLifetimes,
    window_target::WindowTarget,
    worker::{self, Mailbox},
};
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::Foundation::HWND;

pub(crate) mod file;
pub(crate) mod native;
mod service;
pub(crate) const WM_ICON: u32 = 6011;

pub(crate) struct IconLoader {
    config: Config,
    browser: BrowserPaths,
}

impl IconLoader {
    pub(crate) fn new(config: &Config, ini_dir: &Path) -> Self {
        Self {
            config: config.clone(),
            browser: BrowserPaths::new(config, ini_dir),
        }
    }

    pub(crate) fn native(
        &self,
        group: &str,
        hwnd: HWND,
        allowed: impl Fn() -> bool,
    ) -> (Option<native::OwnedIcon>, Vec<SourceStamp>) {
        let deadline =
            Instant::now() + Duration::from_millis(self.config.icon_query_timeout_ms.into());
        let accepts = || Instant::now() < deadline && allowed();
        let (icon, sources) = self.query(group, hwnd, deadline, accepts);
        if !accepts() {
            debug!("icon stage=query expired-or-cancelled");
            return (None, sources);
        }
        (icon, sources)
    }

    fn query(
        &self,
        group: &str,
        hwnd: HWND,
        deadline: Instant,
        allowed: impl Fn() -> bool,
    ) -> (Option<native::OwnedIcon>, Vec<SourceStamp>) {
        let mut sources = Vec::new();
        let parts: Vec<_> = group.split("::").take(4).collect();
        let executable = parts[0];
        sources.push(SourceStamp::capture(Path::new(executable)));
        let lower = group.to_lowercase();
        if let Some((_, override_path)) = self
            .config
            .switch_apps_override_icons
            .iter()
            .find(|(pattern, _)| lower.contains(pattern.as_str()))
        {
            let path = PathBuf::from(override_path);
            let path = if path.is_absolute() {
                path
            } else {
                Path::new(executable)
                    .parent()
                    .unwrap_or(Path::new(""))
                    .join(path)
            };
            sources.push(SourceStamp::capture(&path));
            if !allowed() {
                return (None, sources);
            }
            if let Some(icon) = file::load(&path) {
                return (Some(icon), sources);
            }
            debug!("icon stage=override unavailable");
        }
        if !allowed() {
            return (None, sources);
        }
        if let [_, profile, app_id] = parts.as_slice() {
            let path = if *profile == "appx" {
                appx::package_directory(app_id).and_then(|directory| {
                    sources.push(SourceStamp::capture(&directory.join("AppxManifest.xml")));
                    appx::package_logo(&directory, None)
                })
            } else {
                self.browser
                    .root(executable)
                    .and_then(|root| pwa_shortcut(root, profile, app_id))
            };
            if let Some(path) = path {
                sources.push(SourceStamp::capture(&path));
                if !allowed() {
                    return (None, sources);
                }
                let icon = if *profile == "appx" {
                    file::load(&path)
                } else {
                    native::exe_icon(&path.to_string_lossy(), &allowed)
                };
                if icon.is_some() {
                    return (icon, sources);
                }
            }
        }
        if let [_, profile] = parts.as_slice() {
            if let Some(path) = self.browser.profile_icon(executable, profile) {
                sources.push(SourceStamp::capture(&path));
                if !allowed() {
                    return (None, sources);
                }
                if let Some(icon) = file::load(&path) {
                    return (Some(icon), sources);
                }
            }
        }
        if !allowed() {
            return (None, sources);
        }
        if let Some(path) = appx::executable_logo(Path::new(executable)) {
            sources.push(SourceStamp::capture(&path));
            if !allowed() {
                return (None, sources);
            }
            if let Some(icon) = file::load(&path) {
                return (Some(icon), sources);
            }
        }
        if let Some(icon) = native::exe_icon(executable, &allowed) {
            return (Some(icon), sources);
        }
        if !allowed() {
            return (None, sources);
        }
        (
            native::window_icon(hwnd, deadline.saturating_duration_since(Instant::now())),
            sources,
        )
    }
}

pub(crate) struct IconResult {
    pub(crate) key: IconKey,
    pub(crate) image: Option<Arc<CachedIcon>>,
    pub(crate) image_requested: bool,
    pub(crate) display_name: Arc<str>,
    pub(crate) complete: bool,
}
pub(crate) struct IconRequest {
    pub(crate) key: IconKey,
    pub(crate) image: bool,
}
pub(crate) struct IconService {
    mailbox: Arc<Mailbox<Vec<IconRequest>, IconResult>>,
    thread: Option<JoinHandle<()>>,
}

impl IconService {
    pub(crate) fn start(
        config: &Config,
        ini_dir: &Path,
        lifetimes: Arc<WindowLifetimes>,
        target: Arc<WindowTarget>,
    ) -> Result<Self> {
        let mailbox = Mailbox::<Vec<IconRequest>, IconResult>::new(16);
        let shared = mailbox.clone();
        let configuration = config.clone();
        let ini_dir = ini_dir.to_owned();
        let thread = thread::Builder::new()
            .name("icon-loader".into())
            .spawn(move || service::run(configuration, ini_dir, lifetimes, shared, target))
            .context("icon stage=thread-create")?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
        })
    }
    pub(crate) fn request(&self, keys: Vec<IconRequest>) -> u64 {
        self.mailbox.request(keys)
    }
    pub(crate) fn take(&self) -> Vec<(u64, IconResult)> {
        self.mailbox.take()
    }
    pub(crate) fn cancel(&self) {
        self.mailbox.cancel();
    }
}
impl Drop for IconService {
    fn drop(&mut self) {
        self.mailbox.close();
        worker::retire(self.thread.take(), "icons");
    }
}
