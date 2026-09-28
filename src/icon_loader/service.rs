use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use super::{native, IconLoader, IconRequest, IconResult, WM_ICON};
use crate::{
    config::Config,
    icon_cache::{IconCache, ICON_PIXELS},
    utils::com::ComApartment,
    window_snapshot::lifetimes::WindowLifetimes,
    window_target::WindowTarget,
    worker::Mailbox,
};

pub(super) fn run(
    configuration: Config,
    ini_dir: PathBuf,
    lifetimes: Arc<WindowLifetimes>,
    shared: Arc<Mailbox<Vec<IconRequest>, IconResult>>,
    target: Arc<WindowTarget>,
) {
    let _com = match ComApartment::sta() {
        Ok(com) => com,
        Err(error) => {
            error!("icon stage=com error={error:#}");
            shared.close();
            target.try_post(WM_ICON);
            return;
        }
    };
    let loader = IconLoader::new(&configuration, &ini_dir);
    let mut cache = IconCache::new(&configuration);
    let mut names = crate::app_name::NameCache::new(&configuration);
    let mut deduplicated = 0usize;
    while !shared.closed() && target.is_live() {
        let Some((generation, keys)) = shared.receive(Duration::from_secs(1)) else {
            continue;
        };
        let started = crate::diagnostics::sample_start(configuration.metrics_enabled);
        let mut published_names = 0usize;
        let mut seen = HashSet::new();
        for IconRequest {
            key,
            image: image_requested,
        } in keys
        {
            if !seen.insert(key.clone()) {
                deduplicated += 1;
                continue;
            }
            let allowed = || shared.current(generation) && target.is_live();
            if !allowed() {
                break;
            }
            if !key.identity.is_current(&lifetimes) {
                continue;
            }
            let display_name = names.resolve(&key.group);
            if !allowed() {
                break;
            }
            let image = if !image_requested {
                None
            } else if let Some(image) = cache.get(&key) {
                image
            } else {
                let previous = cache.previous(&key);
                if let Some(previous) = &previous {
                    if !shared.publish(
                        generation,
                        IconResult {
                            key: key.clone(),
                            image: Some(previous.image.clone()),
                            image_requested,
                            display_name: display_name.clone(),
                            complete: false,
                        },
                    ) {
                        break;
                    }
                    target.try_post(WM_ICON);
                }
                if let Some(lease) = cache.reserve() {
                    let (icon, sources) = loader.native(&key.group, key.identity.hwnd(), || {
                        allowed() && key.identity.is_current(&lifetimes)
                    });
                    if !allowed() {
                        break;
                    }
                    if !key.identity.is_current(&lifetimes) {
                        continue;
                    }
                    let source_succeeded = icon.is_some();
                    let image = icon.or_else(native::fallback).and_then(|icon| {
                        match native::rasterize(icon.0, ICON_PIXELS) {
                            Ok(image) => Some((image, lease)),
                            Err(error) => {
                                debug!("icon stage=rasterize error={error:#}");
                                None
                            }
                        }
                    });
                    if !allowed() {
                        break;
                    }
                    if !key.identity.is_current(&lifetimes) {
                        continue;
                    }
                    cache.insert(key.clone(), previous, image, sources, source_succeeded)
                } else {
                    debug!("icon stage=budget pinned-by-consumer");
                    cache.insert(key.clone(), previous, None, Vec::new(), false)
                }
            };
            if !allowed() || !key.identity.is_current(&lifetimes) {
                continue;
            }
            if !shared.publish(
                generation,
                IconResult {
                    key,
                    image,
                    image_requested,
                    display_name,
                    complete: true,
                },
            ) {
                break;
            }
            published_names += 1;
            target.try_post(WM_ICON);
        }
        crate::diagnostics::stage_elapsed("icons", started);
        if configuration.metrics_enabled {
            info!("metrics event=icon_cache entries={} bytes={} hits={} misses={} failures={} fallbacks={} stale_served={} refresh_unchanged={} refresh_changed={} deduplicated={deduplicated} names={published_names} workers=1", cache.len(), cache.bytes(), cache.hits, cache.misses, cache.failures, cache.fallbacks, cache.stale_served, cache.refresh_unchanged, cache.refresh_changed);
        }
    }
}
