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
    pub(crate) expires: Instant,
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
}

pub(crate) struct IconCache {
    entries: HashMap<IconKey, Entry>,
    limit: usize,
    failure_ttl: Duration,
    budget: Arc<ByteBudget>,
    clock: u64,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) failures: u64,
    pub(crate) fallbacks: u64,
}

impl IconCache {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            entries: HashMap::new(),
            limit: config.icon_cache_limit as usize,
            failure_ttl: Duration::from_millis(config.icon_failure_ttl_ms.into()),
            budget: Arc::new(ByteBudget {
                limit: config.icon_cache_mb as usize * 1024 * 1024,
                used: AtomicUsize::new(0),
            }),
            clock: 0,
            hits: 0,
            misses: 0,
            failures: 0,
            fallbacks: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &IconKey) -> Option<Option<Arc<CachedIcon>>> {
        self.clock = self.clock.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(key).filter(|entry| {
            entry.expires > Instant::now() && entry.sources.iter().all(SourceStamp::unchanged)
        }) {
            entry.used = self.clock;
            self.hits += 1;
            return Some(entry.image.clone());
        }
        self.entries.remove(key);
        self.misses += 1;
        None
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
        let ttl = if source_succeeded && image.is_some() {
            Duration::from_secs(30)
        } else {
            self.failure_ttl
        };
        let expires = Instant::now() + ttl;
        let image = image.map(|(image, lease)| {
            assert_eq!(image.data.len(), lease.bytes);
            Arc::new(CachedIcon {
                image,
                revision: NEXT_REVISION.fetch_add(1, Ordering::Relaxed),
                expires,
                _lease: lease,
            })
        });
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
            expires: Instant::now() + Duration::from_secs(30),
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
            expires: Instant::now(),
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
        cache.insert(key(1), None, vec![SourceStamp::capture(&path)], false);
        assert!(matches!(cache.get(&key(1)), Some(None)));
        std::fs::write(&path, b"changed-length").unwrap();
        assert!(cache.get(&key(1)).is_none());
        for index in 1..=3 {
            cache.insert(key(index), None, Vec::new(), false);
        }
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key(1)).is_none());
        cache.entries.get_mut(&key(2)).unwrap().expires = Instant::now();
        assert!(cache.get(&key(2)).is_none());
        let lease = cache.reserve().unwrap();
        let image = cache
            .insert(
                key(4),
                Some((PixelImage::new(ICON_PIXELS, ICON_PIXELS).unwrap(), lease)),
                Vec::new(),
                true,
            )
            .unwrap();
        assert!(image.expires.duration_since(Instant::now()) > Duration::from_secs(29));
        cache.entries.clear();
        assert_eq!(cache.bytes(), ICON_BYTES);
        drop(image);
        assert_eq!(cache.bytes(), 0);
    }
}
