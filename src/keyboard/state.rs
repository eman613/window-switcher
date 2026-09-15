use crate::config::{Hotkey, SWITCH_APPS_HOTKEY_ID, SWITCH_WINDOWS_HOTKEY_ID};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchKind {
    Apps,
    Windows,
}

#[derive(Clone, Copy)]
pub(super) struct InputPermissions {
    pub(super) windows: bool,
    pub(super) apps: bool,
}

#[cfg(test)]
impl From<bool> for InputPermissions {
    fn from(windows: bool) -> Self {
        Self {
            windows,
            apps: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputAction {
    Cycle(SwitchKind, bool),
    Finish(SwitchKind),
    Cancel,
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
        self.pressed[index] = key.down;
        if let Some(gesture) = self.active.as_mut() {
            let allowed = if gesture.kind == SwitchKind::Apps {
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
            if !gesture.finishing && !self.modifier_pressed(gesture.modifier) {
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
        let matching = self.hotkeys.iter().find(|hotkey| {
            let allowed = (hotkey.id == SWITCH_APPS_HOTKEY_ID && permissions.apps)
                || (hotkey.id == SWITCH_WINDOWS_HOTKEY_ID && permissions.windows);
            let altgr = self.pressed[0x38 + 256] && (self.pressed[0x1d] || self.injected_control);
            allowed
                && !(altgr && matches!(hotkey.get_modifier(), 0x38 | 0x1d))
                && self.modifier_pressed(hotkey.get_modifier())
                && hotkey.code == key.scan
                && (!matches!(key.scan, 0x47..=0x53) || key.extended)
        });
        if let Some(hotkey) = matching {
            let kind = if hotkey.id == SWITCH_APPS_HOTKEY_ID {
                SwitchKind::Apps
            } else {
                SwitchKind::Windows
            };
            let modifier = hotkey.get_modifier();
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
            if gesture.kind == SwitchKind::Apps {
                let action = match key.scan {
                    0x01 => {
                        gesture.finishing = true;
                        Some(InputAction::Cancel)
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
    fn apps_blacklist_is_independent_and_altgr_is_not_an_alt_binding() {
        let mut state = machine();
        let permissions = InputPermissions {
            windows: true,
            apps: false,
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
