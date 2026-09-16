//! Search owns a native text surface and one cancellable offline matcher.
mod controls;
mod matcher;
mod messages;
mod service;
mod window;

use crate::{
    config::Config, layout::MonitorSnapshot, localization::Text,
    utils::window_identity::WindowIdentity, window_snapshot::WindowSnapshot,
    window_target::WindowTarget,
};
use anyhow::Result;
use std::sync::Arc;
use windows::Win32::Foundation::HWND;

pub(crate) const MAX_QUERY_UNITS: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct SearchEntry {
    pub(crate) identity: WindowIdentity,
    pub(crate) executable: Arc<str>,
    title: Arc<str>,
    app: Arc<str>,
}

impl SearchEntry {
    fn label(&self) -> String {
        format!("{} — {}", self.title, self.app)
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect()
    }
}

struct SearchResults {
    entries: Vec<SearchEntry>,
    total: usize,
}

pub(crate) enum SearchAction {
    Cancel,
    Activate(SearchEntry),
}

pub(crate) struct SearchSession {
    window: window::SearchWindow,
    service: service::SearchService,
    source: Option<Arc<WindowSnapshot>>,
    results: Vec<SearchEntry>,
    selected: Option<WindowIdentity>,
    generation: u64,
    pending: bool,
    query: String,
    failure_shown: bool,
}

impl SearchSession {
    pub(crate) fn new(
        owner: HWND,
        config: &Config,
        target: Arc<WindowTarget>,
        text: Text,
    ) -> Result<Self> {
        Ok(Self {
            window: window::SearchWindow::create(owner, target.clone(), text)?,
            service: service::SearchService::start(config, target)?,
            source: None,
            results: Vec::new(),
            selected: None,
            generation: 0,
            pending: false,
            query: String::new(),
            failure_shown: false,
        })
    }
    pub(crate) fn hwnd(&self) -> HWND {
        self.window.hwnd
    }
    pub(crate) fn active(&self) -> bool {
        self.window.visible()
    }
    pub(crate) fn revision(&self) -> Option<u64> {
        self.source.as_ref().map(|source| source.revision)
    }
    pub(crate) fn open(&mut self, config: &Config, monitor: MonitorSnapshot) -> Result<()> {
        if self.active() {
            return self.window.focus();
        }
        self.close();
        self.pending = true;
        self.window.show(config, monitor)
    }
    pub(crate) fn reposition(&mut self, config: &Config, monitor: MonitorSnapshot) -> Result<()> {
        self.window.position(config, monitor)
    }
    pub(crate) fn close(&mut self) {
        self.window.hide();
        self.service.cancel();
        self.source = None;
        self.results.clear();
        self.selected = None;
        self.query.clear();
        self.pending = false;
        self.failure_shown = false;
    }
    pub(crate) fn snapshot(&mut self, source: WindowSnapshot) -> Result<()> {
        self.remember_selection();
        self.source = Some(Arc::new(source));
        self.request()
    }
    fn remember_selection(&mut self) {
        if !self.pending {
            self.selected = self
                .window
                .selected()
                .and_then(|index| self.results.get(index))
                .map(|entry| entry.identity);
        }
    }
    fn request(&mut self) -> Result<()> {
        self.pending = true;
        self.failure_shown = false;
        self.window.pending()?;
        if let Some(source) = &self.source {
            self.generation = self.service.request(source.clone(), self.query.clone());
        }
        Ok(())
    }
    pub(crate) fn invalidate(&mut self) -> Result<()> {
        self.service.cancel();
        self.source = None;
        self.pending = true;
        self.window.pending()
    }

    pub(crate) fn poll(&mut self) -> Result<Option<SearchAction>> {
        if !self.active() {
            return Ok(None);
        }
        let events = self.window.take_events();
        if events.flags & messages::CANCEL != 0 {
            return Ok(Some(SearchAction::Cancel));
        }
        if !self.service.healthy() {
            if !self.failure_shown {
                error!("search stage=worker unavailable");
                self.window.failure()?;
                self.failure_shown = true;
                self.pending = true;
            }
            return Ok(None);
        }
        self.remember_selection();
        if events.flags & messages::RELAYOUT != 0 {
            self.window.layout()?;
        }
        if events.flags & messages::CHANGED != 0 {
            self.query = self.window.query()?;
            self.request()?;
        }
        if self.window.composing() {
            // The native EDIT owns composition; do not enable results while an
            // earlier committed query is returning during a new composition.
            return Ok(None);
        }
        for (generation, result) in self.service.take() {
            if generation != self.generation {
                continue;
            }
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    error!("search stage=match error={error:#}");
                    self.window.failure()?;
                    self.pending = true;
                    self.failure_shown = true;
                    continue;
                }
            };
            let selected = self
                .selected
                .and_then(|identity| {
                    result
                        .entries
                        .iter()
                        .position(|entry| entry.identity == identity)
                })
                .unwrap_or(0);
            self.window
                .replace(&result.entries, result.total, selected, generation)?;
            self.results = result.entries;
            self.pending = false;
            debug!(
                "search stage=results count={} total={}",
                self.results.len(),
                result.total
            );
        }
        if events.flags & messages::CHANGED == 0 && !self.pending {
            if let Some((_, index)) = events.accept.filter(|(epoch, _)| *epoch == self.generation) {
                if let Some(entry) = self.results.get(index) {
                    return Ok(Some(SearchAction::Activate(entry.clone())));
                }
            }
        }
        Ok(None)
    }
}
