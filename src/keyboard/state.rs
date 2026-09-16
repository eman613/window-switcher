use crate::config::{
    Hotkey, DETAILS_HOTKEY_ID, PAUSE_HOTKEY_ID, SEARCH_HOTKEY_ID, SWITCH_APPS_HOTKEY_ID,
    SWITCH_WINDOWS_HOTKEY_ID,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchKind {
    Apps,
    Windows,
    Search,
}

#[derive(Clone, Copy)]
pub(super) struct InputPermissions {
    pub(super) windows: bool,
    pub(super) apps: bool,
    pub(super) surface: InputSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputSurface {
    None,
    Panel,
    Details,
}

#[cfg(test)]
impl From<bool> for InputPermissions {
    fn from(windows: bool) -> Self {
        Self {
            windows,
            apps: true,
            surface: InputSurface::Panel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputAction {
    Cycle(SwitchKind, bool),
    Finish(SwitchKind),
    Cancel,
    TogglePause,
    ShowDetails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputEvent {
    pub(crate) session: u64,
    pub(crate) action: InputAction,
}

#[derive(Clone, Copy)]
pub(super) struct KeyInput {
    pub scan: u32,
    pub extended: bool,
    pub down: bool,
}

impl KeyInput {
    fn index(self) -> Option<usize> {
        if self.scan == 0 || self.scan > 255 {
            None
        } else {
            Some(self.scan as usize + usize::from(self.extended) * 256)
        }
    }
}

#[derive(Default)]
pub(super) struct Decision {
    pub consume: bool,
    pub event: Option<InputEvent>,
}

struct Gesture {
    session: u64,
    kind: SwitchKind,
    modifier: u32,
    finishing: bool,
    sticky: bool,
}

pub(super) struct InputMachine {
    hotkeys: Vec<Hotkey>,
    pressed: [bool; 512],
    consumed: [bool; 512],
    active: Option<Gesture>,
    next_session: u64,
    injected_control: bool,
}

impl InputMachine {
    pub(super) fn new(hotkeys: Vec<Hotkey>) -> Self {
        Self {
            hotkeys,
            pressed: [false; 512],
            consumed: [false; 512],
            active: None,
            next_session: 0,
            injected_control: false,
        }
    }

    pub(super) fn seed_modifier(&mut self, scan: u32, extended: bool, down: bool) {
        if let Some(index) = (KeyInput {
            scan,
            extended,
            down,
        })
        .index()
        {
            self.pressed[index] = down;
        }
    }

    pub(super) fn reset(&mut self) -> Option<u64> {
        self.pressed.fill(false);
        self.consumed.fill(false);
        self.injected_control = false;
        // Keep the monotonic session counter: dispatch ACKs survive a rollback.
        self.active.take().map(|gesture| gesture.session)
    }

    pub(super) fn handle(
        &mut self,
        key: KeyInput,
        permissions: impl Into<InputPermissions>,
        acknowledged: u64,
        revoked: u64,
    ) -> Decision {
        let permissions = permissions.into();
        let Some(index) = key.index() else {
            return Decision::default();
        };
        if self
            .active
            .as_ref()
            .is_some_and(|g| g.session <= revoked || g.session <= acknowledged)
        {
            self.active = None;
        }
        let repeated = self.pressed[index];
        self.pressed[index] = key.down;
        // Recovery is independent of candidate permissions and sticky sessions.
        // Session zero denotes a control command, delivered through its own slot.
        if key.down
            && self
                .hotkeys
                .iter()
                .any(|hotkey| hotkey.id == PAUSE_HOTKEY_ID && self.matches_hotkey(hotkey, key))
        {
            return Decision {
                consume: true,
                event: (!repeated).then_some(InputEvent {
                    session: 0,
                    action: InputAction::TogglePause,
                }),
            };
        }
        if let Some(gesture) = self.active.as_mut() {
            let allowed = if gesture.kind != SwitchKind::Windows {
                permissions.apps
            } else {
                permissions.windows
            };
            if !allowed && !gesture.finishing {
                gesture.finishing = true;
                return Decision {
                    consume: false,
                    event: Some(InputEvent {
                        session: gesture.session,
                        action: InputAction::Cancel,
                    }),
                };
            }
        }
        if let Some(gesture) = self.active.as_ref() {
            if !gesture.sticky && !gesture.finishing && !self.modifier_pressed(gesture.modifier) {
                let event = InputEvent {
                    session: gesture.session,
                    action: InputAction::Finish(gesture.kind),
                };
                self.active.as_mut().unwrap().finishing = true;
                return Decision {
                    consume: false,
                    event: Some(event),
                };
            }
        }
        if !key.down {
            return Decision {
                consume: std::mem::take(&mut self.consumed[index]),
                event: None,
            };
        }
        if self.active.as_ref().is_some_and(|g| g.finishing) {
            return Decision::default();
        }
        // Retain ownership of a consumed trigger until release, even when opening
        // the view changes keyboard focus before the user's key-repeat arrives.
        if repeated
            && self.consumed[index]
            && self.active.as_ref().is_some_and(|g| g.sticky)
            && self
                .hotkeys
                .iter()
                .any(|hotkey| hotkey.id == DETAILS_HOTKEY_ID && hotkey.code == key.scan)
        {
            return Decision {
                consume: true,
                event: None,
            };
        }
        if permissions.surface == InputSurface::Panel
            && permissions.apps
            && self
                .active
                .as_ref()
                .is_some_and(|g| g.kind == SwitchKind::Apps)
            && self
                .hotkeys
                .iter()
                .any(|hotkey| hotkey.id == DETAILS_HOTKEY_ID && self.matches_hotkey(hotkey, key))
        {
            let gesture = self.active.as_mut().unwrap();
            gesture.sticky = true;
            return Decision {
                consume: true,
                event: (!repeated).then_some(InputEvent {
                    session: gesture.session,
                    action: InputAction::ShowDetails,
                }),
            };
        }
        let matching = self.hotkeys.iter().find(|hotkey| {
            let allowed = (matches!(hotkey.id, SWITCH_APPS_HOTKEY_ID | SEARCH_HOTKEY_ID)
                && permissions.apps)
                || (hotkey.id == SWITCH_WINDOWS_HOTKEY_ID && permissions.windows);
            allowed && self.matches_hotkey(hotkey, key)
        });
        if let Some(hotkey) = matching {
            let kind = match hotkey.id {
                SWITCH_APPS_HOTKEY_ID => SwitchKind::Apps,
                SEARCH_HOTKEY_ID => SwitchKind::Search,
                _ => SwitchKind::Windows,
            };
            let modifier = hotkey.get_modifier();
            // A normal switch replaces a sticky search with a new session.
            // Search text, navigation and composition remain native control input.
            if self.active.as_ref().is_some_and(|g| {
                (g.sticky && (kind != SwitchKind::Search || g.kind != SwitchKind::Search))
                    || (kind == SwitchKind::Search && g.kind != SwitchKind::Search)
            }) {
                self.active = None;
            }
            if self.active.as_ref().is_some_and(|g| g.modifier != modifier) {
                return Decision::default();
            }
            if self.active.is_none() {
                let Some(session) = self.next_session.checked_add(1) else {
                    return Decision::default();
                };
                self.next_session = session;
                self.active = Some(Gesture {
                    session,
                    kind,
                    modifier,
                    finishing: false,
                    sticky: kind == SwitchKind::Search,
                });
            }
            let reverse = self.pressed[0x2a] || self.pressed[0x36];
            let gesture = self.active.as_mut().unwrap();
            gesture.kind = kind;
            return Decision {
                consume: true,
                event: Some(InputEvent {
                    session: gesture.session,
                    action: InputAction::Cycle(kind, reverse),
                }),
            };
        }
        if let Some(gesture) = self.active.as_mut() {
            if gesture.kind == SwitchKind::Apps
                && (!gesture.sticky || permissions.surface == InputSurface::Panel)
            {
                let action = match key.scan {
                    0x01 => {
                        gesture.finishing = true;
                        Some(InputAction::Cancel)
                    }
                    0x1c if gesture.sticky => {
                        gesture.finishing = true;
                        Some(InputAction::Finish(SwitchKind::Apps))
                    }
                    0x48 | 0x4b | 0x4d | 0x50 if key.extended => Some(InputAction::Cycle(
                        SwitchKind::Apps,
                        matches!(key.scan, 0x48 | 0x4b),
                    )),
                    _ => None,
                };
                if let Some(action) = action {
                    return Decision {
                        consume: true,
                        event: Some(InputEvent {
                            session: gesture.session,
                            action,
                        }),
                    };
                }
            }
        }
        Decision::default()
    }

    pub(super) fn accepted(&mut self, key: KeyInput, consume: bool) {
        if key.down {
            if let Some(index) = key.index() {
                self.consumed[index] = consume;
            }
        }
    }

    pub(super) fn rejected(&mut self, key: KeyInput) {
        self.active = None;
        self.accepted(key, false);
    }

    pub(super) fn observe_injected_passthrough(&mut self, key: KeyInput) {
        // AltGr's synthetic left-Control is observed even when injected
        // shortcuts are passed through. It never becomes a binding modifier.
        if key.scan == 0x1d && !key.extended {
            self.injected_control = key.down;
        }
    }

    fn matches_hotkey(&self, hotkey: &Hotkey, key: KeyInput) -> bool {
        let altgr = self.pressed[0x38 + 256] && (self.pressed[0x1d] || self.injected_control);
        !(altgr && matches!(hotkey.get_modifier(), 0x38 | 0x1d))
            && self.modifier_pressed(hotkey.get_modifier())
            && hotkey.code == key.scan
            && (!matches!(key.scan, 0x47..=0x53) || key.extended)
    }

    fn modifier_pressed(&self, modifier: u32) -> bool {
        match modifier {
            0x38 | 0x1d => self.pressed[modifier as usize] || self.pressed[modifier as usize + 256],
            0x5b => self.pressed[0x5b + 256] || self.pressed[0x5c + 256],
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn machine() -> InputMachine {
        InputMachine::new(vec![
            Hotkey::create(SWITCH_APPS_HOTKEY_ID, "apps", "alt+tab").unwrap(),
            Hotkey::create(SWITCH_WINDOWS_HOTKEY_ID, "windows", "alt+`").unwrap(),
        ])
    }
    fn key(scan: u32, extended: bool, down: bool) -> KeyInput {
        KeyInput {
            scan,
            extended,
            down,
        }
    }

    #[test]
    fn details_is_panel_scoped_survives_release_and_returns_to_sticky_navigation() {
        let mut state = machine();
        state
            .hotkeys
            .push(Hotkey::create(DETAILS_HOTKEY_ID, "details", "ctrl+enter").unwrap());
        let outside = InputPermissions {
            apps: true,
            windows: true,
            surface: InputSurface::None,
        };
        let panel = InputPermissions {
            surface: InputSurface::Panel,
            ..outside
        };
        let details = InputPermissions {
            surface: InputSurface::Details,
            ..outside
        };
        state.handle(key(0x1d, false, true), outside, 0, 0);
        assert!(!state.handle(key(0x1c, false, true), outside, 0, 0).consume);
        state.handle(key(0x1c, false, false), outside, 0, 0);
        state.handle(key(0x38, false, true), panel, 0, 0);
        let session = state
            .handle(key(0x0f, false, true), panel, 0, 0)
            .event
            .unwrap()
            .session;
        let trigger = key(0x1c, false, true);
        let entered = state.handle(trigger, panel, 0, 0);
        assert_eq!(entered.event.unwrap().action, InputAction::ShowDetails);
        state.accepted(trigger, entered.consume);
        let repeated = state.handle(trigger, details, 0, 0);
        assert!(repeated.consume && repeated.event.is_none());
        assert!(state.handle(key(0x1c, false, false), details, 0, 0).consume);
        assert!(state
            .handle(key(0x38, false, false), details, 0, 0)
            .event
            .is_none());
        state.handle(key(0x1d, false, false), details, 0, 0);
        for scan in [0x48, 0x50, 0x1c, 0x01] {
            let result = state.handle(key(scan, scan != 0x1c && scan != 0x01, true), details, 0, 0);
            assert!(!result.consume && result.event.is_none());
            state.handle(
                key(scan, scan != 0x1c && scan != 0x01, false),
                details,
                0,
                0,
            );
        }
        assert!(!state.handle(key(0x48, true, true), outside, 0, 0).consume);
        assert_eq!(
            state
                .handle(key(0x50, true, true), panel, 0, 0)
                .event
                .unwrap()
                .action,
            InputAction::Cycle(SwitchKind::Apps, false)
        );
        let finish = state
            .handle(key(0x1c, false, true), panel, 0, 0)
            .event
            .unwrap();
        assert_eq!(finish.session, session);
        assert_eq!(finish.action, InputAction::Finish(SwitchKind::Apps));
    }

    #[test]
    fn a_global_binding_replaces_details_with_a_new_session() {
        let mut state = machine();
        state
            .hotkeys
            .push(Hotkey::create(DETAILS_HOTKEY_ID, "details", "ctrl+enter").unwrap());
        state
            .hotkeys
            .push(Hotkey::create(SEARCH_HOTKEY_ID, "search", "ctrl+space").unwrap());
        state.handle(key(0x38, false, true), true, 0, 0);
        let first = state
            .handle(key(0x0f, false, true), true, 0, 0)
            .event
            .unwrap();
        state.handle(key(0x1d, false, true), true, 0, 0);
        state.handle(key(0x1c, false, true), true, 0, 0);
        let next = state
            .handle(
                key(0x39, false, true),
                InputPermissions {
                    apps: true,
                    windows: true,
                    surface: InputSurface::Details,
                },
                0,
                0,
            )
            .event
            .unwrap();
        assert!(next.session > first.session);
        assert_eq!(next.action, InputAction::Cycle(SwitchKind::Search, false));
    }

    #[test]
    fn pause_recovery_bypasses_permissions_without_repeating_or_leaking_release() {
        let mut state = machine();
        state
            .hotkeys
            .push(Hotkey::create(PAUSE_HOTKEY_ID, "pause", "ctrl+f10").unwrap());
        let denied = InputPermissions {
            apps: false,
            windows: false,
            surface: InputSurface::None,
        };
        state.handle(key(0x1d, false, true), denied, 0, 0);
        let down = key(0x44, false, true);
        let first = state.handle(down, denied, 0, 0);
        assert!(first.consume);
        assert_eq!(first.event.unwrap().action, InputAction::TogglePause);
        state.accepted(down, first.consume);
        assert!(state.handle(down, denied, 0, 0).event.is_none());
        assert!(state.handle(key(0x44, false, false), denied, 0, 0).consume);
        assert!(!state.handle(key(0x1d, false, false), denied, 0, 0).consume);
        state.handle(key(0x38, false, true), denied, 0, 0);
        let normal = state.handle(key(0x0f, false, true), denied, 0, 0);
        assert!(!normal.consume && normal.event.is_none());
    }

    #[test]
    fn search_survives_modifier_release_and_preserves_native_text_input() {
        let mut state = machine();
        state
            .hotkeys
            .push(Hotkey::create(SEARCH_HOTKEY_ID, "search", "ctrl+space").unwrap());
        state.handle(key(0x1d, false, true), true, 0, 0);
        let search = state
            .handle(key(0x39, false, true), true, 0, 0)
            .event
            .unwrap();
        assert_eq!(search.action, InputAction::Cycle(SwitchKind::Search, false));
        assert!(state
            .handle(key(0x1d, false, false), true, 0, 0)
            .event
            .is_none());
        for scan in [0x1e, 0x0e, 0x1c, 0x01, 0x48] {
            let decision = state.handle(key(scan, scan == 0x48, true), true, 0, 0);
            assert!(!decision.consume && decision.event.is_none());
        }
        state.handle(key(0x38, false, true), true, 0, 0);
        let apps = state
            .handle(key(0x0f, false, true), true, 0, 0)
            .event
            .unwrap();
        assert!(apps.session > search.session);
        assert_eq!(apps.action, InputAction::Cycle(SwitchKind::Apps, false));
        assert_eq!(
            state
                .handle(key(0x38, false, false), true, 0, 0)
                .event
                .unwrap()
                .action,
            InputAction::Finish(SwitchKind::Apps)
        );
    }

    #[test]
    fn apps_blacklist_is_independent_and_altgr_is_not_an_alt_binding() {
        let mut state = machine();
        let permissions = InputPermissions {
            windows: true,
            apps: false,
            surface: InputSurface::None,
        };
        state.handle(key(0x38, false, true), permissions, 0, 0);
        assert!(
            !state
                .handle(key(0x0f, false, true), permissions, 0, 0)
                .consume
        );
        assert!(
            state
                .handle(key(0x29, false, true), permissions, 0, 0)
                .consume
        );
        state.reset();
        state.handle(key(0x1d, false, true), true, 0, 0);
        state.handle(key(0x38, true, true), true, 0, 0);
        assert!(!state.handle(key(0x0f, false, true), true, 0, 0).consume);
        state.reset();
        state.observe_injected_passthrough(key(0x1d, false, true));
        state.handle(key(0x38, true, true), true, 0, 0);
        assert!(!state.handle(key(0x0f, false, true), true, 0, 0).consume);
        state.observe_injected_passthrough(key(0x1d, false, false));
        assert!(state.handle(key(0x0f, false, true), true, 0, 0).consume);
    }

    #[test]
    fn overlapping_modifiers_and_shift_release_preserve_other_side() {
        let mut state = machine();
        state.handle(key(0x38, false, true), true, 0, 0);
        state.handle(key(0x38, true, true), true, 0, 0);
        state.handle(key(0x2a, false, true), true, 0, 0);
        state.handle(key(0x36, false, true), true, 0, 0);
        state.handle(key(0x2a, false, false), true, 0, 0);
        assert_eq!(
            state
                .handle(key(0x0f, false, true), true, 0, 0)
                .event
                .unwrap()
                .action,
            InputAction::Cycle(SwitchKind::Apps, true)
        );
        assert!(state
            .handle(key(0x38, false, false), true, 0, 0)
            .event
            .is_none());
        assert_eq!(
            state
                .handle(key(0x38, true, false), true, 0, 0)
                .event
                .unwrap()
                .action,
            InputAction::Finish(SwitchKind::Apps)
        );
    }

    #[test]
    fn esc_only_consumed_in_active_apps_and_numpad_is_not_arrow() {
        let mut state = machine();
        state.handle(key(0x38, false, true), true, 0, 0);
        assert!(!state.handle(key(0x01, false, true), true, 0, 0).consume);
        state.handle(key(0x0f, false, true), true, 0, 0);
        assert!(!state.handle(key(0x48, false, true), true, 0, 0).consume);
        assert!(state.handle(key(0x48, true, true), true, 0, 0).consume);
        assert_eq!(
            state
                .handle(key(0x01, false, true), true, 0, 0)
                .event
                .unwrap()
                .action,
            InputAction::Cancel
        );
        assert!(!state.handle(key(0x0f, false, true), true, 0, 0).consume);
        assert!(state.handle(key(0x0f, false, true), true, 1, 0).consume);
    }

    #[test]
    fn finish_does_not_depend_on_last_key_and_blacklist_fails_open() {
        let mut state = machine();
        state.handle(key(0x38, false, true), true, 0, 0);
        assert!(!state.handle(key(0x29, false, true), false, 0, 0).consume);
        let event = state
            .handle(key(0x0f, false, true), true, 0, 0)
            .event
            .unwrap();
        state.handle(key(0x1e, false, true), true, 0, 0);
        assert_eq!(
            state
                .handle(key(0x38, false, false), true, 0, 0)
                .event
                .unwrap()
                .session,
            event.session
        );
        state.rejected(key(0x0f, false, true));
        assert!(
            !state
                .handle(key(0x0f, false, false), true, 0, event.session)
                .consume
        );
    }
}
