//! Snapshot-scoped text and optional phonetic indexes, owned only by the worker.
use super::{
    field::{IndexedField, Query},
    matcher,
    phonetic::{PhoneticIndex, MAX_QUERY_WORK},
    SearchEntry, SearchResults,
};
use crate::{
    app_name::NameCache,
    config::{Config, SearchField},
    picker::RowHighlights,
    window_snapshot::WindowSnapshot,
};
use anyhow::{ensure, Result};
use std::sync::Arc;

const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;
struct IndexedEntry {
    entry: SearchEntry,
    fields: Vec<IndexedField>,
}
pub(super) struct SearchIndex {
    source: Arc<WindowSnapshot>,
    entries: Vec<IndexedEntry>,
}

impl SearchIndex {
    pub fn uses(&self, source: &Arc<WindowSnapshot>) -> bool {
        Arc::ptr_eq(&self.source, source)
    }
    pub fn build(
        source: Arc<WindowSnapshot>,
        config: &Config,
        names: &mut NameCache,
        current: impl Fn() -> bool,
    ) -> Result<Option<Self>> {
        let mut entries = Vec::new();
        let mut bytes: usize = 0;
        for records in source.groups.values() {
            for record in records {
                if !current() {
                    return Ok(None);
                }
                let entry = SearchEntry {
                    identity: record.identity,
                    key: crate::icon_cache::IconKey {
                        group: record.application.icon_key.clone(),
                        identity: record.identity,
                    },
                    elevated: record.process.elevated,
                    minimized: record.minimized,
                    executable: record.process.executable.clone(),
                    title: Arc::from(record.title.as_str()),
                    app: record
                        .application
                        .name(&names.resolve(&record.application.icon_key)),
                    highlights: RowHighlights::default(),
                };
                let fields: Vec<_> = [SearchField::App, SearchField::Title, SearchField::Exe]
                    .into_iter()
                    .filter(|kind| config.search_fields.contains(*kind))
                    .map(|kind| IndexedField {
                        kind,
                        normalized: matcher::normalize(entry.field(kind)),
                        phonetic: None,
                    })
                    .collect();
                bytes = bytes.saturating_add(
                    fields.iter().map(|f| f.normalized.len()).sum::<usize>()
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
        // Reserve every original field first; phonetic data cannot evict raw text.
        if config.search_pinyin {
            let mut remaining = MAX_INDEX_BYTES - bytes;
            for entry in &mut entries {
                for field in &mut entry.fields {
                    if !current() {
                        return Ok(None);
                    }
                    field.phonetic = PhoneticIndex::build(
                        entry.entry.field(field.kind),
                        &mut remaining,
                        &current,
                    );
                }
            }
        }
        Ok(current().then_some(Self { source, entries }))
    }

    pub fn find(
        &self,
        value: &str,
        config: &Config,
        current: impl Fn() -> bool,
    ) -> Option<SearchResults> {
        let query = Query::new(value, config.search_match);
        let mut work = MAX_QUERY_WORK;
        let mut matches = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if !current() {
                return None;
            }
            if let Some(score) = entry
                .fields
                .iter()
                .filter_map(|field| query.score(field, &mut work, &current))
                .min()
            {
                matches.push((score, index));
            }
        }
        matches.sort_unstable();
        let total = matches.len();
        let mut entries = Vec::new();
        let mut highlight_work = MAX_QUERY_WORK;
        let original_query: Arc<str> = Arc::from(value);
        for (_, index) in matches.into_iter().take(config.search_max_results as usize) {
            if !current() {
                return None;
            }
            let indexed = &self.entries[index];
            let mut entry = indexed.entry.clone();
            let (kind, primary, secondary) = entry.display_text();
            let mut ranges = |kind, text: &str| {
                indexed
                    .fields
                    .iter()
                    .find(|field| field.kind == kind)
                    .map_or_else(Vec::new, |field| {
                        query.ranges(
                            text,
                            field.phonetic.is_some(),
                            &mut highlight_work,
                            &current,
                        )
                    })
            };
            entry.highlights = RowHighlights {
                query: original_query.clone(),
                primary: ranges(kind, &primary),
                secondary: ranges(SearchField::Exe, &secondary),
            };
            entries.push(entry);
        }
        current().then_some(SearchResults { total, entries })
    }
}

#[cfg(test)]
mod tests;
