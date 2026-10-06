use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{ensure, Context, Result};

use super::{index::SearchIndex, SearchResults, MAX_QUERY_UNITS};
use crate::{
    app_name::NameCache,
    config::Config,
    keyboard::dispatch::WM_INPUT_READY,
    utils::com::ComApartment,
    window_snapshot::WindowSnapshot,
    window_target::WindowTarget,
    worker::{self, Mailbox},
};

struct Request {
    source: Arc<WindowSnapshot>,
    query: String,
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
                            .is_none_or(|index| !index.uses(&request.source))
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
