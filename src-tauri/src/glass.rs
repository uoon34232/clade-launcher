//! The optional translucent window material ("glass").
//!
//! Glass is purely cosmetic and must never get in the way: when anything about it is unavailable
//! or fails, Clade silently stays on the opaque warm treatment it has always had.
//!
//! **Technique (Windows 11):** the window keeps its normal, opaque WebView background. Turning
//! glass on asks the desktop window manager for its documented *Mica* system backdrop
//! (`DwmSetWindowAttribute` with `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW`, forcing the
//! light variant with `DWMWA_USE_IMMERSIVE_DARK_MODE = 0`) and makes the WebView's own background
//! transparent (`set_background_color` with alpha 0), so the page's translucent surfaces sit over
//! the cached desktop material without per-frame live-blur GPU overhead. Turning it off clears the
//! backdrop and restores the opaque background, so nothing of the desktop can show through. Both
//! directions are applied to the running window, deduplicated so repeated focus events never poke
//! DWM when nothing changed, and suspended while the window is hidden.
//!
//! **What decides it** (`wanted`): High Contrast always wins and means opaque. Otherwise the
//! person's choice (`ui.transparency`): `off` is opaque, `on` asks for glass, and `system` follows
//! the Windows "Transparency effects" preference, which is read once and watched on the dedicated
//! `glass-watch` thread through the documented
//! `Windows.UI.ViewManagement.UISettings.AdvancedEffectsEnabled` (a read-only OS preference API;
//! nothing is ever written to the registry or to Windows settings).
//!
//! **Limit, stated plainly:** Windows itself replaces Mica with a solid colour while its
//! "Transparency effects" switch is off. `on` therefore still gives the person's explicit
//! request, but the material behind the page is then Windows' solid fallback (`live = false`), and
//! the page uses its denser surfaces. Clade cannot make Windows draw live transparency against the
//! person's system setting without capturing the screen, which it never does.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Event announcing a change of the resolved glass state.
pub const STATUS_EVENT: &str = "glass-status-changed";

/// What the interface should render. `active` is whether the window material is on; `live` is
/// whether Windows is really drawing it (false when Windows shows its solid fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlassStatus {
    pub active: bool,
    pub live: bool,
}

impl GlassStatus {
    /// The opaque treatment: no material, no transparency of any kind.
    pub const OPAQUE: Self = Self {
        active: false,
        live: false,
    };
}

/// The facts about this PC that glass depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Environment {
    /// The Windows "Transparency effects" preference; `None` when it cannot be read.
    pub system_effects: Option<bool>,
    /// Windows High Contrast is on.
    pub high_contrast: bool,
}

/// The native side: turns the window material on or off. A seam so the decision logic is testable.
pub trait Backdrop {
    fn set(&self, on: bool) -> Result<(), String>;
}

/// Whether glass should be on for this choice and this PC. Pure.
pub fn wanted(mode: &str, env: Environment) -> bool {
    if env.high_contrast {
        return false;
    }
    match mode {
        "on" => true,
        "off" => false,
        // "system", and anything unexpected: follow Windows; an unreadable preference means opaque.
        _ => env.system_effects.unwrap_or(false),
    }
}

/// Decides and applies the material. A failure to initialise it is logged and ends in the opaque
/// treatment; it never propagates.
pub fn resolve(mode: &str, env: Environment, backdrop: &dyn Backdrop) -> GlassStatus {
    if !wanted(mode, env) {
        if let Err(e) = backdrop.set(false) {
            tracing::warn!(error = %e, "could not clear the window backdrop");
        }
        return GlassStatus::OPAQUE;
    }
    match backdrop.set(true) {
        Ok(()) => GlassStatus {
            active: true,
            live: env.system_effects.unwrap_or(false),
        },
        Err(e) => {
            tracing::warn!(error = %e, "the window backdrop is unavailable, using the opaque look");
            // Make sure nothing half-applied stays behind.
            let _ = backdrop.set(false);
            GlassStatus::OPAQUE
        }
    }
}

const EFFECTS_UNKNOWN: u8 = 0;
const EFFECTS_OFF: u8 = 1;
const EFFECTS_ON: u8 = 2;

/// The resolved state, shared with the interface.
pub struct GlassState {
    status: Mutex<GlassStatus>,
    system_effects: AtomicU8,
    applied_backdrop: Mutex<Option<bool>>,
    suspended: AtomicBool,
}

impl GlassState {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(GlassStatus::OPAQUE),
            system_effects: AtomicU8::new(EFFECTS_UNKNOWN),
            applied_backdrop: Mutex::new(None),
            suspended: AtomicBool::new(false),
        }
    }

    fn get(&self) -> GlassStatus {
        *self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores `next`; true when it differs from the previous value.
    fn replace(&self, next: GlassStatus) -> bool {
        let mut guard = self.status.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *guard != next;
        *guard = next;
        changed
    }

    pub(crate) fn set_system_effects(&self, enabled: Option<bool>) {
        let encoded = match enabled {
            None => EFFECTS_UNKNOWN,
            Some(false) => EFFECTS_OFF,
            Some(true) => EFFECTS_ON,
        };
        self.system_effects.store(encoded, Ordering::Release);
    }

    pub(crate) fn system_effects(&self) -> Option<bool> {
        match self.system_effects.load(Ordering::Acquire) {
            EFFECTS_OFF => Some(false),
            EFFECTS_ON => Some(true),
            _ => None,
        }
    }

    /// Resolves and applies the backdrop if the window is active, deduplicating redundant native
    /// DWM calls. Returns `Some(status)` only when the UI-facing status changed.
    fn apply_refresh(
        &self,
        mode: &str,
        high_contrast: bool,
        window_hidden: bool,
        backdrop: &dyn Backdrop,
    ) -> Option<GlassStatus> {
        if self.suspended.load(Ordering::Acquire) || window_hidden {
            // Never turn or keep the DWM backdrop on while the window is hidden or suspended.
            if *self
                .applied_backdrop
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                == Some(true)
            {
                let _ = (DedupBackdrop {
                    state: self,
                    inner: backdrop,
                })
                .set(false);
            }
            return None;
        }
        let env = Environment {
            system_effects: self.system_effects(),
            high_contrast,
        };
        let status = resolve(
            mode,
            env,
            &DedupBackdrop {
                state: self,
                inner: backdrop,
            },
        );
        self.replace(status).then_some(status)
    }

    fn apply_suspend(&self, backdrop: &dyn Backdrop) {
        self.suspended.store(true, Ordering::Release);
        let mut applied = self
            .applied_backdrop
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *applied != Some(false) {
            let _ = backdrop.set(false);
            *applied = Some(false);
        }
    }

    fn clear_suspended(&self) {
        self.suspended.store(false, Ordering::Release);
    }
}

impl Default for GlassState {
    fn default() -> Self {
        Self::new()
    }
}

struct DedupBackdrop<'a> {
    state: &'a GlassState,
    inner: &'a dyn Backdrop,
}

impl Backdrop for DedupBackdrop<'_> {
    fn set(&self, on: bool) -> Result<(), String> {
        let mut applied = self
            .state
            .applied_backdrop
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *applied == Some(on) {
            return Ok(());
        }
        match self.inner.set(on) {
            Ok(()) => {
                *applied = Some(on);
                Ok(())
            }
            Err(e) => {
                *applied = if on { None } else { Some(false) };
                Err(e)
            }
        }
    }
}

#[tauri::command]
pub fn get_glass_status(state: State<'_, GlassState>) -> GlassStatus {
    state.get()
}

/// Re-reads the choice and the cached OS preferences, applies the material to the window when it
/// differs from what is already applied, and tells the interface when the resolved status changed.
/// Safe to call at any time and from any thread; never spawns threads.
pub fn refresh(app: &AppHandle) {
    let mode = app
        .state::<crate::ui_settings::SettingsState>()
        .transparency();
    let window_hidden = app
        .get_webview_window("main")
        .is_some_and(|w| matches!(w.is_visible(), Ok(false)));
    let state = app.state::<GlassState>();
    if let Some(status) = state.apply_refresh(
        &mode,
        crate::system::high_contrast(),
        window_hidden,
        &WindowBackdrop { app },
    ) {
        let _ = app.emit(STATUS_EVENT, status);
    }
}

/// Clears the native DWM backdrop so Windows stops compositing Mica while the launcher window is
/// hidden, without sending a spurious status event to the suspended frontend.
pub fn suspend(app: &AppHandle) {
    app.state::<GlassState>()
        .apply_suspend(&WindowBackdrop { app });
}

/// Re-enables backdrop updates and refreshes the window material when the launcher window returns.
pub fn resume(app: &AppHandle) {
    app.state::<GlassState>().clear_suspended();
    refresh(app);
}

/// Reads the initial Windows transparency preference and calls `refresh` whenever Windows reports
/// that it changed.
pub fn watch_system_preference(app: AppHandle) {
    use windows::core::IInspectable;
    use windows::Foundation::TypedEventHandler;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    use windows::UI::ViewManagement::UISettings;

    let spawned = std::thread::Builder::new()
        .name("glass-watch".into())
        .spawn(move || {
            // SAFETY: initialises this thread for WinRT once; it lives as long as the process.
            if unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_err() {
                return;
            }
            let Ok(settings) = UISettings::new() else {
                return;
            };
            app.state::<GlassState>()
                .set_system_effects(settings.AdvancedEffectsEnabled().ok());
            refresh(&app);

            let handler_app = app.clone();
            let handler = TypedEventHandler::<UISettings, IInspectable>::new(move |sender, _| {
                if let Some(u) = sender.as_ref() {
                    handler_app
                        .state::<GlassState>()
                        .set_system_effects(u.AdvancedEffectsEnabled().ok());
                }
                refresh(&handler_app);
                Ok(())
            });
            if settings.AdvancedEffectsEnabledChanged(&handler).is_err() {
                return;
            }
            // Keep `settings` (and with it the subscription) alive for the whole run.
            loop {
                std::thread::park();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not watch the Windows transparency preference");
    }
}

/// Applies the material to the real window.
struct WindowBackdrop<'a> {
    app: &'a AppHandle,
}

/// The warm paper colour of the opaque look (the same as `backgroundColor` in tauri.conf.json).
const OPAQUE_BACKGROUND: tauri::window::Color = tauri::window::Color(0xF3, 0xED, 0xE3, 0xFF);
const CLEAR_BACKGROUND: tauri::window::Color = tauri::window::Color(0, 0, 0, 0);

impl Backdrop for WindowBackdrop<'_> {
    fn set(&self, on: bool) -> Result<(), String> {
        let window = self
            .app
            .get_webview_window("main")
            .ok_or("the main window does not exist yet")?;
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        native::set_backdrop(hwnd.0 as isize, on)?;
        window
            .set_background_color(Some(if on {
                CLEAR_BACKGROUND
            } else {
                OPAQUE_BACKGROUND
            }))
            .map_err(|e| e.to_string())
    }
}

mod native {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_NONE,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;

    /// Lets the window material reach the whole client area (-1) or takes it back (0).
    fn extend_frame(hwnd: isize, whole: bool) -> Result<(), String> {
        let m = -i32::from(whole);
        let margins = MARGINS {
            cxLeftWidth: m,
            cxRightWidth: m,
            cyTopHeight: m,
            cyBottomHeight: m,
        };
        // SAFETY: `margins` outlives the call; `hwnd` is the live main window.
        let result = unsafe { DwmExtendFrameIntoClientArea(hwnd as _, &margins) };
        if result >= 0 {
            Ok(())
        } else {
            Err(format!(
                "DwmExtendFrameIntoClientArea failed with {result:#010x}"
            ))
        }
    }

    fn set_attribute(hwnd: isize, attribute: i32, value: i32) -> Result<(), String> {
        // SAFETY: `value` outlives the call and its size is passed exactly; `hwnd` is the live main window.
        let result = unsafe {
            DwmSetWindowAttribute(
                hwnd as _,
                attribute as u32,
                std::ptr::from_ref(&value).cast(),
                std::mem::size_of::<i32>() as u32,
            )
        };
        if result >= 0 {
            Ok(())
        } else {
            Err(format!(
                "DwmSetWindowAttribute({attribute}) failed with {result:#010x}"
            ))
        }
    }

    /// Windows 11 22H2 or newer understands the Mica backdrop attribute (`DWMSBT_MAINWINDOW`);
    /// older versions refuse it, which is reported as an error and leaves Clade opaque.
    pub fn set_backdrop(hwnd: isize, on: bool) -> Result<(), String> {
        if on {
            // Clade is a light, warm interface: ask for the light variant of the material.
            set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, 0)?;
            extend_frame(hwnd, true)?;
            set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW)
        } else {
            let attr = set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE);
            let frame = extend_frame(hwnd, false);
            frame.and(attr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Recorder {
        calls: RefCell<Vec<bool>>,
        fail_on: bool,
    }

    impl Recorder {
        fn new(fail_on: bool) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                fail_on,
            }
        }
    }

    impl Backdrop for Recorder {
        fn set(&self, on: bool) -> Result<(), String> {
            self.calls.borrow_mut().push(on);
            if on && self.fail_on {
                Err("not supported on this Windows".into())
            } else {
                Ok(())
            }
        }
    }

    const fn env(system_effects: Option<bool>, high_contrast: bool) -> Environment {
        Environment {
            system_effects,
            high_contrast,
        }
    }

    #[test]
    fn system_follows_windows_and_on_off_are_explicit() {
        for (mode, windows, expected) in [
            ("system", Some(true), true),
            ("system", Some(false), false),
            ("system", None, false),
            ("on", Some(true), true),
            ("on", Some(false), true),
            ("on", None, true),
            ("off", Some(true), false),
            ("off", Some(false), false),
            ("off", None, false),
            ("garbage", Some(true), true),
            ("garbage", None, false),
        ] {
            assert_eq!(
                wanted(mode, env(windows, false)),
                expected,
                "{mode} {windows:?}"
            );
        }
    }

    #[test]
    fn high_contrast_always_means_opaque() {
        for mode in ["system", "on", "off"] {
            assert!(!wanted(mode, env(Some(true), true)), "{mode}");
        }
    }

    #[test]
    fn resolved_state_reports_whether_windows_really_draws_the_material() {
        let b = Recorder::new(false);
        assert_eq!(
            resolve("system", env(Some(true), false), &b),
            GlassStatus {
                active: true,
                live: true
            }
        );
        // an explicit "on" while Windows has transparency off: active, but Windows shows a solid colour
        assert_eq!(
            resolve("on", env(Some(false), false), &b),
            GlassStatus {
                active: true,
                live: false
            }
        );
        assert_eq!(*b.calls.borrow(), [true, true]);
    }

    #[test]
    fn off_clears_the_native_material_instead_of_leaving_it_active() {
        let b = Recorder::new(false);
        assert_eq!(
            resolve("off", env(Some(true), false), &b),
            GlassStatus::OPAQUE
        );
        assert_eq!(
            *b.calls.borrow(),
            [false],
            "the backdrop is explicitly cleared"
        );
    }

    #[test]
    fn a_backdrop_that_cannot_start_ends_in_the_opaque_look_and_is_cleaned_up() {
        let b = Recorder::new(true);
        assert_eq!(
            resolve("on", env(Some(true), false), &b),
            GlassStatus::OPAQUE
        );
        assert_eq!(
            *b.calls.borrow(),
            [true, false],
            "a failed start is rolled back"
        );
    }

    #[test]
    fn the_state_reports_changes_only_once() {
        let s = GlassState::new();
        assert_eq!(s.get(), GlassStatus::OPAQUE);
        assert!(!s.replace(GlassStatus::OPAQUE));
        let glass = GlassStatus {
            active: true,
            live: true,
        };
        assert!(s.replace(glass));
        assert!(!s.replace(glass));
        assert_eq!(s.get(), glass);
    }

    #[test]
    fn the_status_is_sent_in_camel_case() {
        let json = serde_json::to_value(GlassStatus {
            active: true,
            live: false,
        })
        .unwrap();
        assert_eq!(json["active"], true);
        assert_eq!(json["live"], false);
    }

    #[test]
    fn system_effects_preference_is_cached_atomically() {
        let s = GlassState::new();
        assert_eq!(s.system_effects(), None);
        s.set_system_effects(Some(true));
        assert_eq!(s.system_effects(), Some(true));
        s.set_system_effects(Some(false));
        assert_eq!(s.system_effects(), Some(false));
        s.set_system_effects(None);
        assert_eq!(s.system_effects(), None);
    }

    #[test]
    fn backdrop_calls_are_deduplicated_and_suspended_windows_never_reapply_mica() {
        let s = GlassState::new();
        s.set_system_effects(Some(true));
        let b = Recorder::new(false);

        // Initial refresh turns Mica on once; repeated refreshes (e.g. Focused(true)) skip DWM.
        assert_eq!(
            s.apply_refresh("system", false, false, &b),
            Some(GlassStatus {
                active: true,
                live: true
            })
        );
        assert_eq!(s.apply_refresh("system", false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [true]);

        // Suspending clears the native DWM backdrop without emitting a spurious OPAQUE event,
        // and refreshes while suspended or hidden never turn Mica back on.
        s.apply_suspend(&b);
        assert_eq!(*b.calls.borrow(), [true, false]);
        assert_eq!(
            s.get(),
            GlassStatus {
                active: true,
                live: true
            }
        );
        assert_eq!(s.apply_refresh("on", false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [true, false]);

        // Resuming while the window is still hidden keeps the backdrop off; once visible, Mica is
        // re-applied cleanly.
        s.clear_suspended();
        assert_eq!(s.apply_refresh("system", false, true, &b), None);
        assert_eq!(*b.calls.borrow(), [true, false]);

        assert_eq!(s.apply_refresh("system", false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [true, false, true]);
    }

    #[test]
    fn failed_backdrop_rollback_marks_applied_false_and_avoids_redundant_clear_calls() {
        let s = GlassState::new();
        s.set_system_effects(Some(true));
        let b = Recorder::new(true);

        assert_eq!(s.apply_refresh("on", false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [true, false]);

        // Subsequent off refreshes see `applied_backdrop == Some(false)` and skip DWM.
        assert_eq!(s.apply_refresh("off", false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [true, false]);
    }
}
