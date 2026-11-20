//! Native framed game embedding ("Inside Clade").
//!
//! When `"When Minecraft starts"` is `"embed"`, Clade keeps its own frameless window open as a host
//! frame (title strip with Minimize / Maximize / Close + warm background padding) and reparents the
//! running Minecraft window directly inside the launcher's central stage region:
//! - On Windows, via Win32 `SetParent`, child window styles, `SetWindowPos`, and `CreateRoundRectRgn`.
//! - On Linux (X11 / XWayland), via `libX11.so.6` (`XReparentWindow`, `XMoveResizeWindow`) and
//!   `libXext.so.6` (`XShapeCombineMask`) loaded dynamically with `libc::dlopen`.
//!
//! No game frames are ever copied through the WebView: Java/OpenGL renders straight to its native
//! surface and keeps raw mouse/keyboard handling.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State};

/// Event announcing a change to the framed game embedding state.
pub const EMBED_EVENT: &str = "game-embed-state";

const POLL_IDLE: Duration = Duration::from_millis(80);
const POLL_ACTIVE: Duration = Duration::from_millis(24);
const FALLBACK_TOP_PX: i32 = 44;
const FALLBACK_MARGIN_PX: i32 = 14;
const FALLBACK_RADIUS_PX: u32 = 16;

/// Physical stage rectangle inside Clade's main window where the game window sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub radius: u32,
}

impl StageBounds {
    pub fn clamped(x: i32, y: i32, width: u32, height: u32, radius: u32) -> Self {
        let w = width.clamp(64, 16_384);
        let h = height.clamp(64, 16_384);
        let r = radius.min(w.min(h) / 2);
        Self {
            x,
            y,
            width: w,
            height: h,
            radius: r,
        }
    }

    fn fallback_from_window(w: &tauri::WebviewWindow) -> Self {
        let scale = w.scale_factor().unwrap_or(1.0).clamp(0.5, 4.0);
        let size = w
            .inner_size()
            .unwrap_or_else(|_| PhysicalSize::new(1280, 720));
        let top = (f64::from(FALLBACK_TOP_PX) * scale).round() as i32;
        let margin = (f64::from(FALLBACK_MARGIN_PX) * scale).round() as i32;
        let radius = (f64::from(FALLBACK_RADIUS_PX) * scale).round().max(0.0) as u32;
        let width = (size.width as i32 - margin * 2).max(320) as u32;
        let height = (size.height as i32 - top - margin).max(240) as u32;
        Self::clamped(margin, top, width, height, radius)
    }
}

/// UI-facing embedding snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEmbedStatus {
    pub active: bool,
    pub instance_id: Option<String>,
    pub maximized: bool,
}

struct ActiveSession {
    instance_id: String,
    pid: u32,
    game_win: AtomicU64,
    attached: AtomicBool,
    maximized: AtomicBool,
    restore_size: Mutex<Option<PhysicalSize<u32>>>,
    restore_pos: Mutex<Option<PhysicalPosition<i32>>>,
    close_requested: AtomicBool,
    close_attempts: AtomicU8,
    focus_requested: AtomicBool,
    stop: Arc<AtomicBool>,
}

impl ActiveSession {
    fn new(instance_id: String, pid: u32) -> Self {
        Self {
            instance_id,
            pid,
            game_win: AtomicU64::new(0),
            attached: AtomicBool::new(false),
            maximized: AtomicBool::new(false),
            restore_size: Mutex::new(None),
            restore_pos: Mutex::new(None),
            close_requested: AtomicBool::new(false),
            close_attempts: AtomicU8::new(0),
            focus_requested: AtomicBool::new(false),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn status(&self) -> GameEmbedStatus {
        if !self.attached.load(Ordering::Acquire) {
            return GameEmbedStatus::default();
        }
        GameEmbedStatus {
            active: true,
            instance_id: Some(self.instance_id.clone()),
            maximized: self.maximized.load(Ordering::Acquire),
        }
    }
}

/// Snapshot saved when detaching a framed game window so host geometry can be restored cleanly.
struct FinishedEmbed {
    was_attached: bool,
    restore_size: Option<PhysicalSize<u32>>,
    restore_pos: Option<PhysicalPosition<i32>>,
}

/// State managing framed game attachment and stage coordinates.
pub struct EmbedState {
    bounds: Mutex<Option<StageBounds>>,
    bounds_seq: AtomicU64,
    active: Mutex<Option<Arc<ActiveSession>>>,
}

impl EmbedState {
    pub fn new() -> Self {
        Self {
            bounds: Mutex::new(None),
            bounds_seq: AtomicU64::new(0),
            active: Mutex::new(None),
        }
    }

    pub fn set_bounds(&self, bounds: StageBounds) {
        *self.bounds.lock().unwrap_or_else(|e| e.into_inner()) = Some(bounds);
        self.bounds_seq.fetch_add(1, Ordering::Release);
    }

    pub fn current_bounds(&self) -> (Option<StageBounds>, u64) {
        let seq = self.bounds_seq.load(Ordering::Acquire);
        let bounds = *self.bounds.lock().unwrap_or_else(|e| e.into_inner());
        (bounds, seq)
    }

    pub fn status(&self) -> GameEmbedStatus {
        let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map_or_else(GameEmbedStatus::default, |s| s.status())
    }

    pub fn is_active(&self) -> bool {
        self.status().active
    }

    fn begin(&self, instance_id: String, pid: u32) -> Arc<ActiveSession> {
        let next = Arc::new(ActiveSession::new(instance_id, pid));
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = guard.replace(next.clone()) {
            prev.stop.store(true, Ordering::Release);
        }
        next
    }

    fn current_session(&self) -> Option<Arc<ActiveSession>> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn finish(&self, instance_id: &str) -> Option<FinishedEmbed> {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let matches = guard.as_ref().is_some_and(|s| s.instance_id == instance_id);
        if !matches {
            return None;
        }
        let session = guard.take()?;
        session.stop.store(true, Ordering::Release);
        let was_attached = session.attached.swap(false, Ordering::AcqRel);
        let restore_size = *session
            .restore_size
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let restore_pos = *session
            .restore_pos
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Some(FinishedEmbed {
            was_attached,
            restore_size,
            restore_pos,
        })
    }
}

impl Default for EmbedState {
    fn default() -> Self {
        Self::new()
    }
}

/// Native window identifier (`HWND` on Windows, X11 `Window` ID on Linux) if embedding is possible.
pub fn host_window_id(window: &tauri::WebviewWindow) -> Option<u64> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        #[cfg(windows)]
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as usize as u64),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(h) if h.window != 0 && x11::is_available() => Some(h.window),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(h) if x11::is_available() => Some(u64::from(h.window.get())),
        _ => None,
    }
}

/// True when the current main window exposes a native handle supported for game embedding.
pub fn can_embed(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| host_window_id(&w))
        .is_some()
}

pub fn is_active(app: &AppHandle) -> bool {
    app.try_state::<EmbedState>().is_some_and(|s| s.is_active())
}

#[tauri::command]
pub fn get_game_embed_status(state: State<'_, EmbedState>) -> GameEmbedStatus {
    state.status()
}

#[tauri::command]
pub fn set_game_stage_bounds(
    state: State<'_, EmbedState>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    radius: u32,
) {
    state.set_bounds(StageBounds::clamped(x, y, width, height, radius));
}

#[tauri::command]
pub fn toggle_game_maximize(app: AppHandle, state: State<'_, EmbedState>) -> GameEmbedStatus {
    let Some(session) = state.current_session() else {
        return GameEmbedStatus::default();
    };
    if !session.attached.load(Ordering::Acquire) {
        return GameEmbedStatus::default();
    }
    if let Some(w) = app.get_webview_window("main") {
        let cur = w
            .is_maximized()
            .unwrap_or_else(|_| session.maximized.load(Ordering::Acquire));
        let next = !cur;
        if next {
            let _ = w.maximize();
        } else {
            let _ = w.unmaximize();
        }
        session.maximized.store(next, Ordering::Release);
    }
    let status = session.status();
    let _ = app.emit(EMBED_EVENT, &status);
    status
}

/// Called when the host window receives focus while a framed game is running.
pub fn on_host_focused(app: &AppHandle) {
    if let Some(session) = app
        .try_state::<EmbedState>()
        .and_then(|s| s.current_session())
    {
        if session.attached.load(Ordering::Acquire) {
            session.focus_requested.store(true, Ordering::Release);
        }
    }
}

/// Keeps `maximized` in sync with native window state when the host resizes during a framed game.
pub fn on_host_resized(app: &AppHandle) {
    let Some(state) = app.try_state::<EmbedState>() else {
        return;
    };
    let Some(session) = state.current_session() else {
        return;
    };
    if !session.attached.load(Ordering::Acquire) {
        return;
    }
    if let Some(w) = app.get_webview_window("main") {
        if let Ok(actual) = w.is_maximized() {
            let prev = session.maximized.swap(actual, Ordering::AcqRel);
            if prev != actual {
                let _ = app.emit(EMBED_EVENT, session.status());
            }
        }
    }
}

/// Sends a graceful close signal to the embedded game window on the first close click.
/// Returns `true` if `CloseRequested` should be prevented so the game can save & close cleanly
/// inside the frame; a second close click falls through to Clade's background-exit handling.
pub fn request_close_or_detach(app: &AppHandle) -> bool {
    let Some(session) = app
        .try_state::<EmbedState>()
        .and_then(|s| s.current_session())
    else {
        return false;
    };
    if !session.attached.load(Ordering::Acquire) || session.game_win.load(Ordering::Acquire) == 0 {
        return false;
    }
    if session.close_attempts.fetch_add(1, Ordering::AcqRel) == 0 {
        session.close_requested.store(true, Ordering::Release);
        return true;
    }
    false
}

/// Starts monitoring a newly launched game process (`pid`) to embed its window into Clade's stage.
pub fn start_session(app: AppHandle, instance_id: String, pid: u32) {
    if pid == 0 {
        return;
    }
    let Some(host_win) = app.get_webview_window("main") else {
        return;
    };
    let Some(host_id) = host_window_id(&host_win) else {
        tracing::info!(
            "framed game mode unavailable on this display handle; keeping standalone window"
        );
        return;
    };
    let state = app.state::<EmbedState>();
    let session = state.begin(instance_id, pid);

    let thread_name = "clade-game-embed".to_owned();
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            #[cfg(windows)]
            run_watcher_win32(app, session, host_id);
            #[cfg(target_os = "linux")]
            run_watcher_x11(app, session, host_id);
            #[cfg(not(any(windows, target_os = "linux")))]
            let _ = (app, session, host_id);
        });
}

/// Tears down framed mode when the game finishes, returning Clade's window to its menu geometry.
pub fn end_session(app: &AppHandle, instance_id: &str) {
    let Some(state) = app.try_state::<EmbedState>() else {
        return;
    };
    let Some(ended) = state.finish(instance_id) else {
        return;
    };
    let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
    if ended.was_attached {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.unmaximize();
            let _ = w.set_maximizable(false);
            if let Some(sz) = ended.restore_size {
                let _ = w.set_size(tauri::Size::Physical(sz));
            }
            if let Some(pos) = ended.restore_pos {
                let _ = w.set_position(tauri::Position::Physical(pos));
            }
        }
    }
}

fn mark_attached(app: &AppHandle, session: &ActiveSession, win_id: u64) {
    session.game_win.store(win_id, Ordering::Release);
    if !session.attached.swap(true, Ordering::AcqRel) {
        if let Some(w) = app.get_webview_window("main") {
            if let Ok(sz) = w.inner_size() {
                *session
                    .restore_size
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(sz);
            }
            if let Ok(pos) = w.outer_position() {
                *session
                    .restore_pos
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(pos);
            }
            let _ = w.set_resizable(true);
            let _ = w.set_maximizable(true);
        }
        let _ = app.emit(EMBED_EVENT, session.status());
    }
}

fn active_bounds(app: &AppHandle) -> (StageBounds, u64) {
    let state = app.state::<EmbedState>();
    let (maybe, seq) = state.current_bounds();
    let bounds = maybe.unwrap_or_else(|| {
        app.get_webview_window("main")
            .map(|w| StageBounds::fallback_from_window(&w))
            .unwrap_or_else(|| StageBounds::clamped(14, 44, 1252, 662, 16))
    });
    (bounds, seq)
}

#[cfg(windows)]
fn run_watcher_win32(app: AppHandle, session: Arc<ActiveSession>, host_id: u64) {
    let host_hwnd = host_id as usize as win32::HWND;
    let mut last_seq = u64::MAX;

    while !session.stop.load(Ordering::Acquire) {
        let mut mc_id = session.game_win.load(Ordering::Acquire);
        if mc_id != 0 && !win32::is_valid(mc_id as usize as win32::HWND) {
            mc_id = 0;
            session.game_win.store(0, Ordering::Release);
            last_seq = u64::MAX;
        }

        if mc_id == 0 {
            if let Some(found) = win32::find_game_window(session.pid, host_hwnd) {
                let (bounds, seq) = active_bounds(&app);
                if win32::attach_and_place(found, host_hwnd, bounds) {
                    mark_attached(&app, &session, found as usize as u64);
                    last_seq = seq;
                }
            }
            std::thread::sleep(POLL_IDLE);
            continue;
        }

        let mc_hwnd = mc_id as usize as win32::HWND;
        let (bounds, seq) = active_bounds(&app);
        if seq != last_seq {
            win32::apply_bounds(mc_hwnd, bounds);
            last_seq = seq;
        }
        if session.focus_requested.swap(false, Ordering::AcqRel) {
            win32::focus_window(mc_hwnd);
        }
        if session.close_requested.swap(false, Ordering::AcqRel) {
            win32::request_close(mc_hwnd);
        }
        std::thread::sleep(POLL_ACTIVE);
    }
}

#[cfg(target_os = "linux")]
fn run_watcher_x11(app: AppHandle, session: Arc<ActiveSession>, host_id: u64) {
    let Some(api) = x11::api() else {
        return;
    };
    let Some(dpy) = api.open_display() else {
        tracing::warn!("could not open X11 display for framed game mode");
        return;
    };
    let host_xid = host_id as x11::Window;
    let mut last_seq = u64::MAX;
    let mut last_pids_refresh = std::time::Instant::now()
        .checked_sub(Duration::from_secs(10))
        .unwrap_or_else(std::time::Instant::now);
    let mut pids = std::collections::HashSet::new();

    while !session.stop.load(Ordering::Acquire) {
        let cur_xid = session.game_win.load(Ordering::Acquire) as x11::Window;
        let mut invalidated = false;

        api.drain_events(dpy, cur_xid, &mut invalidated);
        if invalidated && cur_xid != 0 {
            session.game_win.store(0, Ordering::Release);
            last_seq = u64::MAX;
        }

        let mc_xid = session.game_win.load(Ordering::Acquire) as x11::Window;
        if mc_xid == 0 {
            if last_pids_refresh.elapsed() >= Duration::from_millis(300) {
                pids = x11::process_tree_pids(session.pid);
                last_pids_refresh = std::time::Instant::now();
            }
            if let Some(found) = api.find_game_window(dpy, host_xid, &pids) {
                let (bounds, seq) = active_bounds(&app);
                if api.attach_and_place(dpy, found, host_xid, bounds) {
                    mark_attached(&app, &session, found);
                    last_seq = seq;
                }
            }
            std::thread::sleep(POLL_IDLE);
            continue;
        }

        let (bounds, seq) = active_bounds(&app);
        if seq != last_seq {
            api.apply_bounds(dpy, mc_xid, bounds);
            last_seq = seq;
        }
        if session.focus_requested.swap(false, Ordering::AcqRel) {
            api.focus_window(dpy, mc_xid);
        }
        if session.close_requested.swap(false, Ordering::AcqRel) {
            api.request_close(dpy, mc_xid);
        }
        std::thread::sleep(POLL_ACTIVE);
    }
    api.close_display(dpy);
}

// ===================================================================================== Windows FFI

#[cfg(windows)]
mod win32 {
    use super::StageBounds;
    pub use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetClientRect, GetParent, GetWindow, GetWindowLongPtrW,
        GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, SetParent,
        SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, SWP_ASYNCWINDOWPOS,
        SWP_FRAMECHANGED, SWP_SHOWWINDOW, WM_CLOSE, WS_CAPTION, WS_CHILD, WS_CLIPSIBLINGS,
        WS_EX_APPWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
        WS_VISIBLE,
    };

    const HWND_TOP: HWND = std::ptr::null_mut();

    struct Search {
        pid: u32,
        host: HWND,
        found: Option<HWND>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut Search);
        if hwnd == search.host || IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut w_pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut w_pid);
        if w_pid != search.pid {
            return 1;
        }
        let owner = GetWindow(hwnd, GW_OWNER);
        let parent = GetParent(hwnd);
        if !owner.is_null() && parent != search.host {
            return 1;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut rect) == 0 {
            return 1;
        }
        if (rect.right - rect.left) >= 160 && (rect.bottom - rect.top) >= 120 {
            search.found = Some(hwnd);
            return 0;
        }
        1
    }

    pub fn find_game_window(pid: u32, host: HWND) -> Option<HWND> {
        let mut search = Search {
            pid,
            host,
            found: None,
        };
        // SAFETY: `search` outlives synchronous `EnumWindows` invocation.
        unsafe {
            EnumWindows(Some(enum_proc), (&mut search as *mut Search) as LPARAM);
        }
        search.found
    }

    pub fn is_valid(hwnd: HWND) -> bool {
        // SAFETY: Win32 handles non-window pointers safely by returning 0.
        unsafe { IsWindow(hwnd) != 0 && IsWindowVisible(hwnd) != 0 }
    }

    pub fn attach_and_place(mc: HWND, host: HWND, b: StageBounds) -> bool {
        // SAFETY: valid live HWNDs belonging to the game child and launcher host.
        unsafe {
            let style = GetWindowLongPtrW(mc, GWL_STYLE);
            let strip = (WS_POPUP
                | WS_CAPTION
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_SYSMENU) as isize;
            let add = (WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS) as isize;
            SetWindowLongPtrW(mc, GWL_STYLE, (style & !strip) | add);

            let ex = GetWindowLongPtrW(mc, GWL_EXSTYLE);
            SetWindowLongPtrW(mc, GWL_EXSTYLE, ex & !(WS_EX_APPWINDOW as isize));

            if SetParent(mc, host).is_null() {
                return false;
            }
            apply_bounds(mc, b);
            BringWindowToTop(mc);
            true
        }
    }

    pub fn apply_bounds(mc: HWND, b: StageBounds) {
        let w = i32::try_from(b.width).unwrap_or(1280);
        let h = i32::try_from(b.height).unwrap_or(720);
        let corner = i32::try_from(b.radius.saturating_mul(2)).unwrap_or(0);
        // SAFETY: `mc` is a child window inside the launcher; ownership of `rgn` transfers to Windows
        // on non-zero return of `SetWindowRgn`.
        unsafe {
            SetWindowPos(
                mc,
                HWND_TOP,
                b.x,
                b.y,
                w,
                h,
                SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS,
            );
            let rgn = if corner > 0 {
                CreateRoundRectRgn(0, 0, w + 1, h + 1, corner, corner)
            } else {
                std::ptr::null_mut()
            };
            if SetWindowRgn(mc, rgn, 1) == 0 && !rgn.is_null() {
                DeleteObject(rgn);
            }
        }
    }

    pub fn focus_window(mc: HWND) {
        // SAFETY: best-effort z-order lift when host window regains focus.
        unsafe {
            BringWindowToTop(mc);
        }
    }

    pub fn request_close(mc: HWND) -> bool {
        // SAFETY: posts asynchronous WM_CLOSE to the game window.
        unsafe { IsWindow(mc) != 0 && PostMessageW(mc, WM_CLOSE, 0, 0) != 0 }
    }
}

// ================================================================================ Linux X11 FFI

#[cfg(target_os = "linux")]
pub(crate) mod x11 {
    use std::collections::HashSet;
    use std::ffi::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void, CStr};
    use std::sync::OnceLock;

    use super::StageBounds;

    pub type Display = c_void;
    pub type Window = c_ulong;
    pub type Pixmap = c_ulong;
    pub type Drawable = c_ulong;
    pub type Atom = c_ulong;
    pub type Time = c_ulong;
    pub type Gc = *mut c_void;
    pub type Bool = c_int;
    pub type Status = c_int;

    const XA_ATOM: Atom = 4;
    const XA_CARDINAL: Atom = 6;
    const ANY_PROPERTY_TYPE: Atom = 0;
    const IS_VIEWABLE: c_int = 2;
    const PROP_MODE_REPLACE: c_int = 0;
    const FOCUS_CHANGE_MASK: c_long = 1 << 21;
    const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;
    const BUTTON_PRESS_MASK: c_uint = 1 << 2;
    const ANY_BUTTON: c_uint = 0;
    const ANY_MODIFIER: c_uint = 1 << 15;
    const GRAB_MODE_SYNC: c_int = 0;
    const GRAB_MODE_ASYNC: c_int = 1;
    const REPLAY_POINTER: c_int = 2;
    const REVERT_TO_PARENT: c_int = 2;
    const CURRENT_TIME: Time = 0;

    const EVENT_BUTTON_PRESS: c_int = 4;
    const EVENT_FOCUS_IN: c_int = 9;
    const EVENT_FOCUS_OUT: c_int = 10;
    const EVENT_DESTROY_NOTIFY: c_int = 17;
    const EVENT_UNMAP_NOTIFY: c_int = 18;
    const EVENT_CLIENT_MESSAGE: c_int = 33;

    const SHAPE_SET: c_int = 0;
    const SHAPE_BOUNDING: c_int = 0;

    #[repr(C)]
    #[derive(Default)]
    struct XWindowAttributes {
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
        border_width: c_int,
        depth: c_int,
        visual: *mut c_void,
        root: Window,
        class: c_int,
        bit_gravity: c_int,
        win_gravity: c_int,
        backing_store: c_int,
        backing_planes: c_ulong,
        backing_pixel: c_ulong,
        save_under: Bool,
        colormap: c_ulong,
        map_installed: Bool,
        map_state: c_int,
        all_event_masks: c_long,
        your_event_mask: c_long,
        do_not_propagate_mask: c_long,
        override_redirect: Bool,
        screen: *mut c_void,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct XAnyEvent {
        event_type: c_int,
        serial: c_ulong,
        send_event: Bool,
        display: *mut Display,
        window: Window,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct XDestroyWindowEvent {
        event_type: c_int,
        serial: c_ulong,
        send_event: Bool,
        display: *mut Display,
        event: Window,
        window: Window,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct XClientMessageEvent {
        event_type: c_int,
        serial: c_ulong,
        send_event: Bool,
        display: *mut Display,
        window: Window,
        message_type: Atom,
        format: c_int,
        data: [c_long; 5],
    }

    #[repr(C)]
    union XEvent {
        event_type: c_int,
        xany: XAnyEvent,
        xdestroywindow: XDestroyWindowEvent,
        xclient: XClientMessageEvent,
        _pad: [c_long; 24],
    }

    #[repr(C)]
    struct XGcValues {
        _opaque: [u8; 128],
    }

    type XErrorHandler = Option<unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int>;

    unsafe extern "C" fn ignore_x_error(_dpy: *mut Display, _err: *mut c_void) -> c_int {
        0
    }

    pub(crate) struct X11Api {
        _lib_x11: *mut c_void,
        _lib_xext: *mut c_void,
        x_open_display: unsafe extern "C" fn(*const c_char) -> *mut Display,
        x_close_display: unsafe extern "C" fn(*mut Display) -> c_int,
        x_set_error_handler: unsafe extern "C" fn(XErrorHandler) -> XErrorHandler,
        x_default_root_window: unsafe extern "C" fn(*mut Display) -> Window,
        x_query_tree: unsafe extern "C" fn(
            *mut Display,
            Window,
            *mut Window,
            *mut Window,
            *mut *mut Window,
            *mut c_uint,
        ) -> Status,
        x_get_window_property: unsafe extern "C" fn(
            *mut Display,
            Window,
            Atom,
            c_long,
            c_long,
            Bool,
            Atom,
            *mut Atom,
            *mut c_int,
            *mut c_ulong,
            *mut c_ulong,
            *mut *mut c_uchar,
        ) -> c_int,
        x_get_window_attributes:
            unsafe extern "C" fn(*mut Display, Window, *mut XWindowAttributes) -> Status,
        x_change_property: unsafe extern "C" fn(
            *mut Display,
            Window,
            Atom,
            Atom,
            c_int,
            c_int,
            *const c_uchar,
            c_int,
        ) -> c_int,
        x_reparent_window:
            unsafe extern "C" fn(*mut Display, Window, Window, c_int, c_int) -> c_int,
        x_move_resize_window:
            unsafe extern "C" fn(*mut Display, Window, c_int, c_int, c_uint, c_uint) -> c_int,
        x_map_raised: unsafe extern "C" fn(*mut Display, Window) -> c_int,
        x_select_input: unsafe extern "C" fn(*mut Display, Window, c_long) -> c_int,
        x_set_input_focus: unsafe extern "C" fn(*mut Display, Window, c_int, Time) -> c_int,
        x_grab_button: unsafe extern "C" fn(
            *mut Display,
            c_uint,
            c_uint,
            Window,
            Bool,
            c_uint,
            c_int,
            c_int,
            Window,
            c_ulong,
        ) -> c_int,
        x_ungrab_button: unsafe extern "C" fn(*mut Display, c_uint, c_uint, Window) -> c_int,
        x_allow_events: unsafe extern "C" fn(*mut Display, c_int, Time) -> c_int,
        x_send_event:
            unsafe extern "C" fn(*mut Display, Window, Bool, c_long, *mut XEvent) -> Status,
        x_pending: unsafe extern "C" fn(*mut Display) -> c_int,
        x_next_event: unsafe extern "C" fn(*mut Display, *mut XEvent) -> c_int,
        x_sync: unsafe extern "C" fn(*mut Display, Bool) -> c_int,
        x_flush: unsafe extern "C" fn(*mut Display) -> c_int,
        x_intern_atom: unsafe extern "C" fn(*mut Display, *const c_char, Bool) -> Atom,
        x_free: unsafe extern "C" fn(*mut c_void) -> c_int,
        x_create_pixmap:
            unsafe extern "C" fn(*mut Display, Drawable, c_uint, c_uint, c_uint) -> Pixmap,
        x_free_pixmap: unsafe extern "C" fn(*mut Display, Pixmap) -> c_int,
        x_create_gc: unsafe extern "C" fn(*mut Display, Drawable, c_ulong, *mut XGcValues) -> Gc,
        x_free_gc: unsafe extern "C" fn(*mut Display, Gc) -> c_int,
        x_set_foreground: unsafe extern "C" fn(*mut Display, Gc, c_ulong) -> c_int,
        x_fill_rectangle:
            unsafe extern "C" fn(*mut Display, Drawable, Gc, c_int, c_int, c_uint, c_uint) -> c_int,
        x_fill_arc: unsafe extern "C" fn(
            *mut Display,
            Drawable,
            Gc,
            c_int,
            c_int,
            c_uint,
            c_uint,
            c_int,
            c_int,
        ) -> c_int,
        x_shape_combine_mask:
            unsafe extern "C" fn(*mut Display, Window, c_int, c_int, c_int, Pixmap, c_int),
    }

    // SAFETY: `Display` handles are never shared across threads (each watcher opens its own
    // `Display*`), and function pointers resolved from `dlopen` live for the process lifetime.
    unsafe impl Send for X11Api {}
    unsafe impl Sync for X11Api {}

    unsafe fn sym<T>(lib: *mut c_void, name: &CStr) -> Option<T> {
        let p = libc::dlsym(lib, name.as_ptr());
        if p.is_null() {
            return None;
        }
        Some(std::mem::transmute_copy::<*mut c_void, T>(&p))
    }

    impl X11Api {
        pub(crate) fn load() -> Option<Self> {
            // SAFETY: loads standard X11/Xext shared libraries at runtime with lazy symbol resolution.
            unsafe {
                let lib_x11 =
                    libc::dlopen(c"libX11.so.6".as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
                if lib_x11.is_null() {
                    return None;
                }
                let lib_xext =
                    libc::dlopen(c"libXext.so.6".as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
                if lib_xext.is_null() {
                    return None;
                }
                let api = Self {
                    _lib_x11: lib_x11,
                    _lib_xext: lib_xext,
                    x_open_display: sym(lib_x11, c"XOpenDisplay")?,
                    x_close_display: sym(lib_x11, c"XCloseDisplay")?,
                    x_set_error_handler: sym(lib_x11, c"XSetErrorHandler")?,
                    x_default_root_window: sym(lib_x11, c"XDefaultRootWindow")?,
                    x_query_tree: sym(lib_x11, c"XQueryTree")?,
                    x_get_window_property: sym(lib_x11, c"XGetWindowProperty")?,
                    x_get_window_attributes: sym(lib_x11, c"XGetWindowAttributes")?,
                    x_change_property: sym(lib_x11, c"XChangeProperty")?,
                    x_reparent_window: sym(lib_x11, c"XReparentWindow")?,
                    x_move_resize_window: sym(lib_x11, c"XMoveResizeWindow")?,
                    x_map_raised: sym(lib_x11, c"XMapRaised")?,
                    x_select_input: sym(lib_x11, c"XSelectInput")?,
                    x_set_input_focus: sym(lib_x11, c"XSetInputFocus")?,
                    x_grab_button: sym(lib_x11, c"XGrabButton")?,
                    x_ungrab_button: sym(lib_x11, c"XUngrabButton")?,
                    x_allow_events: sym(lib_x11, c"XAllowEvents")?,
                    x_send_event: sym(lib_x11, c"XSendEvent")?,
                    x_pending: sym(lib_x11, c"XPending")?,
                    x_next_event: sym(lib_x11, c"XNextEvent")?,
                    x_sync: sym(lib_x11, c"XSync")?,
                    x_flush: sym(lib_x11, c"XFlush")?,
                    x_intern_atom: sym(lib_x11, c"XInternAtom")?,
                    x_free: sym(lib_x11, c"XFree")?,
                    x_create_pixmap: sym(lib_x11, c"XCreatePixmap")?,
                    x_free_pixmap: sym(lib_x11, c"XFreePixmap")?,
                    x_create_gc: sym(lib_x11, c"XCreateGC")?,
                    x_free_gc: sym(lib_x11, c"XFreeGC")?,
                    x_set_foreground: sym(lib_x11, c"XSetForeground")?,
                    x_fill_rectangle: sym(lib_x11, c"XFillRectangle")?,
                    x_fill_arc: sym(lib_x11, c"XFillArc")?,
                    x_shape_combine_mask: sym(lib_xext, c"XShapeCombineMask")?,
                };
                (api.x_set_error_handler)(Some(ignore_x_error));
                Some(api)
            }
        }

        pub(crate) fn open_display(&self) -> Option<*mut Display> {
            // SAFETY: null instructs libX11 to connect to the session DISPLAY.
            let dpy = unsafe { (self.x_open_display)(std::ptr::null()) };
            (!dpy.is_null()).then_some(dpy)
        }

        pub(crate) fn close_display(&self, dpy: *mut Display) {
            // SAFETY: `dpy` was opened by `open_display`.
            unsafe {
                (self.x_close_display)(dpy);
            }
        }

        fn atom(&self, dpy: *mut Display, name: &CStr) -> Atom {
            // SAFETY: valid open Display and NUL-terminated static atom name.
            unsafe { (self.x_intern_atom)(dpy, name.as_ptr(), 0) }
        }

        fn window_pid(&self, dpy: *mut Display, win: Window, pid_atom: Atom) -> Option<u32> {
            let mut actual_type: Atom = 0;
            let mut actual_format: c_int = 0;
            let mut nitems: c_ulong = 0;
            let mut bytes_after: c_ulong = 0;
            let mut prop: *mut c_uchar = std::ptr::null_mut();
            // SAFETY: output pointers initialized and buffer freed via `XFree` below.
            unsafe {
                if (self.x_get_window_property)(
                    dpy,
                    win,
                    pid_atom,
                    0,
                    1,
                    0,
                    XA_CARDINAL,
                    &mut actual_type,
                    &mut actual_format,
                    &mut nitems,
                    &mut bytes_after,
                    &mut prop,
                ) != 0
                {
                    return None;
                }
                let out = if !prop.is_null() && actual_format == 32 && nitems >= 1 {
                    let val = *(prop as *const c_ulong);
                    u32::try_from(val).ok()
                } else {
                    None
                };
                if !prop.is_null() {
                    (self.x_free)(prop.cast());
                }
                out
            }
        }

        fn inspect_subtree(
            &self,
            dpy: *mut Display,
            win: Window,
            host_xid: Window,
            pids: &HashSet<u32>,
            pid_atom: Atom,
            depth: usize,
        ) -> Option<Window> {
            if depth > 8 || win == host_xid {
                return None;
            }
            if let Some(pid) = self.window_pid(dpy, win, pid_atom) {
                if pids.contains(&pid) {
                    let mut attrs = XWindowAttributes::default();
                    // SAFETY: `attrs` matches X11 XWindowAttributes layout on Linux x86_64.
                    let ok = unsafe { (self.x_get_window_attributes)(dpy, win, &mut attrs) != 0 };
                    if ok
                        && attrs.map_state == IS_VIEWABLE
                        && attrs.override_redirect == 0
                        && attrs.width >= 160
                        && attrs.height >= 120
                    {
                        return Some(win);
                    }
                }
            }
            let mut root_ret: Window = 0;
            let mut parent_ret: Window = 0;
            let mut children: *mut Window = std::ptr::null_mut();
            let mut count: c_uint = 0;
            // SAFETY: `XQueryTree` allocates `children` freed with `XFree`.
            unsafe {
                if (self.x_query_tree)(
                    dpy,
                    win,
                    &mut root_ret,
                    &mut parent_ret,
                    &mut children,
                    &mut count,
                ) == 0
                {
                    return None;
                }
                let found = if !children.is_null() && count > 0 {
                    let slice = std::slice::from_raw_parts(children, count as usize);
                    slice.iter().rev().find_map(|&c| {
                        self.inspect_subtree(dpy, c, host_xid, pids, pid_atom, depth + 1)
                    })
                } else {
                    None
                };
                if !children.is_null() {
                    (self.x_free)(children.cast());
                }
                found
            }
        }

        pub(crate) fn find_game_window(
            &self,
            dpy: *mut Display,
            host_xid: Window,
            pids: &HashSet<u32>,
        ) -> Option<Window> {
            let pid_atom = self.atom(dpy, c"_NET_WM_PID");
            if pid_atom == 0 {
                return None;
            }
            // SAFETY: `XDefaultRootWindow` returns the X11 root window ID.
            let root = unsafe { (self.x_default_root_window)(dpy) };
            self.inspect_subtree(dpy, root, host_xid, pids, pid_atom, 0)
        }

        pub(crate) fn attach_and_place(
            &self,
            dpy: *mut Display,
            mc: Window,
            host: Window,
            b: StageBounds,
        ) -> bool {
            let motif_atom = self.atom(dpy, c"_MOTIF_WM_HINTS");
            let hints: [c_ulong; 5] = [2, 0, 0, 0, 0];
            // SAFETY: `dpy`, `mc`, and `host` are live X11 display and window identifiers; error
            // handler ignores asynchronous BadWindow races cleanly.
            unsafe {
                if motif_atom != 0 {
                    (self.x_change_property)(
                        dpy,
                        mc,
                        motif_atom,
                        motif_atom,
                        32,
                        PROP_MODE_REPLACE,
                        hints.as_ptr().cast(),
                        5,
                    );
                }
                (self.x_reparent_window)(dpy, mc, host, b.x, b.y);
                self.apply_bounds(dpy, mc, b);
                (self.x_map_raised)(dpy, mc);
                (self.x_select_input)(dpy, mc, FOCUS_CHANGE_MASK | STRUCTURE_NOTIFY_MASK);
                (self.x_set_input_focus)(dpy, mc, REVERT_TO_PARENT, CURRENT_TIME);
                (self.x_sync)(dpy, 0);
            }
            true
        }

        pub(crate) fn apply_bounds(&self, dpy: *mut Display, mc: Window, b: StageBounds) {
            let w = b.width as c_uint;
            let h = b.height as c_uint;
            // SAFETY: resizes reparented child inside host and updates 1-bit XShape bounding mask.
            unsafe {
                (self.x_move_resize_window)(dpy, mc, b.x, b.y, w, h);
                if b.radius == 0 {
                    (self.x_shape_combine_mask)(dpy, mc, SHAPE_BOUNDING, 0, 0, 0, SHAPE_SET);
                } else {
                    let r = (b.radius as c_int)
                        .min((b.width as c_int) / 2)
                        .min((b.height as c_int) / 2);
                    let d = (r * 2) as c_uint;
                    let pixmap = (self.x_create_pixmap)(dpy, mc, w, h, 1);
                    if pixmap != 0 {
                        let gc = (self.x_create_gc)(dpy, pixmap, 0, std::ptr::null_mut());
                        if !gc.is_null() {
                            (self.x_set_foreground)(dpy, gc, 0);
                            (self.x_fill_rectangle)(dpy, pixmap, gc, 0, 0, w, h);
                            (self.x_set_foreground)(dpy, gc, 1);
                            if w > d {
                                (self.x_fill_rectangle)(dpy, pixmap, gc, r, 0, w - d, h);
                            }
                            if h > d {
                                (self.x_fill_rectangle)(dpy, pixmap, gc, 0, r, w, h - d);
                            }
                            if d > 0 {
                                let rx = (w - d) as c_int;
                                let by = (h - d) as c_int;
                                for (cx, cy) in [(0, 0), (rx, 0), (0, by), (rx, by)] {
                                    (self.x_fill_arc)(dpy, pixmap, gc, cx, cy, d, d, 0, 360 * 64);
                                }
                            }
                            (self.x_shape_combine_mask)(
                                dpy,
                                mc,
                                SHAPE_BOUNDING,
                                0,
                                0,
                                pixmap,
                                SHAPE_SET,
                            );
                            (self.x_free_gc)(dpy, gc);
                        }
                        (self.x_free_pixmap)(dpy, pixmap);
                    }
                }
                (self.x_map_raised)(dpy, mc);
                (self.x_flush)(dpy);
            }
        }

        pub(crate) fn focus_window(&self, dpy: *mut Display, mc: Window) {
            // SAFETY: elevates child and focuses it cleanly when host gains focus.
            unsafe {
                (self.x_map_raised)(dpy, mc);
                (self.x_set_input_focus)(dpy, mc, REVERT_TO_PARENT, CURRENT_TIME);
                (self.x_ungrab_button)(dpy, ANY_BUTTON, ANY_MODIFIER, mc);
                (self.x_flush)(dpy);
            }
        }

        pub(crate) fn request_close(&self, dpy: *mut Display, mc: Window) -> bool {
            let protocols = self.atom(dpy, c"WM_PROTOCOLS");
            let delete = self.atom(dpy, c"WM_DELETE_WINDOW");
            if protocols == 0 || delete == 0 {
                return false;
            }
            let mut ev = XEvent {
                xclient: XClientMessageEvent {
                    event_type: EVENT_CLIENT_MESSAGE,
                    serial: 0,
                    send_event: 1,
                    display: dpy,
                    window: mc,
                    message_type: protocols,
                    format: 32,
                    data: [delete as c_long, CURRENT_TIME as c_long, 0, 0, 0],
                },
            };
            // SAFETY: initialized ClientMessage event for ICCCM WM_DELETE_WINDOW.
            unsafe {
                let ok = (self.x_send_event)(dpy, mc, 0, 0, &mut ev) != 0;
                (self.x_flush)(dpy);
                ok
            }
        }

        pub(crate) fn drain_events(&self, dpy: *mut Display, mc: Window, invalidated: &mut bool) {
            // SAFETY: reads queued X11 events from the thread-private Display handle.
            unsafe {
                while (self.x_pending)(dpy) > 0 {
                    let mut ev = std::mem::zeroed::<XEvent>();
                    (self.x_next_event)(dpy, &mut ev);
                    let et = ev.event_type;
                    if mc == 0 {
                        continue;
                    }
                    match et {
                        EVENT_FOCUS_OUT if ev.xany.window == mc => {
                            (self.x_grab_button)(
                                dpy,
                                ANY_BUTTON,
                                ANY_MODIFIER,
                                mc,
                                0,
                                BUTTON_PRESS_MASK,
                                GRAB_MODE_SYNC,
                                GRAB_MODE_ASYNC,
                                0,
                                0,
                            );
                        }
                        EVENT_FOCUS_IN if ev.xany.window == mc => {
                            (self.x_ungrab_button)(dpy, ANY_BUTTON, ANY_MODIFIER, mc);
                        }
                        EVENT_BUTTON_PRESS if ev.xany.window == mc => {
                            (self.x_set_input_focus)(dpy, mc, REVERT_TO_PARENT, CURRENT_TIME);
                            (self.x_allow_events)(dpy, REPLAY_POINTER, CURRENT_TIME);
                            (self.x_ungrab_button)(dpy, ANY_BUTTON, ANY_MODIFIER, mc);
                            (self.x_flush)(dpy);
                        }
                        EVENT_DESTROY_NOTIFY | EVENT_UNMAP_NOTIFY
                            if ev.xdestroywindow.window == mc =>
                        {
                            *invalidated = true;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    static API: OnceLock<Option<X11Api>> = OnceLock::new();

    pub(crate) fn api() -> Option<&'static X11Api> {
        API.get_or_init(X11Api::load).as_ref()
    }

    pub fn is_available() -> bool {
        api().is_some()
    }

    /// Collects `root_pid` and all descendant process IDs from `/proc/<pid>/stat`.
    pub(crate) fn process_tree_pids(root_pid: u32) -> HashSet<u32> {
        let mut children: Vec<(u32, u32)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir("/proc") {
            for entry in rd.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let Ok(pid) = name.parse::<u32>() else {
                    continue;
                };
                if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) {
                    if let Some(rparen) = stat.rfind(')') {
                        let mut parts = stat[rparen + 1..].split_whitespace();
                        if let Some(ppid) = parts.nth(1).and_then(|s| s.parse::<u32>().ok()) {
                            children.push((pid, ppid));
                        }
                    }
                }
            }
        }
        let mut out = HashSet::from([root_pid]);
        loop {
            let before = out.len();
            for &(pid, ppid) in &children {
                if out.contains(&ppid) {
                    out.insert(pid);
                }
            }
            if out.len() == before {
                break;
            }
        }
        out
    }

    // Silence dead-code warnings on unused atom constant kept alongside XA_CARDINAL.
    const _: Atom = XA_ATOM;
    const _: Atom = ANY_PROPERTY_TYPE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_bounds_clamp_dimensions_and_corner_radius() {
        assert_eq!(
            StageBounds::clamped(14, 44, 0, 20, 40),
            StageBounds {
                x: 14,
                y: 44,
                width: 64,
                height: 64,
                radius: 32,
            }
        );
        assert_eq!(
            StageBounds::clamped(-5, 0, 1280, 720, 16),
            StageBounds {
                x: -5,
                y: 0,
                width: 1280,
                height: 720,
                radius: 16,
            }
        );
    }

    #[test]
    fn embed_state_tracks_attachment_bounds_and_reset() {
        let state = EmbedState::new();
        assert_eq!(state.status(), GameEmbedStatus::default());

        let s1 = state.begin("survival".into(), 1001);
        assert!(
            !state.is_active(),
            "must remain inactive until window attaches"
        );
        assert!(state.current_bounds().0.is_none());

        state.set_bounds(StageBounds::clamped(14, 44, 1200, 650, 16));
        let (bounds, seq1) = state.current_bounds();
        assert_eq!(
            bounds,
            Some(StageBounds {
                x: 14,
                y: 44,
                width: 1200,
                height: 650,
                radius: 16
            })
        );
        assert_eq!(seq1, 1);

        s1.game_win.store(42, Ordering::Release);
        s1.attached.store(true, Ordering::Release);
        *s1.restore_size.lock().unwrap() = Some(PhysicalSize::new(1280, 720));
        assert_eq!(
            state.status(),
            GameEmbedStatus {
                active: true,
                instance_id: Some("survival".into()),
                maximized: false,
            }
        );

        assert!(state.finish("other").is_none());
        let ended = state.finish("survival").unwrap();
        assert!(ended.was_attached);
        assert_eq!(ended.restore_size, Some(PhysicalSize::new(1280, 720)));
        assert!(s1.stop.load(Ordering::Acquire));
        assert!(!state.is_active());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn x11_and_shape_symbols_load_cleanly_on_linux() {
        assert!(
            x11::is_available(),
            "libX11.so.6 and libXext.so.6 symbols must load on Linux desktop hosts"
        );
        let pid = std::process::id();
        assert!(x11::process_tree_pids(pid).contains(&pid));
    }
}
