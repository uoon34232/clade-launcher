//! The optional translucent window material ("glass").
//!
//! Glass is purely cosmetic and must never get in the way: when anything about it is unavailable
//! or fails, Clade silently stays on the opaque warm treatment it has always had.
//!
//! **Technique (Windows 11):** the window keeps its normal, opaque WebView background. Turning
//! glass on asks the desktop window manager for its documented *Mica* system backdrop
//! (`DwmSetWindowAttribute` with `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW`, matching the
//! resolved theme via `DWMWA_USE_IMMERSIVE_DARK_MODE`) and makes the WebView's own background
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

use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::ThreadId;

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

/// The native side: turns the window material on or off for the resolved theme. A seam so the
/// decision logic is testable.
pub trait Backdrop {
    fn set(&self, on: bool, dark: bool) -> Result<(), String>;
}

/// Whether Windows' color value represents a dark background (official WinRT luminance threshold:
/// `5 * G + 2 * R + B < 8 * 128`).
pub const fn is_color_dark(r: u8, g: u8, b: u8) -> bool {
    5u32 * (g as u32) + 2u32 * (r as u32) + (b as u32) < 8 * 128
}

/// Resolves whether native chrome and opaque surfaces should use dark styling (`true` for dark,
/// `false` for light) given `ui.theme` (`system`, `light`, `dark`) and the cached OS preference.
pub fn wanted_dark(theme_mode: &str, system_dark: Option<bool>) -> bool {
    match theme_mode {
        "dark" => true,
        "light" => false,
        // "system", and anything unexpected: follow Windows; an unreadable preference stays light.
        _ => system_dark.unwrap_or(false),
    }
}

/// The opaque paper background colour matching the resolved theme.
pub const fn opaque_background(dark: bool) -> tauri::window::Color {
    if dark {
        OPAQUE_DARK_BACKGROUND
    } else {
        OPAQUE_LIGHT_BACKGROUND
    }
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
pub fn resolve(mode: &str, dark: bool, env: Environment, backdrop: &dyn Backdrop) -> GlassStatus {
    if !wanted(mode, env) {
        if let Err(e) = backdrop.set(false, dark) {
            tracing::warn!(error = %e, "could not clear the window backdrop");
        }
        return GlassStatus::OPAQUE;
    }
    match backdrop.set(true, dark) {
        Ok(()) => GlassStatus {
            active: true,
            live: env.system_effects.unwrap_or(false),
        },
        Err(e) => {
            tracing::warn!(error = %e, "the window backdrop is unavailable, using the opaque look");
            // Make sure nothing half-applied stays behind.
            let _ = backdrop.set(false, dark);
            GlassStatus::OPAQUE
        }
    }
}

const PREF_UNKNOWN: u8 = 0;
const PREF_OFF: u8 = 1;
const PREF_ON: u8 = 2;

const fn encode_opt_bool(v: Option<bool>) -> u8 {
    match v {
        None => PREF_UNKNOWN,
        Some(false) => PREF_OFF,
        Some(true) => PREF_ON,
    }
}

const fn decode_opt_bool(v: u8) -> Option<bool> {
    match v {
        PREF_OFF => Some(false),
        PREF_ON => Some(true),
        _ => None,
    }
}

/// Reentrant-safe serialization gate for `GlassState` transitions.
///
/// Cross-thread callers serialize in turn so `suspended`, `applied`, native DWM calls, `status`,
/// and `STATUS_EVENT` emission stay atomic and strictly ordered. Same-thread reentrancy increments
/// depth instead of deadlocking, while `GlassInner::seq` lets outer frames reconcile if a nested
/// transition ran mid-call.
struct SerialGate {
    state: Mutex<Option<(ThreadId, usize)>>,
    cvar: Condvar,
}

impl SerialGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(None),
            cvar: Condvar::new(),
        }
    }

    fn lock(&self) -> GateGuard<'_> {
        let me = std::thread::current().id();
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match *guard {
                None => {
                    *guard = Some((me, 1));
                    break;
                }
                Some((owner, depth)) if owner == me => {
                    *guard = Some((me, depth + 1));
                    break;
                }
                Some(_) => {
                    guard = self.cvar.wait(guard).unwrap_or_else(|e| e.into_inner());
                }
            }
        }
        GateGuard { gate: self }
    }
}

struct GateGuard<'a> {
    gate: &'a SerialGate,
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        let mut guard = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        let released = match *guard {
            Some((owner, depth)) if depth > 1 => {
                *guard = Some((owner, depth - 1));
                false
            }
            _ => {
                *guard = None;
                true
            }
        };
        drop(guard);
        if released {
            self.gate.cvar.notify_one();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppliedBackdrop {
    Unset,
    Applied { on: bool, dark: bool },
    Unknown,
}

struct GlassInner {
    applied: AppliedBackdrop,
    suspended: bool,
    seq: u64,
}

/// The resolved state, shared with the interface.
pub struct GlassState {
    status: Mutex<GlassStatus>,
    system_effects: AtomicU8,
    system_dark: AtomicU8,
    main_hwnd: AtomicIsize,
    gate: SerialGate,
    inner: Mutex<GlassInner>,
}

impl GlassState {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(GlassStatus::OPAQUE),
            system_effects: AtomicU8::new(PREF_UNKNOWN),
            system_dark: AtomicU8::new(PREF_UNKNOWN),
            main_hwnd: AtomicIsize::new(0),
            gate: SerialGate::new(),
            inner: Mutex::new(GlassInner {
                applied: AppliedBackdrop::Unset,
                suspended: false,
                seq: 0,
            }),
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
        self.system_effects
            .store(encode_opt_bool(enabled), Ordering::Release);
    }

    pub(crate) fn system_effects(&self) -> Option<bool> {
        decode_opt_bool(self.system_effects.load(Ordering::Acquire))
    }

    pub(crate) fn set_system_dark(&self, dark: Option<bool>) {
        self.system_dark
            .store(encode_opt_bool(dark), Ordering::Release);
    }

    pub(crate) fn system_dark(&self) -> Option<bool> {
        decode_opt_bool(self.system_dark.load(Ordering::Acquire))
    }

    fn cached_hwnd(&self) -> Option<isize> {
        let raw = self.main_hwnd.load(Ordering::Acquire);
        if raw == 0 {
            return None;
        }
        // SAFETY: `raw` came from `main`'s `HWND`; `IsWindow` queries handle validity without
        // sending messages to the window thread.
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(raw as _) } != 0 {
            Some(raw)
        } else {
            self.main_hwnd.store(0, Ordering::Release);
            None
        }
    }

    fn resolve_hwnd(&self, app: &AppHandle) -> Option<isize> {
        if let Some(hwnd) = self.cached_hwnd() {
            return Some(hwnd);
        }
        // Resolve outside `gate` so worker threads never wait on Tauri's main-thread getter while
        // the UI thread may be waiting on `gate`.
        let window = app.get_webview_window("main")?;
        let hwnd = window.hwnd().ok()?.0 as isize;
        if hwnd != 0 {
            self.main_hwnd.store(hwnd, Ordering::Release);
            Some(hwnd)
        } else {
            None
        }
    }

    /// Resolves and applies the backdrop if the window is active, deduplicating redundant native
    /// DWM calls. Returns `Some(status)` only when the UI-facing status changed.
    fn apply_refresh(
        &self,
        mode: &str,
        dark: bool,
        high_contrast: bool,
        window_hidden: bool,
        backdrop: &dyn Backdrop,
    ) -> Option<GlassStatus> {
        let _gate = self.gate.lock();
        let (my_seq, suspended, applied) = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.seq = inner.seq.wrapping_add(1);
            (inner.seq, inner.suspended, inner.applied)
        };
        if suspended || window_hidden {
            // Never turn or keep the DWM backdrop on while the window is hidden or suspended.
            if matches!(
                applied,
                AppliedBackdrop::Applied { on: true, .. } | AppliedBackdrop::Unknown
            ) {
                let _ = (DedupBackdrop {
                    state: self,
                    inner: backdrop,
                })
                .set(false, dark);
            }
            return None;
        }
        let env = Environment {
            system_effects: self.system_effects(),
            high_contrast,
        };
        let status = resolve(
            mode,
            dark,
            env,
            &DedupBackdrop {
                state: self,
                inner: backdrop,
            },
        );
        let (superseded, now_suspended, now_applied) = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            (inner.seq != my_seq, inner.suspended, inner.applied)
        };
        if now_suspended {
            if matches!(now_applied, AppliedBackdrop::Applied { on: true, .. }) {
                let _ = (DedupBackdrop {
                    state: self,
                    inner: backdrop,
                })
                .set(false, dark);
            }
            return None;
        }
        if superseded {
            return None;
        }
        self.replace(status).then_some(status)
    }

    fn apply_suspend(&self, dark: bool, backdrop: &dyn Backdrop) {
        let _gate = self.gate.lock();
        let (my_seq, needs_clear) = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.seq = inner.seq.wrapping_add(1);
            inner.suspended = true;
            let needs_clear = !matches!(
                inner.applied,
                AppliedBackdrop::Applied { on: false, dark: d } if d == dark
            );
            (inner.seq, needs_clear)
        };
        if !needs_clear {
            return;
        }
        let result = backdrop.set(false, dark);
        let (same_seq, suspended, applied) = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.seq == my_seq {
                inner.applied = match &result {
                    Ok(()) => AppliedBackdrop::Applied { on: false, dark },
                    Err(e) => {
                        tracing::warn!(error = %e, "could not clear the window backdrop on suspend");
                        AppliedBackdrop::Unknown
                    }
                };
                return;
            }
            (false, inner.suspended, inner.applied)
        };
        debug_assert!(!same_seq);
        if !suspended {
            if let AppliedBackdrop::Applied {
                on: true,
                dark: next_dark,
            } = applied
            {
                let reapply = backdrop.set(true, next_dark);
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if !inner.suspended
                    && matches!(inner.applied, AppliedBackdrop::Applied { on: true, .. })
                {
                    inner.applied = if reapply.is_ok() {
                        AppliedBackdrop::Applied {
                            on: true,
                            dark: next_dark,
                        }
                    } else {
                        AppliedBackdrop::Unknown
                    };
                }
                return;
            }
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.suspended || !matches!(inner.applied, AppliedBackdrop::Applied { on: true, .. }) {
            inner.applied = match &result {
                Ok(()) => AppliedBackdrop::Applied { on: false, dark },
                Err(e) => {
                    tracing::warn!(error = %e, "could not clear the window backdrop on suspend");
                    AppliedBackdrop::Unknown
                }
            };
        }
    }

    fn clear_suspended(&self) {
        let _gate = self.gate.lock();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.seq = inner.seq.wrapping_add(1);
        inner.suspended = false;
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
    fn set(&self, on: bool, dark: bool) -> Result<(), String> {
        let call_seq = {
            let inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            let already = match inner.applied {
                AppliedBackdrop::Applied {
                    on: a_on,
                    dark: a_dark,
                } => a_on == on && a_dark == dark,
                AppliedBackdrop::Unset | AppliedBackdrop::Unknown => false,
            };
            if already {
                return Ok(());
            }
            inner.seq
        };
        let result = self.inner.set(on, dark);
        let (suspended, applied) = {
            let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.seq == call_seq {
                inner.applied = match &result {
                    Ok(()) => AppliedBackdrop::Applied { on, dark },
                    Err(_) => AppliedBackdrop::Unknown,
                };
                return result;
            }
            (inner.suspended, inner.applied)
        };
        // A nested reentrant transition ran while `self.inner.set(on, dark)` was in flight; ensure
        // the physical window matches the newer transition.
        if on && (suspended || !matches!(applied, AppliedBackdrop::Applied { on: true, .. })) {
            let rollback_dark = match applied {
                AppliedBackdrop::Applied { dark: d, .. } => d,
                _ => dark,
            };
            let rollback = self.inner.set(false, rollback_dark);
            let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.suspended
                || !matches!(inner.applied, AppliedBackdrop::Applied { on: true, .. })
            {
                inner.applied = if rollback.is_ok() {
                    AppliedBackdrop::Applied {
                        on: false,
                        dark: rollback_dark,
                    }
                } else {
                    AppliedBackdrop::Unknown
                };
            }
        } else if !on && !suspended {
            if let AppliedBackdrop::Applied {
                on: true,
                dark: next_dark,
            } = applied
            {
                let reapply = self.inner.set(true, next_dark);
                let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                if !inner.suspended
                    && matches!(
                        inner.applied,
                        AppliedBackdrop::Applied { on: true, dark: d } if d == next_dark
                    )
                {
                    inner.applied = if reapply.is_ok() {
                        AppliedBackdrop::Applied {
                            on: true,
                            dark: next_dark,
                        }
                    } else {
                        AppliedBackdrop::Unknown
                    };
                }
            } else {
                let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.applied = match &result {
                    Ok(()) => AppliedBackdrop::Applied { on, dark },
                    Err(_) => AppliedBackdrop::Unknown,
                };
            }
        } else {
            let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.applied = match &result {
                Ok(()) => AppliedBackdrop::Applied { on, dark },
                Err(_) => AppliedBackdrop::Unknown,
            };
        }
        result
    }
}

#[tauri::command]
pub fn get_glass_status(state: State<'_, GlassState>) -> GlassStatus {
    state.get()
}

/// Returns whether Clade's active theme resolves to dark right now (used for native surfaces).
pub fn is_dark(app: &AppHandle) -> bool {
    let settings = app.state::<crate::ui_settings::SettingsState>();
    let glass = app.state::<GlassState>();
    wanted_dark(&settings.theme(), glass.system_dark())
}

fn refresh_locked(app: &AppHandle, state: &GlassState, hwnd: Option<isize>) {
    let settings = app.state::<crate::ui_settings::SettingsState>();
    let mode = settings.transparency();
    let dark = wanted_dark(&settings.theme(), state.system_dark());
    let window_hidden = hwnd.is_some_and(|h| {
        // SAFETY: `h` was verified by `resolve_hwnd`; `IsWindowVisible` reads window visibility
        // flags directly without dispatching messages to the UI thread.
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(h as _) == 0 }
    });
    if let Some(status) = state.apply_refresh(
        &mode,
        dark,
        crate::system::high_contrast(),
        window_hidden,
        &WindowBackdrop { app, hwnd },
    ) {
        let _ = app.emit(STATUS_EVENT, status);
    }
}

/// Re-reads the choice and the cached OS preferences, applies the material to the window when it
/// differs from what is already applied, and tells the interface when the resolved status changed.
/// Safe to call at any time and from any thread; never spawns threads.
pub fn refresh(app: &AppHandle) {
    let state = app.state::<GlassState>();
    let hwnd = state.resolve_hwnd(app);
    let _gate = state.gate.lock();
    refresh_locked(app, &state, hwnd);
}

/// Clears the native DWM backdrop so Windows stops compositing Mica while the launcher window is
/// hidden, without sending a spurious status event to the suspended frontend.
pub fn suspend(app: &AppHandle) {
    let state = app.state::<GlassState>();
    let hwnd = state.resolve_hwnd(app);
    let dark = is_dark(app);
    let _gate = state.gate.lock();
    state.apply_suspend(dark, &WindowBackdrop { app, hwnd });
}

/// Re-enables backdrop updates and refreshes the window material when the launcher window returns.
pub fn resume(app: &AppHandle) {
    let state = app.state::<GlassState>();
    let hwnd = state.resolve_hwnd(app);
    let _gate = state.gate.lock();
    state.clear_suspended();
    refresh_locked(app, &state, hwnd);
}

/// Starts the permanent `glass-watch` WinRT MTA thread to read the initial Windows transparency
/// and theme preferences (returning the OS dark-mode flag once ready, or `None` on timeout/error)
/// and call `refresh` whenever either changes. Never calls `RoUninitialize` because `windows-core`
/// caches activation factories process-wide.
pub fn watch_system_preference(app: AppHandle) -> Option<bool> {
    use windows::core::IInspectable;
    use windows::Foundation::TypedEventHandler;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    use windows::UI::ViewManagement::{UIColorType, UISettings};

    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Option<bool>>(1);
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
            let initial_effects = settings.AdvancedEffectsEnabled().ok();
            let initial_dark = settings
                .GetColorValue(UIColorType::Background)
                .ok()
                .map(|c| is_color_dark(c.R, c.G, c.B));
            let glass = app.state::<GlassState>();
            glass.set_system_effects(initial_effects);
            glass.set_system_dark(initial_dark);
            let _ = ready_tx.send(initial_dark);
            drop(ready_tx);

            let initial_app = app.clone();
            let _ = app.run_on_main_thread(move || refresh(&initial_app));

            let effects_app = app.clone();
            let effects_handler =
                TypedEventHandler::<UISettings, IInspectable>::new(move |sender, _| {
                    if let Some(u) = sender.as_ref() {
                        effects_app
                            .state::<GlassState>()
                            .set_system_effects(u.AdvancedEffectsEnabled().ok());
                    }
                    let ui_app = effects_app.clone();
                    let _ = effects_app.run_on_main_thread(move || refresh(&ui_app));
                    Ok(())
                });
            let color_app = app.clone();
            let color_handler =
                TypedEventHandler::<UISettings, IInspectable>::new(move |sender, _| {
                    if let Some(u) = sender.as_ref() {
                        let dark = u
                            .GetColorValue(UIColorType::Background)
                            .ok()
                            .map(|c| is_color_dark(c.R, c.G, c.B));
                        color_app.state::<GlassState>().set_system_dark(dark);
                    }
                    let ui_app = color_app.clone();
                    let _ = color_app.run_on_main_thread(move || refresh(&ui_app));
                    Ok(())
                });
            let effects_ok = settings
                .AdvancedEffectsEnabledChanged(&effects_handler)
                .is_ok();
            let color_ok = settings.ColorValuesChanged(&color_handler).is_ok();
            if !effects_ok && !color_ok {
                return;
            }
            // Keep `settings` (and with it the subscriptions and MTA) alive for the whole run.
            loop {
                std::thread::park();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not watch the Windows appearance preference");
        return None;
    }
    ready_rx
        .recv_timeout(std::time::Duration::from_millis(150))
        .ok()
        .flatten()
}

/// Applies the material to the real window.
struct WindowBackdrop<'a> {
    app: &'a AppHandle,
    hwnd: Option<isize>,
}

/// The warm paper colours of the opaque look (light matches `backgroundColor` in tauri.conf.json).
pub const OPAQUE_LIGHT_BACKGROUND: tauri::window::Color =
    tauri::window::Color(0xF3, 0xED, 0xE3, 0xFF);
pub const OPAQUE_DARK_BACKGROUND: tauri::window::Color =
    tauri::window::Color(0x1C, 0x19, 0x17, 0xFF);
const CLEAR_BACKGROUND: tauri::window::Color = tauri::window::Color(0, 0, 0, 0);

fn apply_backdrop_transition(
    on: bool,
    dark: bool,
    apply_native: impl FnOnce(bool, bool) -> Result<(), String>,
    apply_background: impl FnOnce(tauri::window::Color) -> Result<(), String>,
) -> Result<(), String> {
    if on {
        apply_native(true, dark)?;
        apply_background(CLEAR_BACKGROUND)
    } else {
        // Paint the opaque paper fill before tearing down Mica so no 1-frame transparent flash shows.
        let bg_res = apply_background(opaque_background(dark));
        let native_res = apply_native(false, dark);
        match (native_res, bg_res) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
            (Err(native_err), Err(bg_err)) => Err(format!("{native_err}; {bg_err}")),
        }
    }
}

impl Backdrop for WindowBackdrop<'_> {
    fn set(&self, on: bool, dark: bool) -> Result<(), String> {
        let window = self
            .app
            .get_webview_window("main")
            .ok_or("the main window does not exist yet")?;
        apply_backdrop_transition(
            on,
            dark,
            |enable, is_dark| {
                let hwnd = self.hwnd.ok_or("the main window handle is unavailable")?;
                native::set_backdrop(hwnd, enable, is_dark)
            },
            |color| {
                window
                    .set_background_color(Some(color))
                    .map_err(|e| e.to_string())
            },
        )
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
    pub fn set_backdrop(hwnd: isize, on: bool, dark: bool) -> Result<(), String> {
        let dark_val = i32::from(dark);
        if on {
            set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, dark_val)?;
            extend_frame(hwnd, true)?;
            set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW)
        } else {
            let _ = set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, dark_val);
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
    use std::sync::atomic::AtomicBool;

    struct Recorder {
        calls: RefCell<Vec<(bool, bool)>>,
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
        fn set(&self, on: bool, dark: bool) -> Result<(), String> {
            self.calls.borrow_mut().push((on, dark));
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
            resolve("system", false, env(Some(true), false), &b),
            GlassStatus {
                active: true,
                live: true
            }
        );
        // an explicit "on" while Windows has transparency off: active, but Windows shows a solid colour
        assert_eq!(
            resolve("on", true, env(Some(false), false), &b),
            GlassStatus {
                active: true,
                live: false
            }
        );
        assert_eq!(*b.calls.borrow(), [(true, false), (true, true)]);
    }

    #[test]
    fn off_clears_the_native_material_instead_of_leaving_it_active() {
        let b = Recorder::new(false);
        assert_eq!(
            resolve("off", true, env(Some(true), false), &b),
            GlassStatus::OPAQUE
        );
        assert_eq!(
            *b.calls.borrow(),
            [(false, true)],
            "the backdrop is explicitly cleared with the active dark preference"
        );
    }

    #[test]
    fn a_backdrop_that_cannot_start_ends_in_the_opaque_look_and_is_cleaned_up() {
        let b = Recorder::new(true);
        assert_eq!(
            resolve("on", false, env(Some(true), false), &b),
            GlassStatus::OPAQUE
        );
        assert_eq!(
            *b.calls.borrow(),
            [(true, false), (false, false)],
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
    fn system_dark_preference_is_cached_atomically_and_color_threshold_matches_winrt() {
        let s = GlassState::new();
        assert_eq!(s.system_dark(), None);
        s.set_system_dark(Some(true));
        assert_eq!(s.system_dark(), Some(true));
        s.set_system_dark(Some(false));
        assert_eq!(s.system_dark(), Some(false));
        s.set_system_dark(None);
        assert_eq!(s.system_dark(), None);

        assert!(is_color_dark(0, 0, 0));
        assert!(is_color_dark(28, 25, 23));
        assert!(!is_color_dark(255, 255, 255));
        assert!(!is_color_dark(243, 237, 227));

        assert!(wanted_dark("dark", Some(false)));
        assert!(!wanted_dark("light", Some(true)));
        assert!(wanted_dark("system", Some(true)));
        assert!(!wanted_dark("system", Some(false)));
        assert!(!wanted_dark("system", None));
        assert!(wanted_dark("unknown", Some(true)));
        assert_eq!(opaque_background(false), OPAQUE_LIGHT_BACKGROUND);
        assert_eq!(opaque_background(true), OPAQUE_DARK_BACKGROUND);
    }

    #[test]
    fn backdrop_calls_are_deduplicated_and_reapply_when_theme_flips() {
        let s = GlassState::new();
        s.set_system_effects(Some(true));
        let b = Recorder::new(false);

        // Initial light refresh turns Mica on once; repeated light refreshes skip DWM.
        assert_eq!(
            s.apply_refresh("system", false, false, false, &b),
            Some(GlassStatus {
                active: true,
                live: true
            })
        );
        assert_eq!(s.apply_refresh("system", false, false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [(true, false)]);

        // Switching to dark theme while glass stays active reapplies DWM with dark = true, without
        // emitting a redundant GlassStatus event because active/live didn't change.
        assert_eq!(s.apply_refresh("system", true, false, false, &b), None);
        assert_eq!(s.apply_refresh("system", true, false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [(true, false), (true, true)]);

        // Switching to opaque dark also reapplies native background / DWM dark attribute cleanly.
        assert_eq!(
            s.apply_refresh("off", true, false, false, &b),
            Some(GlassStatus::OPAQUE)
        );
        assert_eq!(s.apply_refresh("off", false, false, false, &b), None);
        assert_eq!(
            *b.calls.borrow(),
            [(true, false), (true, true), (false, true), (false, false)]
        );
    }

    #[test]
    fn suspended_windows_never_reapply_mica() {
        let s = GlassState::new();
        s.set_system_effects(Some(true));
        let b = Recorder::new(false);

        assert_eq!(
            s.apply_refresh("system", false, false, false, &b),
            Some(GlassStatus {
                active: true,
                live: true
            })
        );
        s.apply_suspend(false, &b);
        assert_eq!(*b.calls.borrow(), [(true, false), (false, false)]);
        assert_eq!(
            s.get(),
            GlassStatus {
                active: true,
                live: true
            }
        );
        assert_eq!(s.apply_refresh("on", true, false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [(true, false), (false, false)]);

        s.clear_suspended();
        assert_eq!(s.apply_refresh("system", false, false, true, &b), None);
        assert_eq!(*b.calls.borrow(), [(true, false), (false, false)]);

        assert_eq!(s.apply_refresh("system", false, false, false, &b), None);
        assert_eq!(
            *b.calls.borrow(),
            [(true, false), (false, false), (true, false)]
        );
    }

    #[test]
    fn failed_backdrop_rollback_marks_applied_false_and_avoids_redundant_clear_calls() {
        let s = GlassState::new();
        s.set_system_effects(Some(true));
        let b = Recorder::new(true);

        assert_eq!(s.apply_refresh("on", false, false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [(true, false), (false, false)]);

        // Subsequent off refreshes with the same dark state skip DWM.
        assert_eq!(s.apply_refresh("off", false, false, false, &b), None);
        assert_eq!(*b.calls.borrow(), [(true, false), (false, false)]);
    }

    #[test]
    fn backdrop_set_releases_mutex_before_native_call() {
        struct ReentrantBackdrop<'a> {
            state: &'a GlassState,
            reentered: AtomicBool,
            calls: RefCell<Vec<(bool, bool)>>,
        }

        impl Backdrop for ReentrantBackdrop<'_> {
            fn set(&self, on: bool, dark: bool) -> Result<(), String> {
                self.calls.borrow_mut().push((on, dark));
                if !self.reentered.swap(true, Ordering::AcqRel) {
                    // Re-entering `GlassState` while the native backdrop call is in flight must not deadlock.
                    let _ = self.state.apply_refresh("system", dark, false, false, self);
                    self.state.apply_suspend(dark, self);
                }
                Ok(())
            }
        }

        let s = GlassState::new();
        s.set_system_effects(Some(true));
        let b = ReentrantBackdrop {
            state: &s,
            reentered: AtomicBool::new(false),
            calls: RefCell::new(Vec::new()),
        };

        let _ = s.apply_refresh("system", false, false, false, &b);
        assert!(b.reentered.load(Ordering::Acquire));
        // Outer `set(true, false)` -> nested `set(true, false)` -> nested suspend `set(false, false)`
        // -> outer `DedupBackdrop::set` reconciles the physical window back to `false` once outer returns.
        assert_eq!(
            *b.calls.borrow(),
            [(true, false), (true, false), (false, false), (false, false)]
        );
    }
}
