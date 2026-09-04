use crate::{
    config::{BackdropFallback, BackdropMode},
    utils::os_version_info,
};

use anyhow::{anyhow, Result};
use std::{ffi::c_void, mem::size_of};
use windows::{
    core::{s, w, BOOL},
    Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DwmIsCompositionEnabled, DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_NONE,
            DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWM_SYSTEMBACKDROP_TYPE,
        },
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION},
    },
};

const SYSTEM_BACKDROP_MIN_BUILD: u32 = 22_621;
const SWCA_MIN_BUILD: u32 = 17_763;
const WINDOW_COMPOSITION_ATTRIBUTE_ACCENT_POLICY: u32 = 19;
const ACCENT_DISABLED: u32 = 0;
const ACCENT_ENABLE_BLUR_BEHIND: u32 = 3;
const ACCENT_ENABLE_ACRYLIC_BLUR_BEHIND: u32 = 4;

type SetWindowCompositionAttributeFn =
    unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> BOOL;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttributeData {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppliedBackdrop {
    Solid,
    Alpha,
    SwcaBlur,
    SwcaAcrylic,
    SystemAcrylic,
    Mica,
}

impl AppliedBackdrop {
    const fn uses_alpha(self) -> bool {
        !matches!(self, Self::Solid)
    }
}

#[derive(Debug, Clone, Copy)]
struct CapabilityFlags {
    system_backdrop: bool,
    swca: bool,
}

struct BackdropCapabilities {
    build: u32,
    composition_enabled: bool,
    remote_session: bool,
    set_window_composition_attribute: Option<SetWindowCompositionAttributeFn>,
}

impl BackdropCapabilities {
    fn detect() -> Self {
        let build = os_version_info()
            .map(|info| info.dwBuildNumber)
            .unwrap_or_default();
        let composition_enabled = match unsafe { DwmIsCompositionEnabled() } {
            Ok(enabled) => enabled.as_bool(),
            Err(err) => {
                warn!("DwmIsCompositionEnabled failed; disabling backdrop effects: {err}");
                false
            }
        };
        let remote_session = unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0;
        let set_window_composition_attribute =
            (composition_enabled && !remote_session && build >= SWCA_MIN_BUILD)
                .then(load_set_window_composition_attribute)
                .flatten();

        Self {
            build,
            composition_enabled,
            remote_session,
            set_window_composition_attribute,
        }
    }

    fn flags(&self) -> CapabilityFlags {
        let effects_available = self.composition_enabled && !self.remote_session;
        CapabilityFlags {
            system_backdrop: effects_available && self.build >= SYSTEM_BACKDROP_MIN_BUILD,
            swca: effects_available
                && self.build >= SWCA_MIN_BUILD
                && self.set_window_composition_attribute.is_some(),
        }
    }
}

pub(crate) struct BackdropController {
    hwnd: HWND,
    applied: AppliedBackdrop,
    fallback: AppliedBackdrop,
    set_window_composition_attribute: Option<SetWindowCompositionAttributeFn>,
    last_accent_color: Option<u32>,
}

impl BackdropController {
    pub(crate) fn new(
        hwnd: HWND,
        requested: BackdropMode,
        fallback: BackdropFallback,
        rgb: u32,
        opacity: u8,
    ) -> Self {
        let fallback = fallback_mode(fallback);
        if let Some(applied) = match requested {
            BackdropMode::None => Some(AppliedBackdrop::Solid),
            BackdropMode::Alpha => Some(AppliedBackdrop::Alpha),
            _ => None,
        } {
            info!("backdrop requested={requested:?} applied={applied:?}");
            return Self {
                hwnd,
                applied,
                fallback,
                set_window_composition_attribute: None,
                last_accent_color: None,
            };
        }

        let capabilities = BackdropCapabilities::detect();
        let candidates = candidate_order(requested, fallback, capabilities.flags());
        let mut controller = Self {
            hwnd,
            applied: AppliedBackdrop::Solid,
            fallback,
            set_window_composition_attribute: capabilities.set_window_composition_attribute,
            last_accent_color: None,
        };

        for candidate in candidates {
            match controller.apply(candidate, rgb, opacity) {
                Ok(()) => {
                    controller.applied = candidate;
                    info!(
                        "backdrop requested={requested:?} applied={candidate:?} build={} composition={} remote_session={}",
                        capabilities.build,
                        capabilities.composition_enabled,
                        capabilities.remote_session
                    );
                    return controller;
                }
                Err(err) => {
                    warn!("failed to apply {candidate:?} backdrop; trying fallback: {err:#}");
                }
            }
        }

        controller
    }

    pub(crate) fn background_alpha(&self, configured_opacity: u8) -> u8 {
        if self.applied.uses_alpha() {
            opacity_to_alpha(configured_opacity)
        } else {
            u8::MAX
        }
    }

    pub(crate) fn update_tint(&mut self, rgb: u32, opacity: u8) {
        let state = match self.applied {
            AppliedBackdrop::SwcaBlur => ACCENT_ENABLE_BLUR_BEHIND,
            AppliedBackdrop::SwcaAcrylic => ACCENT_ENABLE_ACRYLIC_BLUR_BEHIND,
            _ => return,
        };
        let acrylic = self.applied == AppliedBackdrop::SwcaAcrylic;
        let color = accent_gradient_color(rgb, opacity, acrylic);
        if self.last_accent_color == Some(color) {
            return;
        }

        if let Err(err) = self.apply_accent(state, color) {
            warn!(
                "failed to refresh {:?} backdrop tint; using {:?}: {err:#}",
                self.applied, self.fallback
            );
            let _ = self.disable_accent();
            self.applied = self.fallback;
            self.last_accent_color = None;
        }
    }

    fn apply(&mut self, candidate: AppliedBackdrop, rgb: u32, opacity: u8) -> Result<()> {
        match candidate {
            AppliedBackdrop::Solid | AppliedBackdrop::Alpha => Ok(()),
            AppliedBackdrop::SwcaBlur => self.apply_accent(
                ACCENT_ENABLE_BLUR_BEHIND,
                accent_gradient_color(rgb, opacity, false),
            ),
            AppliedBackdrop::SwcaAcrylic => self.apply_accent(
                ACCENT_ENABLE_ACRYLIC_BLUR_BEHIND,
                accent_gradient_color(rgb, opacity, true),
            ),
            AppliedBackdrop::SystemAcrylic => self.apply_system_backdrop(DWMSBT_TRANSIENTWINDOW),
            AppliedBackdrop::Mica => self.apply_system_backdrop(DWMSBT_MAINWINDOW),
        }
    }

    fn apply_accent(&mut self, state: u32, gradient_color: u32) -> Result<()> {
        let function = self
            .set_window_composition_attribute
            .ok_or_else(|| anyhow!("SetWindowCompositionAttribute is unavailable"))?;
        let mut policy = AccentPolicy {
            state,
            flags: if state == ACCENT_ENABLE_BLUR_BEHIND {
                2
            } else {
                0
            },
            gradient_color,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttributeData {
            attribute: WINDOW_COMPOSITION_ATTRIBUTE_ACCENT_POLICY,
            data: (&mut policy as *mut AccentPolicy).cast(),
            size: size_of::<AccentPolicy>(),
        };
        if !unsafe { function(self.hwnd, &mut data) }.as_bool() {
            return Err(anyhow!("SetWindowCompositionAttribute returned FALSE"));
        }
        self.last_accent_color = Some(gradient_color);
        Ok(())
    }

    fn disable_accent(&mut self) -> Result<()> {
        self.apply_accent(ACCENT_DISABLED, 0)
    }

    fn apply_system_backdrop(&self, backdrop_type: DWM_SYSTEMBACKDROP_TYPE) -> Result<()> {
        unsafe {
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                (&backdrop_type as *const DWM_SYSTEMBACKDROP_TYPE).cast(),
                size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
            )
        }
        .map_err(|err| anyhow!("DwmSetWindowAttribute failed: {err}"))
    }

    fn clear(&mut self) {
        match self.applied {
            AppliedBackdrop::SwcaBlur | AppliedBackdrop::SwcaAcrylic => {
                let _ = self.disable_accent();
            }
            AppliedBackdrop::SystemAcrylic | AppliedBackdrop::Mica => {
                let _ = self.apply_system_backdrop(DWMSBT_NONE);
            }
            AppliedBackdrop::Solid | AppliedBackdrop::Alpha => {}
        }
    }
}

impl Drop for BackdropController {
    fn drop(&mut self) {
        self.clear();
    }
}

fn candidate_order(
    requested: BackdropMode,
    fallback: AppliedBackdrop,
    capabilities: CapabilityFlags,
) -> Vec<AppliedBackdrop> {
    let mut candidates = Vec::with_capacity(4);
    match requested {
        BackdropMode::None => push_candidate(&mut candidates, AppliedBackdrop::Solid),
        BackdropMode::Alpha => push_candidate(&mut candidates, AppliedBackdrop::Alpha),
        BackdropMode::Blur => {
            if capabilities.swca {
                push_candidate(&mut candidates, AppliedBackdrop::SwcaBlur);
            }
            push_candidate(&mut candidates, fallback);
        }
        BackdropMode::Acrylic | BackdropMode::Auto => {
            if capabilities.system_backdrop {
                push_candidate(&mut candidates, AppliedBackdrop::SystemAcrylic);
            }
            if capabilities.swca {
                push_candidate(&mut candidates, AppliedBackdrop::SwcaAcrylic);
            }
            push_candidate(&mut candidates, fallback);
        }
        BackdropMode::Mica => {
            if capabilities.system_backdrop {
                push_candidate(&mut candidates, AppliedBackdrop::Mica);
                push_candidate(&mut candidates, AppliedBackdrop::SystemAcrylic);
            }
            if capabilities.swca {
                push_candidate(&mut candidates, AppliedBackdrop::SwcaAcrylic);
            }
            push_candidate(&mut candidates, fallback);
        }
    }
    candidates
}

fn push_candidate(candidates: &mut Vec<AppliedBackdrop>, candidate: AppliedBackdrop) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

const fn fallback_mode(fallback: BackdropFallback) -> AppliedBackdrop {
    match fallback {
        BackdropFallback::Solid => AppliedBackdrop::Solid,
        BackdropFallback::Alpha => AppliedBackdrop::Alpha,
    }
}

const fn opacity_to_alpha(opacity: u8) -> u8 {
    ((opacity as u16 * u8::MAX as u16 + 50) / 100) as u8
}

const fn accent_gradient_color(rgb: u32, opacity: u8, acrylic: bool) -> u32 {
    let red = (rgb >> 16) & 0xff;
    let green = (rgb >> 8) & 0xff;
    let blue = rgb & 0xff;
    let mut alpha = opacity_to_alpha(opacity) as u32;
    if acrylic && alpha == 0 {
        alpha = 1;
    }
    red | (green << 8) | (blue << 16) | (alpha << 24)
}

fn load_set_window_composition_attribute() -> Option<SetWindowCompositionAttributeFn> {
    let module = unsafe { GetModuleHandleW(w!("user32.dll")) }.ok()?;
    let procedure = unsafe { GetProcAddress(module, s!("SetWindowCompositionAttribute")) }?;
    Some(unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SetWindowCompositionAttributeFn>(
            procedure,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_order_uses_supported_effects_and_configured_fallback() {
        let all = CapabilityFlags {
            system_backdrop: true,
            swca: true,
        };
        assert_eq!(
            candidate_order(BackdropMode::Acrylic, AppliedBackdrop::Alpha, all),
            vec![
                AppliedBackdrop::SystemAcrylic,
                AppliedBackdrop::SwcaAcrylic,
                AppliedBackdrop::Alpha,
            ]
        );
        assert_eq!(
            candidate_order(BackdropMode::Mica, AppliedBackdrop::Solid, all),
            vec![
                AppliedBackdrop::Mica,
                AppliedBackdrop::SystemAcrylic,
                AppliedBackdrop::SwcaAcrylic,
                AppliedBackdrop::Solid,
            ]
        );

        let unavailable = CapabilityFlags {
            system_backdrop: false,
            swca: false,
        };
        assert_eq!(
            candidate_order(BackdropMode::Auto, AppliedBackdrop::Alpha, unavailable),
            vec![AppliedBackdrop::Alpha]
        );
    }

    #[test]
    fn opacity_and_accent_color_use_expected_byte_order() {
        assert_eq!(opacity_to_alpha(0), 0);
        assert_eq!(opacity_to_alpha(50), 128);
        assert_eq!(opacity_to_alpha(100), 255);
        assert_eq!(accent_gradient_color(0x12_34_56, 50, false), 0x80_56_34_12);
        assert_eq!(accent_gradient_color(0x12_34_56, 0, true), 0x01_56_34_12);
    }
}
