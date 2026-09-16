use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{ensure, Context, Result};

use super::{matcher, SearchEntry, SearchResults, MAX_QUERY_UNITS};
use crate::{
    app_name::NameCache,
    config::{Config, SearchField},
    keyboard::dispatch::WM_INPUT_READY,
    utils::com::ComApartment,
    window_snapshot::WindowSnapshot,
    window_target::WindowTarget,
    worker::{self, Mailbox},
};

const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;

struct Request {
    source: Arc<WindowSnapshot>,
    query: String,
}

struct IndexedEntry {
    entry: SearchEntry,
    fields: Vec<String>,
}

struct SearchIndex {
    source: Arc<WindowSnapshot>,
    entries: Vec<IndexedEntry>,
}

impl SearchIndex {
    fn build(
        source: Arc<WindowSnapshot>,
        config: &Config,
        names: &mut NameCache,
        current: impl Fn() -> bool,
    ) -> Result<Option<Self>> {
        let mut entries = Vec::new();
        let mut bytes: usize = 0;
        for (group, records) in &source.groups {
            if !current() {
                return Ok(None);
            }
            let app = names.resolve(group);
            for record in records {
                if !current() {
                    return Ok(None);
                }
                let entry = SearchEntry {
                    identity: record.identity,
                    executable: record.process.executable.clone(),
                    title: Arc::from(record.title.as_str()),
                    app: app.clone(),
                };
                let fields: Vec<String> = [
                    (SearchField::App, entry.app.as_ref()),
                    (SearchField::Title, entry.title.as_ref()),
                    (SearchField::Exe, entry.executable.as_ref()),
                ]
                .into_iter()
                .filter(|(field, _)| config.search_fields.contains(*field))
                .map(|(_, value)| matcher::normalize(value))
                .collect();
                bytes = bytes.saturating_add(
                    fields.iter().map(String::len).sum::<usize>()
                        + entry.title.len()
                        + entry.app.len()
                        + entry.executable.len(),
                );
                ensure!(
                    bytes <= MAX_INDEX_BYTES && entries.len() < crate::layout::MAX_WINDOWS,
                    "search stage=index text-or-window-budget exceeded"
                );
                entries.push(IndexedEntry { entry, fields });
            }
        }
        Ok(Some(Self { source, entries }))
    }

    fn find(
        &self,
        query: &str,
        config: &Config,
        current: impl Fn() -> bool,
    ) -> Option<SearchResults> {
        let query = matcher::normalize(query.trim());
        let mut matches = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if !current() {
                return None;
            }
            if let Some(score) = entry
                .fields
                .iter()
                .filter_map(|field| matcher::score(field, &query, config.search_match))
                .min()
            {
                matches.push((score, index));
            }
        }
        matches.sort_unstable();
        Some(SearchResults {
            total: matches.len(),
            entries: matches
                .into_iter()
                .take(config.search_max_results as usize)
                .map(|(_, index)| self.entries[index].entry.clone())
                .collect(),
        })
    }
}

pub(super) struct SearchService {
    mailbox: Arc<Mailbox<Request, Result<SearchResults>>>,
    thread: Option<JoinHandle<()>>,
}

impl SearchService {
    pub(super) fn start(config: &Config, target: Arc<WindowTarget>) -> Result<Self> {
        let mailbox = Mailbox::<Request, Result<SearchResults>>::new(1);
        let shared = mailbox.clone();
        let config = config.clone();
        let thread = thread::Builder::new()
            .name("window-search".into())
            .spawn(move || {
                let _com = match ComApartment::sta() {
                    Ok(com) => com,
                    Err(error) => {
                        error!("search stage=com error={error:#}");
                        shared.close();
                        target.try_post(WM_INPUT_READY);
                        return;
                    }
                };
                let mut names = NameCache::new(&config);
                let mut index: Option<SearchIndex> = None;
                let mut last_generation = 0;
                while !shared.closed() && target.is_live() {
                    let Some((generation, request)) = shared.receive(Duration::from_millis(50))
                    else {
                        if !shared.current(last_generation) {
                            index = None;
                        }
                        continue;
                    };
                    last_generation = generation;
                    let current = || shared.current(generation) && target.is_live();
                    let result = (|| -> Result<Option<SearchResults>> {
                        ensure!(
                            request.query.encode_utf16().count() <= MAX_QUERY_UNITS,
                            "search stage=query length-limit exceeded"
                        );
                        if index
                            .as_ref()
                            .is_none_or(|index| !Arc::ptr_eq(&index.source, &request.source))
                        {
                            index =
                                SearchIndex::build(request.source, &config, &mut names, current)?;
                        }
                        Ok(index
                            .as_ref()
                            .and_then(|index| index.find(&request.query, &config, current)))
                    })();
                    let output = match result {
                        Ok(Some(results)) => Ok(results),
                        Ok(None) => continue,
                        Err(error) => Err(error),
                    };
                    if shared.publish(generation, output) {
                        target.try_post(WM_INPUT_READY);
                    }
                }
            })
            .context("search stage=thread-create")?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
        })
    }

    pub(super) fn request(&self, source: Arc<WindowSnapshot>, query: String) -> u64 {
        self.mailbox.request(Request { source, query })
    }
    pub(super) fn cancel(&self) {
        self.mailbox.cancel();
    }
    pub(super) fn take(&self) -> Vec<(u64, Result<SearchResults>)> {
        self.mailbox.take()
    }
    pub(super) fn healthy(&self) -> bool {
        !self.mailbox.closed()
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for SearchService {
    fn drop(&mut self) {
        self.mailbox.close();
        worker::retire(self.thread.take(), "search");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::window_identity::WindowIdentity;

    fn index(count: usize) -> SearchIndex {
        SearchIndex {
            source: Arc::new(WindowSnapshot {
                groups: Default::default(),
                revision: 1,
            }),
            entries: (0..count)
                .map(|n| IndexedEntry {
                    entry: SearchEntry {
                        identity: WindowIdentity::fixture(n + 1),
                        executable: Arc::from("fixture.exe"),
                        title: Arc::from(format!("窗口 {n}")),
                        app: Arc::from("应用"),
                    },
                    fields: vec![format!("窗口 {n}")],
                })
                .collect(),
        }
    }

    #[test]
    fn empty_query_is_ordered_bounded_and_cancelable() {
        let index = index(210);
        let config = Config::default();
        let result = index.find("  ", &config, || true).unwrap();
        assert_eq!((result.total, result.entries.len()), (210, 50));
        assert_eq!(result.entries[49].identity.window, 50);
        assert!(index.find("窗口", &config, || false).is_none());
        assert!(index
            .find("no results", &config, || true)
            .unwrap()
            .entries
            .is_empty());
    }
}
