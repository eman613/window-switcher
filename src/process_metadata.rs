//! Process image/elevation cache. PID reuse is checked before every cache hit.
use crate::{
    config::Config,
    utils::{is_process_elevated, HandleWrapper},
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, FILETIME},
        System::Threading::{
            GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) created: u64,
}

pub(crate) fn open_identity(pid: u32) -> Option<(HandleWrapper, ProcessIdentity)> {
    if pid == 0 {
        return None;
    }
    let handle = HandleWrapper::new(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?,
    );
    let (mut created, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(
            handle.get_handle(),
            &mut created,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    Some((
        handle,
        ProcessIdentity {
            pid,
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        },
    ))
}

pub(crate) fn image_path(handle: &HandleWrapper) -> Option<String> {
    let mut capacity = 260;
    loop {
        let mut name = vec![0u16; capacity];
        let mut length = capacity as u32;
        match unsafe {
            QueryFullProcessImageNameW(
                handle.get_handle(),
                PROCESS_NAME_WIN32,
                PWSTR(name.as_mut_ptr()),
                &mut length,
            )
        } {
            Ok(()) if length > 0 && length as usize <= capacity => {
                return String::from_utf16(&name[..length as usize]).ok()
            }
            Err(error)
                if error.code()
                    == windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
                    && capacity < 32768 =>
            {
                capacity = (capacity * 2).min(32768)
            }
            _ => return None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessMetadata {
    pub(crate) identity: ProcessIdentity,
    pub(crate) path: Arc<str>,
    pub(crate) executable: Arc<str>,
    pub(crate) elevated: Option<bool>,
}

struct Entry {
    value: Option<ProcessMetadata>,
    expires: Instant,
    used: u64,
    bytes: usize,
}
const TEXT_LIMIT: usize = 8 * 1024 * 1024;

pub(crate) struct ProcessMetadataCache {
    entries: HashMap<ProcessIdentity, Entry>,
    limit: usize,
    ttl: Duration,
    clock: u64,
    text_bytes: usize,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
}

impl ProcessMetadataCache {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            entries: HashMap::new(),
            limit: config.metadata_cache_limit as usize,
            ttl: Duration::from_millis(config.metadata_ttl_ms.into()),
            clock: 0,
            text_bytes: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub(crate) fn lookup(&mut self, pid: u32) -> Option<ProcessMetadata> {
        let (handle, identity) = open_identity(pid)?;
        let now = Instant::now();
        self.clock = self.clock.wrapping_add(1);
        if let Some(entry) = self
            .entries
            .get_mut(&identity)
            .filter(|entry| entry.expires > now)
        {
            entry.used = self.clock;
            self.hits += 1;
            return entry.value.clone();
        }
        self.misses += 1;
        let value = image_path(&handle).map(|path| ProcessMetadata {
            identity,
            executable: path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or_default()
                .to_lowercase()
                .into(),
            path: path.into(),
            elevated: is_process_elevated(pid),
        });
        self.insert(identity, value.clone(), now);
        value
    }

    fn insert(&mut self, identity: ProcessIdentity, value: Option<ProcessMetadata>, now: Instant) {
        self.entries.retain(|key, entry| {
            let keep = entry.expires > now && key.pid != identity.pid;
            if !keep {
                self.text_bytes -= entry.bytes;
            }
            keep
        });
        let bytes = value.as_ref().map_or(0, |metadata| {
            metadata.path.len() + metadata.executable.len()
        });
        if bytes > TEXT_LIMIT {
            return;
        }
        while self.entries.len() >= self.limit || self.text_bytes + bytes > TEXT_LIMIT {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.text_bytes -= self.entries.remove(&oldest).unwrap().bytes;
        }
        self.text_bytes += bytes;
        self.entries.insert(
            identity,
            Entry {
                value,
                expires: now + self.ttl,
                used: self.clock,
                bytes,
            },
        );
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_is_bounded_expires_and_rejects_pid_reuse() {
        let mut cache = ProcessMetadataCache::new(&Config::default());
        cache.limit = 2;
        let now = Instant::now();
        for pid in 1..=3 {
            cache.clock += 1;
            cache.insert(ProcessIdentity { pid, created: 1 }, None, now);
        }
        assert_eq!(cache.len(), 2);
        assert!(!cache
            .entries
            .contains_key(&ProcessIdentity { pid: 1, created: 1 }));
        cache.insert(ProcessIdentity { pid: 2, created: 2 }, None, now);
        assert!(!cache
            .entries
            .contains_key(&ProcessIdentity { pid: 2, created: 1 }));
        cache.insert(
            ProcessIdentity { pid: 4, created: 1 },
            None,
            now + cache.ttl,
        );
        assert_eq!(cache.len(), 1);
        let pid = std::process::id();
        assert!(cache.lookup(pid).is_some());
        assert!(cache.lookup(pid).is_some());
        assert_eq!(cache.hits, 1);
        assert!(cache.lookup(0).is_none());
    }

    #[test]
    fn long_process_paths_cannot_exceed_the_text_budget() {
        let config = Config {
            metadata_cache_limit: 4096,
            ..Default::default()
        };
        let mut cache = ProcessMetadataCache::new(&config);
        let now = Instant::now();
        for pid in 1..=300 {
            let identity = ProcessIdentity { pid, created: 1 };
            cache.clock += 1;
            cache.insert(
                identity,
                Some(ProcessMetadata {
                    identity,
                    path: "x".repeat(32768).into(),
                    executable: "fixture.exe".into(),
                    elevated: Some(false),
                }),
                now,
            );
            assert!(cache.text_bytes <= TEXT_LIMIT);
        }
        assert!(cache.len() < 300);
        assert!(!cache
            .entries
            .contains_key(&ProcessIdentity { pid: 1, created: 1 }));
    }
}
