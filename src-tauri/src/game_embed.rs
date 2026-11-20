//! Native framed game embedding ("Inside Clade").
//!
//! When `"When Minecraft starts"` is `"embed"`, Clade keeps its own frameless window open as a host
//! frame (title strip with Minimize / Maximize / Close + warm background padding) and reparents the
//! running Minecraft window directly inside the launcher's central stage region via Win32
//! `SetParent`, child window styles, `SetWindowPos`, `SetWindowRgn`, and `CreateRoundRectRgn`.
//!
//! No game frames are ever copied through the WebView: Java/OpenGL renders straight to its native
//! surface and keeps raw mouse/keyboard handling.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::time::Duration;

use raw_window_handle::HasWindowHandle;
#[cfg(windows)]
use raw_window_handle::RawWindowHandle;
use serde::Serialize;
#[cfg(windows)]
use tauri::PhysicalSize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Event announcing a change to the framed game embedding state.
pub const EMBED_EVENT: &str = "game-embed-state";

#[cfg(windows)]
const POLL_IDLE: Duration = Duration::from_millis(16);
#[cfg(windows)]
const POLL_ACTIVE: Duration = Duration::from_millis(24);
#[cfg(windows)]
const FALLBACK_TOP_PX: i32 = 44;
#[cfg(windows)]
const FALLBACK_MARGIN_PX: i32 = 18;
#[cfg(windows)]
const FALLBACK_RADIUS_PX: u32 = 0;

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

    #[cfg(windows)]
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
    #[cfg_attr(not(windows), allow(dead_code))]
    pid: u32,
    game_win: AtomicU64,
    attached: AtomicBool,
    maximized: AtomicBool,
    #[cfg_attr(not(windows), allow(dead_code))]
    attach_win_rect: Mutex<Option<(i32, i32, i32, i32)>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    pre_expand_win_rect: Mutex<Option<(i32, i32, i32, i32)>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    close_requested: AtomicBool,
    close_attempts: AtomicU8,
    #[cfg_attr(not(windows), allow(dead_code))]
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
            attach_win_rect: Mutex::new(None),
            pre_expand_win_rect: Mutex::new(None),
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
    #[cfg_attr(not(windows), allow(dead_code))]
    attach_win_rect: Option<(i32, i32, i32, i32)>,
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
        let attach_win_rect = *session
            .attach_win_rect
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Some(FinishedEmbed {
            was_attached,
            attach_win_rect,
        })
    }
}

impl Default for EmbedState {
    fn default() -> Self {
        Self::new()
    }
}

/// Native window identifier (`HWND` on Windows) if embedding is possible.
pub fn host_window_id(window: &tauri::WebviewWindow) -> Option<u64> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        #[cfg(windows)]
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as usize as u64),
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
    app: AppHandle,
    state: State<'_, EmbedState>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    radius: u32,
) {
    let bounds = StageBounds::clamped(x, y, width, height, radius);
    state.set_bounds(bounds);
    #[cfg(windows)]
    if let Some(session) = state.current_session() {
        let mc_id = session.game_win.load(Ordering::Acquire);
        if session.attached.load(Ordering::Acquire) && mc_id != 0 {
            if let Some(host_id) = app
                .get_webview_window("main")
                .and_then(|w| host_window_id(&w))
            {
                win32::dispatch_host_clipping(&app, host_id, mc_id, Some(bounds));
            }
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[tauri::command]
pub fn toggle_game_maximize(app: AppHandle, state: State<'_, EmbedState>) -> GameEmbedStatus {
    let Some(session) = state.current_session() else {
        return GameEmbedStatus::default();
    };
    if !session.attached.load(Ordering::Acquire) {
        return GameEmbedStatus::default();
    }
    let next = !session.maximized.load(Ordering::Acquire);
    session.maximized.store(next, Ordering::Release);

    if let Some(w) = app.get_webview_window("main") {
        #[cfg(windows)]
        {
            // Expand to the monitor work area instead of Win32 SW_MAXIMIZE so DWM never paints
            // duplicate native caption buttons on top of Clade's custom header strip.
            let _ = w.set_resizable(!next);
            if let Some(host_id) = host_window_id(&w) {
                let mc_id = session.game_win.load(Ordering::Acquire);
                let app_clone = app.clone();
                let session_clone = session.clone();
                let _ = app.run_on_main_thread(move || {
                    let host_hwnd = host_id as usize as win32::HWND;
                    let mc_hwnd = mc_id as usize as win32::HWND;
                    win32::set_work_area_expanded(host_hwnd, next, &session_clone);
                    if mc_id != 0 {
                        win32::sync_host_clipping(
                            host_hwnd,
                            mc_hwnd,
                            Some(active_bounds(&app_clone).0),
                        );
                    }
                });
            }
        }
        #[cfg(not(windows))]
        let _ = w;
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

/// Keeps `maximized` (and Win32 WebView clipping) in sync when the host resizes during a game.
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
    #[cfg(windows)]
    {
        let mc_id = session.game_win.load(Ordering::Acquire);
        if mc_id != 0 {
            if let Some(host_id) = app
                .get_webview_window("main")
                .and_then(|w| host_window_id(&w))
            {
                win32::dispatch_host_clipping(app, host_id, mc_id, Some(active_bounds(app).0));
            }
        }
    }
    #[cfg(not(windows))]
    let _ = session;
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
    tracing::info!(instance = %instance_id, pid, "watching for game window to embed");
    let state = app.state::<EmbedState>();
    let session = state.begin(instance_id, pid);

    let thread_name = "clade-game-embed".to_owned();
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            #[cfg(windows)]
            run_watcher_win32(app, session, host_id);
            #[cfg(not(windows))]
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
            #[cfg(windows)]
            {
                let _ = w.set_resizable(true);
                if let Some(host_id) = host_window_id(&w) {
                    let attach_rect = ended.attach_win_rect;
                    let _ = app.run_on_main_thread(move || {
                        let host_hwnd = host_id as usize as win32::HWND;
                        win32::sync_host_clipping(host_hwnd, std::ptr::null_mut(), None);
                        win32::set_corner_preference(host_hwnd, false);
                        if let Some((x, y, width, height)) = attach_rect {
                            win32::set_window_rect(host_hwnd, x, y, width, height);
                        }
                    });
                }
            }
            #[cfg(not(windows))]
            let _ = (w, ended.attach_win_rect);
        }
    }
}

#[cfg(windows)]
fn mark_attached(app: &AppHandle, session: &ActiveSession, win_id: u64) {
    session.game_win.store(win_id, Ordering::Release);
    if !session.attached.swap(true, Ordering::AcqRel) {
        tracing::info!(
            instance = %session.instance_id,
            "embedded game window into Clade stage"
        );
        if let Some(w) = app.get_webview_window("main") {
            if let Some(host_id) = host_window_id(&w) {
                let host_hwnd = host_id as usize as win32::HWND;
                *session
                    .attach_win_rect
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = win32::window_rect(host_hwnd);
                let _ = w.set_resizable(true);
                win32::dispatch_host_clipping(app, host_id, win_id, Some(active_bounds(app).0));
            } else {
                let _ = w.set_resizable(true);
            }
        }
        let _ = app.emit(EMBED_EVENT, session.status());
    }
}

#[cfg(windows)]
fn active_bounds(app: &AppHandle) -> (StageBounds, u64) {
    let state = app.state::<EmbedState>();
    let (maybe, seq) = state.current_bounds();
    let bounds = maybe.unwrap_or_else(|| {
        app.get_webview_window("main")
            .map(|w| StageBounds::fallback_from_window(&w))
            .unwrap_or_else(|| StageBounds::clamped(18, 44, 1244, 658, 0))
    });
    (bounds, seq)
}

#[cfg(windows)]
fn run_watcher_win32(app: AppHandle, session: Arc<ActiveSession>, host_id: u64) {
    let host_hwnd = host_id as usize as win32::HWND;
    let mut last_seq = u64::MAX;
    let mut delayed_focus_at: Option<std::time::Instant> = None;

    while !session.stop.load(Ordering::Acquire) {
        let mut mc_id = session.game_win.load(Ordering::Acquire);
        if mc_id != 0 && !win32::is_valid(mc_id as usize as win32::HWND) {
            mc_id = 0;
            session.game_win.store(0, Ordering::Release);
            last_seq = u64::MAX;
            delayed_focus_at = None;
        }

        if mc_id == 0 {
            if let Some(found) = win32::find_game_window(session.pid, host_hwnd) {
                let (bounds, seq) = active_bounds(&app);
                if win32::attach_and_place(found, host_hwnd, bounds) {
                    let found_id = found as usize as u64;
                    mark_attached(&app, &session, found_id);
                    last_seq = seq;
                    delayed_focus_at = Some(std::time::Instant::now() + Duration::from_millis(60));
                }
            }
            std::thread::sleep(POLL_IDLE);
            continue;
        }

        let mc_hwnd = mc_id as usize as win32::HWND;
        let (bounds, seq) = active_bounds(&app);
        if seq != last_seq || win32::needs_bounds_sync(mc_hwnd, bounds) {
            win32::apply_bounds(mc_hwnd, bounds);
            if seq != last_seq {
                win32::dispatch_host_clipping(&app, host_id, mc_id, Some(bounds));
            }
            last_seq = seq;
        }
        if session.focus_requested.swap(false, Ordering::AcqRel) {
            delayed_focus_at = Some(std::time::Instant::now() + Duration::from_millis(60));
            win32::focus_window(mc_hwnd);
        } else if delayed_focus_at.is_some_and(|t| std::time::Instant::now() >= t) {
            delayed_focus_at = None;
            win32::focus_window(mc_hwnd);
        }
        if session.close_requested.swap(false, Ordering::AcqRel) {
            win32::request_close(mc_hwnd);
        }
        std::thread::sleep(POLL_ACTIVE);
    }
}

// ===================================================================================== Windows FFI

#[cfg(windows)]
mod win32 {
    use super::StageBounds;
    use tauri::AppHandle;
    use windows_sys::core::BOOL;
    pub use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_DONOTROUND,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        ClientToScreen, CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject,
        GetMonitorInfoW, MonitorFromWindow, RedrawWindow, SetWindowRgn, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, RDW_ALLCHILDREN, RDW_INVALIDATE, RGN_DIFF,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumChildWindows, EnumWindows, GetClientRect, GetParent, GetWindow,
        GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
        PostMessageW, SetParent, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, GW_OWNER,
        HWND_BOTTOM, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SWP_SHOWWINDOW, WM_CLOSE, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN,
        WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
        WS_THICKFRAME, WS_VISIBLE,
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
            EnumWindows(Some(enum_proc), std::ptr::addr_of_mut!(search) as LPARAM);
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
            let style = GetWindowLongPtrW(mc, GWL_STYLE) as u32;
            let strip = WS_POPUP
                | WS_CAPTION
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_SYSMENU;
            let add = WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
            SetWindowLongPtrW(mc, GWL_STYLE, ((style & !strip) | add) as i32 as isize);

            let ex = GetWindowLongPtrW(mc, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(mc, GWL_EXSTYLE, (ex & !WS_EX_APPWINDOW) as i32 as isize);

            // Clear any stale region once without re-applying per resize so WGL flip swapchains stay intact.
            SetWindowRgn(mc, std::ptr::null_mut(), 0);
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
            if corner > 0 {
                let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, corner, corner);
                if SetWindowRgn(mc, rgn, 1) == 0 && !rgn.is_null() {
                    DeleteObject(rgn);
                }
            }
        }
    }

    pub fn window_rect(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: `hwnd` is a live window handle and `rect` is initialized.
        unsafe {
            if IsWindow(hwnd) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
                return None;
            }
        }
        Some((
            rect.left,
            rect.top,
            (rect.right - rect.left).max(1),
            (rect.bottom - rect.top).max(1),
        ))
    }

    pub fn set_window_rect(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: moves/resizes live top-level host window without disturbing Z-order or activation.
        unsafe {
            if IsWindow(hwnd) != 0 {
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    x,
                    y,
                    width.max(1),
                    height.max(1),
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }
    }

    pub fn set_corner_preference(hwnd: HWND, square: bool) {
        let pref: i32 = if square {
            DWMWCP_DONOTROUND
        } else {
            DWMWCP_DEFAULT
        };
        // SAFETY: `pref` outlives synchronous DwmSetWindowAttribute invocation.
        unsafe {
            if IsWindow(hwnd) != 0 {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    std::ptr::from_ref(&pref).cast(),
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    }

    struct ClipChildrenCtx {
        host: HWND,
        mc: HWND,
        client_w: i32,
        client_h: i32,
        hole: Option<(i32, i32, i32, i32)>,
    }

    unsafe extern "system" fn enum_children_proc(child: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &*(lparam as *const ClipChildrenCtx);
        if GetParent(child) != ctx.host || child == ctx.mc {
            return 1;
        }
        if let Some((hx, hy, hw, hh)) = ctx.hole {
            let cstyle = GetWindowLongPtrW(child, GWL_STYLE) as u32;
            if (cstyle & WS_CLIPSIBLINGS) == 0 {
                SetWindowLongPtrW(child, GWL_STYLE, (cstyle | WS_CLIPSIBLINGS) as i32 as isize);
            }
            let outer = CreateRectRgn(0, 0, ctx.client_w, ctx.client_h);
            if !outer.is_null() {
                let cut = CreateRectRgn(hx, hy, hx + hw, hy + hh);
                if !cut.is_null() {
                    let combined = CombineRgn(outer, outer, cut, RGN_DIFF);
                    DeleteObject(cut);
                    if combined != 0 {
                        if SetWindowRgn(child, outer, 1) == 0 {
                            DeleteObject(outer);
                        }
                    } else {
                        DeleteObject(outer);
                    }
                } else {
                    DeleteObject(outer);
                }
            }
            SetWindowPos(
                child,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        } else {
            SetWindowRgn(child, std::ptr::null_mut(), 1);
            RedrawWindow(
                child,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN,
            );
        }
        1
    }

    pub fn sync_host_clipping(host: HWND, mc: HWND, hole: Option<StageBounds>) {
        // SAFETY: invoked on the main UI thread owning `host` and its `WRY_WEBVIEW` child;
        // transfers cutout `HRGN` ownership on successful `SetWindowRgn`.
        unsafe {
            if IsWindow(host) == 0 {
                return;
            }
            let style = GetWindowLongPtrW(host, GWL_STYLE) as u32;
            if (style & WS_CLIPCHILDREN) == 0 {
                SetWindowLongPtrW(host, GWL_STYLE, (style | WS_CLIPCHILDREN) as i32 as isize);
            }
            let mut cr = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetClientRect(host, &mut cr) == 0 || cr.right <= 0 || cr.bottom <= 0 {
                return;
            }
            let hole_rect = hole.map(|b| {
                let w = i32::try_from(b.width).unwrap_or(1244).max(1);
                let h = i32::try_from(b.height).unwrap_or(658).max(1);
                (b.x, b.y, w, h)
            });
            let mut ctx = ClipChildrenCtx {
                host,
                mc,
                client_w: cr.right,
                client_h: cr.bottom,
                hole: hole_rect,
            };
            EnumChildWindows(
                host,
                Some(enum_children_proc),
                std::ptr::addr_of_mut!(ctx) as LPARAM,
            );
            if hole.is_some() && !mc.is_null() && IsWindow(mc) != 0 {
                BringWindowToTop(mc);
            }
        }
    }

    pub fn dispatch_host_clipping(
        app: &AppHandle,
        host_id: u64,
        mc_id: u64,
        hole: Option<StageBounds>,
    ) {
        let _ = app.run_on_main_thread(move || {
            sync_host_clipping(host_id as usize as HWND, mc_id as usize as HWND, hole);
        });
    }

    pub fn set_work_area_expanded(host: HWND, expand: bool, session: &super::ActiveSession) {
        // SAFETY: reads monitor work area and updates top-level host position cleanly on UI thread.
        unsafe {
            if IsWindow(host) == 0 {
                return;
            }
            if expand {
                if let Some(cur) = window_rect(host) {
                    *session
                        .pre_expand_win_rect
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(cur);
                }
                let mon = MonitorFromWindow(host, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    rcMonitor: RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    rcWork: RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    dwFlags: 0,
                };
                if GetMonitorInfoW(mon, &mut mi) == 0 {
                    return;
                }
                let work = mi.rcWork;
                let mut wr = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                let mut cr = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                let mut pt = POINT { x: 0, y: 0 };
                let (il, it, ir, ib) = if GetWindowRect(host, &mut wr) != 0
                    && GetClientRect(host, &mut cr) != 0
                    && ClientToScreen(host, &mut pt) != 0
                {
                    let il = (pt.x - wr.left).clamp(0, 32);
                    let it = (pt.y - wr.top).clamp(0, 32);
                    let ir = (wr.right - (pt.x + cr.right)).clamp(0, 32);
                    let ib = (wr.bottom - (pt.y + cr.bottom)).clamp(0, 32);
                    (il, it, ir, ib)
                } else {
                    (0, 0, 0, 0)
                };
                set_corner_preference(host, true);
                set_window_rect(
                    host,
                    work.left - il,
                    work.top - it,
                    (work.right - work.left) + il + ir,
                    (work.bottom - work.top) + it + ib,
                );
            } else {
                set_corner_preference(host, false);
                if let Some((rx, ry, rw, rh)) = *session
                    .pre_expand_win_rect
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                {
                    set_window_rect(host, rx, ry, rw, rh);
                }
            }
        }
    }

    pub fn needs_bounds_sync(mc: HWND, b: StageBounds) -> bool {
        let w = i32::try_from(b.width).unwrap_or(1280);
        let h = i32::try_from(b.height).unwrap_or(720);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: valid live HWND checked with IsWindowVisible.
        unsafe {
            if IsWindowVisible(mc) == 0 || GetClientRect(mc, &mut rect) == 0 {
                return false;
            }
        }
        (rect.right - rect.left) != w || (rect.bottom - rect.top) != h
    }

    pub fn focus_window(mc: HWND) {
        // SAFETY: best-effort z-order lift when host window regains focus.
        unsafe {
            BringWindowToTop(mc);
        }
    }

    pub fn request_close(mc: HWND) {
        // SAFETY: posts asynchronous WM_CLOSE to the game window.
        unsafe {
            if IsWindow(mc) != 0 {
                let _ = PostMessageW(mc, WM_CLOSE, 0, 0);
            }
        }
    }
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
        *s1.attach_win_rect.lock().unwrap() = Some((100, 100, 1280, 720));
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
        assert_eq!(ended.attach_win_rect, Some((100, 100, 1280, 720)));
        assert!(s1.stop.load(Ordering::Acquire));
        assert!(!state.is_active());
    }
}
