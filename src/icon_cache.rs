use crate::{config::Config, pixels::PixelImage, utils::window_identity::WindowIdentity};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};

pub(crate) const ICON_PIXELS: i32 = 256;
pub(crate) const ICON_BYTES: usize = ICON_PIXELS as usize * ICON_PIXELS as usize * 4;
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct IconKey {
    pub(crate) group: Arc<str>,
    pub(crate) identity: WindowIdentity,
}

#[derive(Debug)]
struct ByteBudget {
    limit: usize,
    used: AtomicUsize,
}

#[derive(Debug)]
pub(crate) struct ByteLease {
    budget: Arc<ByteBudget>,
    bytes: usize,
}
impl Drop for ByteLease {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub(crate) struct CachedIcon {
    pub(crate) image: PixelImage,
    pub(crate) revision: u64,
    _lease: ByteLease,
}

#[derive(Clone, PartialEq, Eq)]
struct FileVersion {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}
#[derive(Clone)]
pub(crate) struct SourceStamp {
    path: PathBuf,
    version: Option<FileVersion>,
}
impl SourceStamp {
    pub(crate) fn capture(path: &Path) -> Self {
        let version = std::fs::metadata(path).ok().map(|metadata| FileVersion {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
        });
        Self {
            path: path.to_owned(),
            version,
        }
    }
    fn unchanged(&self) -> bool {
        Self::capture(&self.path).version == self.version
    }
}

struct Entry {
    image: Option<Arc<CachedIcon>>,
    sources: Vec<SourceStamp>,
    expires: Instant,
    used: u64,
    successful: bool,
}

pub(crate) struct PreviousIcon {
    pub(crate) image: Arc<CachedIcon>,
    successful: bool,
}

pub(crate) struct IconCache {
    entries: HashMap<IconKey, Entry>,
    limit: usize,
    failure_ttl: Duration,
    refresh_interval: Duration,
    budget: Arc<ByteBudget>,
    clock: u64,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) failures: u64,
    pub(crate) fallbacks: u64,
    pub(crate) stale_served: u64,
    pub(crate) refresh_unchanged: u64,
    pub(crate) refresh_changed: u64,
}

impl IconCache {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            entries: HashMap::new(),
            limit: config.icon_cache_limit as usize,
            failure_ttl: Duration::from_millis(config.icon_failure_ttl_ms.into()),
            refresh_interval: Duration::from_secs(config.icon_refresh_interval_s.into()),
            budget: Arc::new(ByteBudget {
                limit: config.icon_cache_mb as usize * 1024 * 1024,
                used: AtomicUsize::new(0),
            }),
            clock: 0,
            hits: 0,
            misses: 0,
            failures: 0,
            fallbacks: 0,
            stale_served: 0,
            refresh_unchanged: 0,
            refresh_changed: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &IconKey) -> Option<Option<Arc<CachedIcon>>> {
        self.get_at(key, Instant::now())
    }

    fn get_at(&mut self, key: &IconKey, now: Instant) -> Option<Option<Arc<CachedIcon>>> {
        self.clock = self.clock.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.used = self.clock;
            if entry.expires > now && entry.sources.iter().all(SourceStamp::unchanged) {
                self.hits += 1;
                return Some(entry.image.clone());
            }
        }
        self.misses += 1;
        None
    }

    pub(crate) fn previous(&mut self, key: &IconKey) -> Option<PreviousIcon> {
        let entry = self.entries.get(key)?;
        let image = entry.image.clone()?;
        self.stale_served += 1;
        Some(PreviousIcon {
            image,
            successful: entry.successful,
        })
    }

    pub(crate) fn reserve(&mut self) -> Option<ByteLease> {
        loop {
            if self
                .budget
                .used
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |bytes| {
                    bytes
                        .checked_add(ICON_BYTES)
                        .filter(|total| *total <= self.budget.limit)
                })
                .is_ok()
            {
                return Some(ByteLease {
                    budget: self.budget.clone(),
                    bytes: ICON_BYTES,
                });
            }
            if !self.evict() {
                return None;
            }
        }
    }

    fn evict(&mut self) -> bool {
        if let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.used)
            .map(|(key, _)| key.clone())
        {
            self.entries.remove(&key);
            true
        } else {
            false
        }
    }

    pub(crate) fn insert(
        &mut self,
        key: IconKey,
        previous: Option<PreviousIcon>,
        image: Option<(PixelImage, ByteLease)>,
        sources: Vec<SourceStamp>,
        source_succeeded: bool,
    ) -> Option<Arc<CachedIcon>> {
        if !source_succeeded || image.is_none() {
            self.failures += 1;
        }
        if !source_succeeded && image.is_some() {
            self.fallbacks += 1;
        }
        let loaded = source_succeeded && image.is_some();
        let ttl = if loaded {
            self.refresh_interval
        } else {
            self.failure_ttl
        };
        let expires = Instant::now() + ttl;
        let same_pixels = previous.as_ref().is_some_and(|previous| {
            image.as_ref().is_some_and(|(image, _)| {
                image.width == previous.image.image.width
                    && image.height == previous.image.image.height
                    && image.data == previous.image.image.data
            })
        });
        let keep_previous = previous.as_ref().is_some_and(|previous| {
            image.is_none() || (previous.successful && !loaded) || same_pixels
        });
        if previous.is_some() && loaded {
            if same_pixels {
                self.refresh_unchanged += 1;
            } else {
                self.refresh_changed += 1;
            }
        }
        let successful =
            loaded || (keep_previous && previous.as_ref().is_some_and(|old| old.successful));
        let image = if keep_previous {
            // Dropping the unused raster also returns its reserved byte lease.
            previous.map(|previous| previous.image)
        } else {
            image.map(|(image, lease)| {
                assert_eq!(image.data.len(), lease.bytes);
                Arc::new(CachedIcon {
                    image,
                    revision: NEXT_REVISION.fetch_add(1, Ordering::Relaxed),
                    _lease: lease,
                })
            })
        };
        while self.entries.len() >= self.limit && !self.entries.contains_key(&key) {
            if !self.evict() {
                break;
            }
        }
        self.clock = self.clock.wrapping_add(1);
        self.entries.insert(
            key,
            Entry {
                image: image.clone(),
                sources,
                expires,
                used: self.clock,
                successful,
            },
        );
        image
    }

    pub(crate) fn bytes(&self) -> usize {
        self.budget.used.load(Ordering::Acquire)
    }
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn fixture(image: PixelImage) -> Arc<CachedIcon> {
        let bytes = image.data.len();
        Arc::new(CachedIcon {
            image,
            revision: NEXT_REVISION.fetch_add(1, Ordering::Relaxed),
            _lease: ByteLease {
                bytes,
                budget: Arc::new(ByteBudget {
                    limit: bytes,
                    used: AtomicUsize::new(bytes),
                }),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_revalidation_preserves_pixels_revision_and_last_good_image_on_failure() {
        let key = IconKey {
            group: "stale-fixture".into(),
            identity: WindowIdentity::fixture(1),
        };
        let mut cache = IconCache::new(&Config::default());
        let lease = cache.reserve().unwrap();
        let original = cache
            .insert(
                key.clone(),
                None,
                Some((PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap(), lease)),
                Vec::new(),
                true,
            )
            .unwrap();
        assert!(cache
            .get_at(&key, Instant::now() + Duration::from_secs(31))
            .is_some());
        let after_refresh = cache.entries[&key].expires + Duration::from_millis(1);
        assert!(cache.get_at(&key, after_refresh).is_none());
        let previous = cache.previous(&key);
        assert!(Arc::ptr_eq(&previous.as_ref().unwrap().image, &original));
        let lease = cache.reserve().unwrap();
        let same = cache
            .insert(
                key.clone(),
                previous,
                Some((PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap(), lease)),
                Vec::new(),
                true,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&same, &original));
        assert_eq!(cache.refresh_unchanged, 1);
        assert_eq!(cache.bytes(), ICON_BYTES);
        let previous = cache.previous(&key);
        let lease = cache.reserve().unwrap();
        let mut fallback = PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap();
        fallback.data[0] = 99;
        let retained = cache
            .insert(
                key.clone(),
                previous,
                Some((fallback, lease)),
                Vec::new(),
                false,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&retained, &original));
        assert_eq!(cache.bytes(), ICON_BYTES);
        assert!(cache.entries[&key].successful);
        let previous = cache.previous(&key);
        let lease = cache.reserve().unwrap();
        let mut changed = PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap();
        changed.data[0] = 77;
        let changed = cache
            .insert(key, previous, Some((changed, lease)), Vec::new(), true)
            .unwrap();
        assert_ne!(changed.revision, original.revision);
        assert_eq!(changed.image.data[0], 77);
        assert_eq!(cache.refresh_changed, 1);
    }

    #[test]
    fn pinned_budget_does_not_replace_a_successful_icon_with_a_placeholder() {
        let key = IconKey {
            group: "pinned-fixture".into(),
            identity: WindowIdentity::fixture(2),
        };
        let mut cache = IconCache::new(&Config::default());
        cache.budget = Arc::new(ByteBudget {
            limit: ICON_BYTES,
            used: AtomicUsize::new(0),
        });
        let lease = cache.reserve().unwrap();
        let original = cache
            .insert(
                key.clone(),
                None,
                Some((PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap(), lease)),
                Vec::new(),
                true,
            )
            .unwrap();
        let previous = cache.previous(&key);
        assert!(cache.reserve().is_none());
        assert!(cache.entries.is_empty());
        let retained = cache
            .insert(key.clone(), previous, None, Vec::new(), false)
            .unwrap();
        assert!(Arc::ptr_eq(&retained, &original));
        assert_eq!(cache.bytes(), ICON_BYTES);
        assert_eq!(
            cache.get(&key).flatten().unwrap().revision,
            original.revision
        );
    }

    #[test]
    fn ui_references_continue_to_count_after_eviction() {
        let config = Config::default();
        let mut cache = IconCache::new(&config);
        cache.budget = Arc::new(ByteBudget {
            limit: ICON_BYTES,
            used: AtomicUsize::new(0),
        });
        let lease = cache.reserve().unwrap();
        let icon = Arc::new(CachedIcon {
            image: PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap(),
            revision: 1,
            _lease: lease,
        });
        let ui = icon.clone();
        drop(icon);
        assert_eq!(cache.bytes(), ICON_BYTES);
        assert!(cache.reserve().is_none());
        drop(ui);
        assert_eq!(cache.bytes(), 0);
        assert!(cache.reserve().is_some());
    }

    #[test]
    fn success_and_failure_entries_are_bounded_expire_and_check_source_changes() {
        let directory = crate::config::test_support::TestDirectory::new();
        let path = directory.0.join("source.ico");
        std::fs::write(&path, b"before").unwrap();
        let key = |index| IconKey {
            group: format!("fixture-{index}").into(),
            identity: WindowIdentity::fixture(index),
        };
        let mut cache = IconCache::new(&Config::default());
        cache.limit = 2;
        cache.insert(key(1), None, None, vec![SourceStamp::capture(&path)], false);
        assert!(matches!(cache.get(&key(1)), Some(None)));
        std::fs::write(&path, b"changed-length").unwrap();
        assert!(cache.get(&key(1)).is_none());
        for index in 1..=3 {
            cache.insert(key(index), None, None, Vec::new(), false);
        }
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key(1)).is_none());
        cache.entries.get_mut(&key(2)).unwrap().expires = Instant::now();
        assert!(cache.get(&key(2)).is_none());
        let lease = cache.reserve().unwrap();
        let image = cache
            .insert(
                key(4),
                None,
                Some((PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap(), lease)),
                Vec::new(),
                true,
            )
            .unwrap();
        assert!(
            cache.entries[&key(4)]
                .expires
                .duration_since(Instant::now())
                > Duration::from_secs(299)
        );
        cache.entries.clear();
        assert_eq!(cache.bytes(), ICON_BYTES);
        drop(image);
        assert_eq!(cache.bytes(), 0);
    }
}
