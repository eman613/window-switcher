#![allow(non_upper_case_globals)] // UIA constants retain their Windows API names.
use super::{
    arrays, provider,
    snapshot::{unavailable, ActionKind, Bridge, Entry, Snapshot},
};
use crate::layout::PixelRect;
use std::{ptr, sync::Arc};
use windows::{
    core::{Interface, Result},
    Win32::{
        System::{Com::SAFEARRAY, Variant::VARIANT},
        UI::Accessibility::*,
    },
};

pub(super) struct Node {
    pub(super) shared: Arc<Bridge>,
    pub(super) item: Option<(u64, u32)>,
}

impl Node {
    pub(super) fn snapshot(&self) -> Result<Arc<Snapshot>> {
        let snapshot = self.shared.read()?;
        if let Some((session, id)) = self.item {
            if !self.shared.visible(session)
                || snapshot.session != session
                || !snapshot.entries.iter().any(|entry| entry.id == id)
            {
                return Err(unavailable());
            }
        }
        Ok(snapshot)
    }
    fn entry<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a Entry> {
        self.item
            .and_then(|(_, id)| snapshot.entries.iter().find(|entry| entry.id == id))
    }
    pub(super) fn fragment(&self, session: u64, id: u32) -> Result<IRawElementProviderFragment> {
        provider::item(self.shared.clone(), session, id)?.cast()
    }
    pub(super) fn root(&self) -> Result<IRawElementProviderSimple> {
        provider::root(self.shared.clone())
    }
    pub(super) fn bounds(&self) -> Result<UiaRect> {
        let snapshot = self.snapshot()?;
        let rect = if !self.shared.visible(snapshot.session) {
            PixelRect::default()
        } else if self.item.is_some() {
            self.entry(&snapshot)
                .and_then(|entry| entry.bounds)
                .unwrap_or_default()
        } else {
            snapshot.bounds
        };
        Ok(UiaRect {
            left: f64::from(rect.left),
            top: f64::from(rect.top),
            width: f64::from(rect.width()),
            height: f64::from(rect.height()),
        })
    }
    pub(super) fn property(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let snapshot = self.snapshot()?;
        let entry = self.entry(&snapshot);
        let visible = self.shared.visible(snapshot.session);
        let selected = visible && entry.is_some_and(|entry| Some(entry.id) == snapshot.selected);
        Ok(match property {
            UIA_ControlTypePropertyId => if entry.is_some() {
                UIA_ListItemControlTypeId.0
            } else {
                UIA_ListControlTypeId.0
            }
            .into(),
            UIA_NamePropertyId => entry
                .map_or(self.shared.name.as_ref(), |entry| entry.name.as_ref())
                .into(),
            UIA_ItemStatusPropertyId => entry.map_or("", |entry| entry.status.as_ref()).into(),
            UIA_HelpTextPropertyId => self.shared.help.as_ref().into(),
            UIA_FrameworkIdPropertyId => "Win32".into(),
            UIA_ClassNamePropertyId => "Window Switcher".into(),
            UIA_AutomationIdPropertyId => self.item.map_or_else(
                || VARIANT::from("window-switcher"),
                |(session, id)| VARIANT::from(format!("app-{session}-{id}").as_str()),
            ),
            UIA_ProcessIdPropertyId => (std::process::id() as i32).into(),
            UIA_NativeWindowHandlePropertyId if self.item.is_none() => {
                (self.shared.target.window_id() as i32).into()
            }
            UIA_IsControlElementPropertyId
            | UIA_IsContentElementPropertyId
            | UIA_IsKeyboardFocusablePropertyId => true.into(),
            UIA_IsEnabledPropertyId => visible.into(),
            // The HWND root delegates focus to its host provider. Its native
            // focus must not be overridden by virtual-item selection state.
            UIA_HasKeyboardFocusPropertyId if entry.is_some() => {
                (selected && snapshot.focused).into()
            }
            UIA_IsOffscreenPropertyId => {
                (!visible || entry.is_some_and(|entry| entry.bounds.is_none())).into()
            }
            _ => VARIANT::default(),
        })
    }
    pub(super) fn navigate(
        &self,
        direction: NavigateDirection,
    ) -> Result<Option<IRawElementProviderFragment>> {
        let snapshot = self.snapshot()?;
        if !self.shared.visible(snapshot.session) {
            return Ok(None);
        }
        let index = self
            .item
            .and_then(|(_, id)| snapshot.entries.iter().position(|entry| entry.id == id));
        let target = match (direction, index) {
            (NavigateDirection_Parent, Some(_)) => return self.root()?.cast().map(Some),
            (NavigateDirection_FirstChild, None) => snapshot.entries.first(),
            (NavigateDirection_LastChild, None) => snapshot.entries.last(),
            (NavigateDirection_NextSibling, Some(index)) => snapshot.entries.get(index + 1),
            (NavigateDirection_PreviousSibling, Some(index)) => index
                .checked_sub(1)
                .and_then(|index| snapshot.entries.get(index)),
            _ => None,
        };
        target
            .map(|entry| self.fragment(snapshot.session, entry.id))
            .transpose()
    }
    pub(super) fn runtime_id(&self) -> Result<*mut SAFEARRAY> {
        self.snapshot()?;
        match self.item {
            Some((session, id)) => arrays::integers(&[
                UiaAppendRuntimeId as i32,
                session as i32,
                (session >> 32) as i32,
                id as i32,
            ]),
            None => Ok(ptr::null_mut()),
        }
    }
    pub(super) fn act(&self, kind: ActionKind) -> Result<()> {
        let snapshot = self.snapshot()?;
        let id = self
            .item
            .map(|(_, id)| id)
            .or(snapshot.selected)
            .ok_or_else(unavailable)?;
        self.shared.send(snapshot.session, id, kind)
    }
    pub(super) fn selected(&self) -> Result<bool> {
        let snapshot = self.snapshot()?;
        Ok(self.shared.visible(snapshot.session)
            && self
                .item
                .is_some_and(|(_, id)| snapshot.selected == Some(id)))
    }
    pub(super) fn focus(&self) -> Result<Option<IRawElementProviderFragment>> {
        self.selected_fragment(true)
    }
    pub(super) fn selection(&self) -> Result<Option<IRawElementProviderFragment>> {
        self.selected_fragment(false)
    }
    fn selected_fragment(
        &self,
        require_focus: bool,
    ) -> Result<Option<IRawElementProviderFragment>> {
        let snapshot = self.snapshot()?;
        if !self.shared.visible(snapshot.session) || (require_focus && !snapshot.focused) {
            return Ok(None);
        }
        snapshot
            .selected
            .map(|id| self.fragment(snapshot.session, id))
            .transpose()
    }
    pub(super) fn provider_at_point(
        &self,
        x: f64,
        y: f64,
    ) -> Result<Option<IRawElementProviderFragment>> {
        let snapshot = self.snapshot()?;
        if !x.is_finite()
            || !y.is_finite()
            || !self.shared.visible(snapshot.session)
            || !snapshot.bounds.contains(x as i32, y as i32)
        {
            return Ok(None);
        }
        if let Some(entry) = snapshot.entries.iter().find(|entry| {
            entry
                .bounds
                .is_some_and(|rect| rect.contains(x as i32, y as i32))
        }) {
            return self.fragment(snapshot.session, entry.id).map(Some);
        }
        self.root()?.cast().map(Some)
    }
}
