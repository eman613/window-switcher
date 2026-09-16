//! UIA event delivery runs off-thread; intermediate frames coalesce.
use super::{
    provider,
    snapshot::{Bridge, Snapshot},
};
use crate::{
    utils::com::ComApartment,
    worker::{self, Mailbox},
};
use anyhow::{Context, Result};
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::{core::Result as ComResult, Win32::UI::Accessibility::*};

pub(super) struct Events {
    mailbox: Arc<Mailbox<Arc<Snapshot>, ()>>,
    thread: Option<JoinHandle<()>>,
}

impl Events {
    pub(super) fn start(shared: Arc<Bridge>) -> Result<Self> {
        let mailbox = Mailbox::<Arc<Snapshot>, ()>::new(1);
        let requests = mailbox.clone();
        let thread = thread::Builder::new()
            .name("accessibility-events".into())
            .spawn(move || {
                let _com = match ComApartment::mta() {
                    Ok(com) => com,
                    Err(error) => {
                        warn!("uia stage=event-apartment error={error:#}");
                        return;
                    }
                };
                let root = match provider::root(shared.clone()) {
                    Ok(root) => root,
                    Err(error) => {
                        warn!("uia stage=event-root code={:#x}", error.code().0);
                        return;
                    }
                };
                let mut previous = Arc::new(Snapshot::default());
                while shared.target.is_live() && !requests.closed() {
                    let Some((generation, snapshot)) = requests.receive(Duration::from_secs(1))
                    else {
                        continue;
                    };
                    if !requests.current(generation) {
                        continue;
                    }
                    let listening = unsafe { UiaClientsAreListening() }.as_bool();
                    trace!(
                        "uia stage=event-snapshot session={} selected={:?} focused={} listening={listening}",
                        snapshot.session, snapshot.selected, snapshot.focused
                    );
                    if listening {
                        let structure = previous.session != snapshot.session
                            || !previous
                                .entries
                                .iter()
                                .map(|entry| entry.id)
                                .eq(snapshot.entries.iter().map(|entry| entry.id));
                        if structure {
                            report("structure", unsafe {
                                UiaRaiseStructureChangedEvent(
                                    &root,
                                    StructureChangeType_ChildrenInvalidated,
                                    std::ptr::null_mut(),
                                    0,
                                )
                            });
                        }
                        if shared.visible(snapshot.session) && requests.current(generation) {
                            if snapshot.selected != previous.selected
                                || snapshot.session != previous.session
                                || snapshot.focused != previous.focused
                            {
                                if let Some(id) = snapshot.selected {
                                    let Ok(selected) =
                                        provider::item(shared.clone(), snapshot.session, id)
                                    else {
                                        continue;
                                    };
                                    report("selection", unsafe {
                                        UiaRaiseAutomationEvent(
                                            &selected,
                                            UIA_SelectionItem_ElementSelectedEventId,
                                        )
                                    });
                                    if snapshot.focused
                                        && shared.visible(snapshot.session)
                                        && requests.current(generation)
                                    {
                                        report("focus", unsafe {
                                            UiaRaiseAutomationEvent(
                                                &selected,
                                                UIA_AutomationFocusChangedEventId,
                                            )
                                        });
                                    }
                                }
                            }
                            for (old, new) in previous.entries.iter().zip(&snapshot.entries) {
                                if old.id != new.id || !requests.current(generation) {
                                    continue;
                                }
                                let Ok(element) =
                                    provider::item(shared.clone(), snapshot.session, new.id)
                                else {
                                    continue;
                                };
                                if old.name != new.name {
                                    report("name", unsafe {
                                        UiaRaiseAutomationPropertyChangedEvent(
                                            &element,
                                            UIA_NamePropertyId,
                                            &old.name.as_ref().into(),
                                            &new.name.as_ref().into(),
                                        )
                                    });
                                }
                                if old.status != new.status {
                                    report("status", unsafe {
                                        UiaRaiseAutomationPropertyChangedEvent(
                                            &element,
                                            UIA_ItemStatusPropertyId,
                                            &old.status.as_ref().into(),
                                            &new.status.as_ref().into(),
                                        )
                                    });
                                }
                            }
                        }
                    }
                    previous = snapshot;
                }
            })
            .context("uia stage=event-thread")?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
        })
    }
    pub(super) fn notify(&self, snapshot: Arc<Snapshot>) {
        self.mailbox.request(snapshot);
    }
}
fn report(kind: &str, result: ComResult<()>) {
    match result {
        Ok(()) => trace!("uia stage=raise-event kind={kind} accepted=true"),
        Err(error) => {
            debug!(
                "uia stage=raise-event kind={kind} code={:#x}",
                error.code().0
            );
        }
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        self.mailbox.close();
        worker::retire(self.thread.take(), "accessibility");
    }
}
