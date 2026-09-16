use crate::{
    config::Config,
    icon_cache::{CachedIcon, IconCache, IconKey, SourceStamp, ICON_PIXELS},
    utils::{
        appx,
        browser::{pwa_shortcut, BrowserPaths},
        com::ComApartment,
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
        let thread = thread::Builder::new().name("icon-loader".into()).spawn(move || {
            let _com = match ComApartment::sta() { Ok(com) => com, Err(error) => { error!("icon stage=com error={error:#}"); shared.close(); target.try_post(WM_ICON); return; } };
            let loader = IconLoader::new(&configuration, &ini_dir);
            let mut cache = IconCache::new(&configuration);
            let mut names = crate::app_name::NameCache::new(&configuration);
            while !shared.closed() && target.is_live() {
                let Some((generation, keys)) = shared.receive(Duration::from_secs(1)) else { continue; };
                let started = crate::diagnostics::sample_start(configuration.metrics_enabled);
                let mut published_names = 0usize;
                for IconRequest { key, image: image_requested } in keys {
                    let allowed = || shared.current(generation) && target.is_live();
                    if !allowed() { break; }
                    if !key.identity.is_current(&lifetimes) { continue; }
                    let image = if !image_requested { None } else if let Some(image) = cache.get(&key) { image } else if let Some(lease) = cache.reserve() {
                        let (icon, sources) = loader.native(&key.group, key.identity.hwnd(), || allowed() && key.identity.is_current(&lifetimes));
                        if !allowed() { break; }
                        if key.identity.is_current(&lifetimes) {
                            let source_succeeded = icon.is_some();
                            let icon = icon.or_else(native::fallback);
                            let image = icon.and_then(|icon| match native::rasterize(icon.0, ICON_PIXELS) {
                                Ok(image) => Some((image, lease)),
                                Err(error) => { debug!("icon stage=rasterize error={error:#}"); None }
                            });
                            cache.insert(key.clone(), image, sources, source_succeeded)
                        } else { None }
                    } else {
                        debug!("icon stage=budget pinned-by-consumer");
                        cache.insert(key.clone(), None, Vec::new(), false)
                    };
                    if !allowed() { break; }
                    let display_name = names.resolve(&key.group);
                    if !allowed() || !key.identity.is_current(&lifetimes) { continue; }
                    if !shared.publish(generation, IconResult { key, image, image_requested, display_name }) { break; }
                    published_names += 1;
                    target.try_post(WM_ICON);
                }
                crate::diagnostics::stage_elapsed("icons", started);
                if configuration.metrics_enabled { info!("metrics event=icon_cache entries={} bytes={} hits={} misses={} failures={} fallbacks={} names={published_names} workers=1", cache.len(), cache.bytes(), cache.hits, cache.misses, cache.failures, cache.fallbacks); }
            }
        }).context("icon stage=thread-create")?;
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
