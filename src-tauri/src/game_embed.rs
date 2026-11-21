//! Native framed game embedding ("Inside Clade").
//!
//! When `"When Minecraft starts"` is `"embed"`, Clade keeps its own frameless window open as a host
//! frame and splits the host client area into two **disjoint** regions:
//!
//! - Top strip `(0, 0, client_w, header_px)`: WebView2 (`WRY_WEBVIEW` + `ICoreWebView2Controller`)
//!   renders `WindowChrome` (`Clade` wordmark, drag strip, Minimize / Maximize / Close buttons).
//! - Stage `(0, header_px, client_w, client_h - header_px)` in host client coordinates, mapped to
//!   screen coordinates: the running Minecraft window is styled as a borderless top-level window
//!   **owned** by Clade's host (`GWLP_HWNDPARENT`, `WS_EX_APPWINDOW` stripped, flush below the strip).
//!
//! Keeping Minecraft an owned top-level `WS_POPUP` instead of a `WS_CHILD` preserves Chromium
//! DirectComposition/Mica in WebView2 AND allows Windows to return the game HWND from
//! `GetActiveWindow()` / `SetForegroundWindow()` so GLFW's `"Back to Game"` pointer grab works.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Event announcing a change to the framed game embedding state.
pub const EMBED_EVENT: &str = "game-embed-state";

/// Logical height (CSS pixels at 100% DPI) of the custom Clade header strip in framed mode.
pub const HEADER_HEIGHT_LOGICAL_PX: i32 = 44;

const POLL_IDLE: Duration = Duration::from_millis(16);
const POLL_ACTIVE: Duration = Duration::from_millis(24);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);

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
}

/// Converts host-client [`StageBounds`] at client origin `(origin_x, origin_y)` into screen `(x, y, width, height)`.
pub fn stage_to_screen_rect(
    origin_x: i32,
    origin_y: i32,
    stage: StageBounds,
) -> (i32, i32, i32, i32) {
    let w = i32::try_from(stage.width).unwrap_or(1280).max(64);
    let h = i32::try_from(stage.height).unwrap_or(720).max(64);
    (origin_x + stage.x, origin_y + stage.y, w, h)
}

/// Disjoint native rectangles for WebView2's top strip and Minecraft's stage in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramedLayout {
    pub webview_width: i32,
    pub webview_height: i32,
    pub stage: StageBounds,
}

/// Computes non-overlapping top-strip and stage bounds from host client pixels and monitor DPI.
pub fn compute_framed_layout(client_w: i32, client_h: i32, dpi: u32) -> Option<FramedLayout> {
    if client_w < 64 || client_h < 64 {
        return None;
    }
    let scale = (f64::from(dpi.max(48)) / 96.0).clamp(0.5, 4.0);
    let max_header = (client_h - 64).max(24);
    let header_px =
        ((f64::from(HEADER_HEIGHT_LOGICAL_PX) * scale).round() as i32).clamp(24, max_header);
    let stage_h = (client_h - header_px).max(64) as u32;
    Some(FramedLayout {
        webview_width: client_w,
        webview_height: header_px,
        stage: StageBounds::clamped(0, header_px, client_w as u32, stage_h, 0),
    })
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
    ever_attached: AtomicBool,
    strip_active: AtomicBool,
    prev_host_clipchildren: AtomicBool,
    maximized: AtomicBool,
    attach_win_rect: Mutex<Option<(i32, i32, i32, i32)>>,
    pre_expand_win_rect: Mutex<Option<(i32, i32, i32, i32)>>,
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
            ever_attached: AtomicBool::new(false),
            strip_active: AtomicBool::new(false),
            prev_host_clipchildren: AtomicBool::new(false),
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

    /// Marks the currently embedded HWND detached while keeping session history (`ever_attached`,
    /// `attach_win_rect`, `prev_host_clipchildren`) until `end_session` completes.
    fn mark_window_detached(&self) -> bool {
        self.game_win.store(0, Ordering::Release);
        self.maximized.store(false, Ordering::Release);
        self.close_requested.store(false, Ordering::Release);
        self.close_attempts.store(0, Ordering::Release);
        self.focus_requested.store(false, Ordering::Release);
        let was_attached = self.attached.swap(false, Ordering::AcqRel);
        let was_strip = self.strip_active.swap(false, Ordering::AcqRel);
        was_attached || was_strip
    }
}

/// Snapshot saved when ending a framed session so host geometry and styles restore idempotently.
struct FinishedEmbed {
    was_attached: bool,
    ever_attached: bool,
    strip_active: bool,
    attach_win_rect: Option<(i32, i32, i32, i32)>,
    prev_host_clipchildren: bool,
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
        let prev = {
            let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
            guard.replace(next.clone())
        };
        if let Some(prev) = prev {
            prev.stop.store(true, Ordering::Release);
            prev.strip_active.store(false, Ordering::Release);
            let prev_mc = prev.game_win.swap(0, Ordering::AcqRel);
            if prev.attached.swap(false, Ordering::AcqRel) && prev_mc != 0 {
                #[cfg(windows)]
                win32::detach_to_standalone(prev_mc as usize as win32::HWND);
            }
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
        let ever_attached = session.ever_attached.load(Ordering::Acquire);
        let strip_active = session.strip_active.swap(false, Ordering::AcqRel);
        let prev_host_clipchildren = session.prev_host_clipchildren.load(Ordering::Acquire);
        let attach_win_rect = *session
            .attach_win_rect
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Some(FinishedEmbed {
            was_attached,
            ever_attached,
            strip_active,
            attach_win_rect,
            prev_host_clipchildren,
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
    state: State<'_, EmbedState>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    radius: u32,
) {
    state.set_bounds(StageBounds::clamped(x, y, width, height, radius));
}

thread_local! {
    static EXTRACTED_CONTROLLER: std::cell::RefCell<Option<win32::ICoreWebView2Controller>> =
        const { std::cell::RefCell::new(None) };
}

/// Clones `ICoreWebView2Controller` on the main UI STA thread and returns from
/// `WebviewWindow::with_webview` immediately so `tauri-runtime-wry` releases its
/// `window_id` mutex before `f` performs Win32/Tauri operations that may synchronously
/// dispatch `WindowEvent` callbacks (`Resized`, `Focused`) or nested webview calls.
fn with_controller_on_main_thread(
    app: &AppHandle,
    f: impl FnOnce(win32::ICoreWebView2Controller) + Send + 'static,
) -> bool {
    let Some(w) = app.get_webview_window("main") else {
        return false;
    };
    app.run_on_main_thread(move || {
        let _ = w.with_webview(|webview| {
            let ctrl = webview.controller().clone();
            EXTRACTED_CONTROLLER.with(|slot| {
                *slot.borrow_mut() = Some(ctrl);
            });
        });
        let controller = EXTRACTED_CONTROLLER.with(|slot| slot.borrow_mut().take());
        if let Some(controller) = controller {
            f(controller);
        }
    })
    .is_ok()
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
        // Expand to the monitor work area instead of Win32 SW_MAXIMIZE so DWM never paints
        // duplicate native caption buttons on top of Clade's custom header strip.
        let _ = w.set_resizable(!next);
        if let Some(host_id) = host_window_id(&w) {
            let session_clone = session.clone();
            let _ = with_controller_on_main_thread(&app, move |controller| {
                let host_hwnd = host_id as usize as win32::HWND;
                win32::set_work_area_expanded(host_hwnd, next, &session_clone);
                if session_clone.strip_active.load(Ordering::Acquire) {
                    if let Some(layout) = win32::layout_for_host(host_hwnd) {
                        let wry_hwnd = win32::controller_wry_hwnd(&controller);
                        win32::apply_webview_bounds(
                            &controller,
                            wry_hwnd,
                            layout.webview_width,
                            layout.webview_height,
                        );
                        win32::sync_stage_bounds_for_layout(
                            host_hwnd,
                            &session_clone,
                            layout.stage,
                            false,
                        );
                        let mc_id = session_clone.game_win.load(Ordering::Acquire);
                        if mc_id != 0 && win32::is_host_foreground(host_hwnd) {
                            win32::focus_window(mc_id as usize as win32::HWND);
                        }
                    }
                }
            });
        }
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

/// Ensures WebView2 strip and embedded stage stay synchronized when host resizes during a game.
pub fn on_host_resized(app: &AppHandle) {
    let Some(state) = app.try_state::<EmbedState>() else {
        return;
    };
    let Some(session) = state.current_session() else {
        return;
    };
    if !session.strip_active.load(Ordering::Acquire) {
        return;
    }
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let Some(host_id) = host_window_id(&w) else {
        return;
    };
    let _ = with_controller_on_main_thread(app, move |controller| {
        if !session.strip_active.load(Ordering::Acquire) {
            return;
        }
        let host_hwnd = host_id as usize as win32::HWND;
        if let Some(layout) = win32::layout_for_host(host_hwnd) {
            let wry_hwnd = win32::controller_wry_hwnd(&controller);
            win32::sync_webview_bounds_if_needed(
                &controller,
                wry_hwnd,
                layout.webview_width,
                layout.webview_height,
            );
            win32::sync_stage_bounds_for_layout(host_hwnd, &session, layout.stage, true);
        }
    });
}

/// Sends a graceful close signal to the embedded game window on the first close click.
/// Returns `true` if `CloseRequested` should be prevented so the game can save & close cleanly
/// inside the frame; a second close click detaches the game to a standalone window and falls
/// through to Clade's background-exit handling.
pub fn request_close_or_detach(app: &AppHandle) -> bool {
    let Some(embed) = app.try_state::<EmbedState>() else {
        return false;
    };
    let Some(session) = embed.current_session() else {
        return false;
    };
    let mc_id = session.game_win.load(Ordering::Acquire);
    if !session.attached.load(Ordering::Acquire) || mc_id == 0 {
        return false;
    }
    if session.close_attempts.fetch_add(1, Ordering::AcqRel) == 0 {
        session.close_requested.store(true, Ordering::Release);
        return true;
    }
    session.stop.store(true, Ordering::Release);
    win32::detach_to_standalone(mc_id as usize as win32::HWND);
    if session.mark_window_detached() {
        finalize_window_detach(app.clone(), session.clone());
    }
    let _ = embed.finish(&session.instance_id);
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
            run_watcher_win32(app, session, host_id);
        });
}

/// Tears down framed mode when the game finishes, restoring full WebView2 bounds and launcher UI.
pub fn end_session(app: &AppHandle, instance_id: &str) {
    let Some(state) = app.try_state::<EmbedState>() else {
        return;
    };
    let Some(ended) = state.finish(instance_id) else {
        return;
    };
    let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
    if ended.was_attached || ended.ever_attached || ended.strip_active {
        restore_full_webview(
            app,
            ended.attach_win_rect,
            Some(ended.prev_host_clipchildren),
            false,
        );
    }
}

fn try_attach_game_window(
    app: &AppHandle,
    session: &Arc<ActiveSession>,
    host_id: u64,
    mc_id: u64,
) -> bool {
    let Some(w) = app.get_webview_window("main") else {
        return false;
    };
    let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
    let app_clone = app.clone();
    let session_clone = session.clone();
    let w_clone = w.clone();

    if !with_controller_on_main_thread(app, move |controller| {
        if session_clone.stop.load(Ordering::Acquire) {
            let _ = tx.send(false);
            return;
        }
        let host_hwnd = host_id as usize as win32::HWND;
        let mc_hwnd = mc_id as usize as win32::HWND;
        if !win32::is_valid(host_hwnd) || !win32::is_valid(mc_hwnd) {
            let _ = tx.send(false);
            return;
        }
        let Some(layout) = win32::layout_for_host(host_hwnd) else {
            let _ = tx.send(false);
            return;
        };
        if win32::stage_screen_rect(host_hwnd, layout.stage).is_none() {
            let _ = tx.send(false);
            return;
        }

        {
            let mut rect_guard = session_clone
                .attach_win_rect
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if rect_guard.is_none() {
                *rect_guard = win32::window_rect(host_hwnd);
            }
        }
        if !session_clone.ever_attached.load(Ordering::Acquire) {
            session_clone
                .prev_host_clipchildren
                .store(win32::host_has_clip_children(host_hwnd), Ordering::Release);
        }
        win32::set_host_clip_children(host_hwnd, true);

        let wry_hwnd = win32::controller_wry_hwnd(&controller);
        win32::ensure_child_clipsiblings(wry_hwnd);

        session_clone.strip_active.store(true, Ordering::Release);
        // SAFETY: executing on the main UI STA thread owning `host_hwnd`, `wry_hwnd`, and `controller`.
        unsafe {
            win32::install_host_subclass(
                host_hwnd,
                win32::HostSubclassState {
                    controller: controller.clone(),
                    wry_hwnd,
                    session: session_clone.clone(),
                },
            );
            win32::install_wry_subclass(
                wry_hwnd,
                win32::WrySubclassState {
                    host_hwnd,
                    controller: controller.clone(),
                    session: session_clone.clone(),
                    sync_pending: std::sync::atomic::AtomicBool::new(false),
                },
            );
        }
        win32::apply_webview_bounds(
            &controller,
            wry_hwnd,
            layout.webview_width,
            layout.webview_height,
        );

        if !win32::attach_and_place(mc_hwnd, host_hwnd, layout.stage) {
            session_clone.strip_active.store(false, Ordering::Release);
            // SAFETY: still on main UI STA thread.
            unsafe {
                win32::uninstall_wry_subclass(wry_hwnd);
                win32::uninstall_host_subclass(host_hwnd);
            }
            if !session_clone.ever_attached.load(Ordering::Acquire)
                && !session_clone.prev_host_clipchildren.load(Ordering::Acquire)
            {
                win32::set_host_clip_children(host_hwnd, false);
            }
            win32::restore_webview_to_client(host_hwnd, &controller, wry_hwnd);
            let _ = tx.send(false);
            return;
        }

        session_clone.game_win.store(mc_id, Ordering::Release);
        session_clone.ever_attached.store(true, Ordering::Release);
        let newly_attached = !session_clone.attached.swap(true, Ordering::AcqRel);
        let _ = w_clone.set_resizable(true);
        if newly_attached {
            tracing::info!(
                instance = %session_clone.instance_id,
                "embedded game window into Clade stage"
            );
        }
        let _ = app_clone.emit(EMBED_EVENT, session_clone.status());
        let _ = tx.send(true);
    }) {
        return false;
    }

    rx.recv_timeout(ATTACH_TIMEOUT).unwrap_or(false)
}

fn restore_full_webview(
    app: &AppHandle,
    restore_rect: Option<(i32, i32, i32, i32)>,
    restore_clipchildren: Option<bool>,
    emit_inactive: bool,
) {
    let Some(w) = app.get_webview_window("main") else {
        if emit_inactive {
            let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
        }
        return;
    };
    let Some(host_id) = host_window_id(&w) else {
        if emit_inactive {
            let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
        }
        return;
    };
    let app_clone = app.clone();
    let w_clone = w.clone();
    let _ = with_controller_on_main_thread(app, move |controller| {
        let host_hwnd = host_id as usize as win32::HWND;
        let visible = win32::is_valid(host_hwnd);
        let wry_hwnd = win32::controller_wry_hwnd(&controller);
        // SAFETY: executing on the main UI STA thread owning `host_hwnd`, `wry_hwnd`, and `webview`.
        unsafe {
            win32::uninstall_wry_subclass(wry_hwnd);
            win32::uninstall_host_subclass(host_hwnd);
        }
        if restore_clipchildren == Some(false) {
            win32::set_host_clip_children(host_hwnd, false);
        }
        win32::set_corner_preference(host_hwnd, false);
        let _ = w_clone.set_resizable(true);
        if let Some((x, y, width, height)) = restore_rect {
            if win32::window_rect(host_hwnd) != Some((x, y, width, height)) {
                win32::set_window_rect(host_hwnd, x, y, width, height);
            }
        }

        win32::restore_webview_to_client_with_visibility(host_hwnd, &controller, wry_hwnd, visible);

        if visible {
            win32::focus_webview_if_foreground(host_hwnd, &controller);
            win32::redraw_all(host_hwnd);
            crate::webview::resume_window(&app_clone);
            crate::glass::refresh(&app_clone);
        }
        if emit_inactive {
            let _ = app_clone.emit(EMBED_EVENT, GameEmbedStatus::default());
        }
    });
}

fn finalize_window_detach(app: AppHandle, session: Arc<ActiveSession>) {
    let attach_rect = *session
        .attach_win_rect
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *session
        .pre_expand_win_rect
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    let prev_clip = session.prev_host_clipchildren.load(Ordering::Acquire);
    restore_full_webview(&app, attach_rect, Some(prev_clip), true);
}

fn run_watcher_win32(app: AppHandle, session: Arc<ActiveSession>, host_id: u64) {
    let host_hwnd = host_id as usize as win32::HWND;
    let mut delayed_focus_at: Option<std::time::Instant> = None;

    while !session.stop.load(Ordering::Acquire) {
        let mut mc_id = session.game_win.load(Ordering::Acquire);
        if mc_id != 0 && !win32::is_attached_window_alive(mc_id as usize as win32::HWND, host_hwnd)
        {
            mc_id = 0;
            delayed_focus_at = None;
            if session.mark_window_detached() {
                finalize_window_detach(app.clone(), session.clone());
            }
        }

        if mc_id == 0 {
            if let Some(found) = win32::find_game_window(session.pid, host_hwnd) {
                let found_id = found as usize as u64;
                if try_attach_game_window(&app, &session, host_id, found_id) {
                    delayed_focus_at = Some(std::time::Instant::now() + Duration::from_millis(60));
                }
            }
            std::thread::sleep(POLL_IDLE);
            continue;
        }

        let mc_hwnd = mc_id as usize as win32::HWND;
        if let Some(layout) = win32::layout_for_host(host_hwnd) {
            win32::sync_stage_bounds_for_layout(host_hwnd, &session, layout.stage, true);
        }
        if session.focus_requested.swap(false, Ordering::AcqRel) {
            delayed_focus_at = Some(std::time::Instant::now() + Duration::from_millis(60));
            if win32::should_hand_focus_to_game(host_hwnd) {
                win32::focus_window(mc_hwnd);
            }
        } else if delayed_focus_at.is_some_and(|t| std::time::Instant::now() >= t) {
            delayed_focus_at = None;
            if win32::should_hand_focus_to_game(host_hwnd) {
                win32::focus_window(mc_hwnd);
            }
        }
        if session.close_requested.swap(false, Ordering::AcqRel) {
            win32::request_close(mc_hwnd);
        }
        std::thread::sleep(POLL_ACTIVE);
    }
}

// ===================================================================================== Windows FFI

mod win32 {
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    pub use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
    use windows_sys::core::BOOL;
    pub use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_DONOTROUND,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MonitorFromWindow, RedrawWindow, ScreenToClient,
        MONITORINFO, MONITOR_DEFAULTTONEAREST, RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW,
    };
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Shell::{
        DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClientRect, GetCursorPos, GetForegroundWindow, GetParent, GetWindow,
        GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
        IsWindowVisible, PostMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
        GWLP_HWNDPARENT, GWL_EXSTYLE, GWL_STYLE, GW_HWNDNEXT, GW_HWNDPREV, GW_OWNER,
        SIZE_MINIMIZED, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WINDOWPOS, WM_APP, WM_CLOSE, WM_DPICHANGED,
        WM_EXITSIZEMOVE, WM_NCDESTROY, WM_SIZE, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING,
        WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE,
        WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE, WS_EX_TOPMOST, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WS_VISIBLE,
    };

    use super::{
        compute_framed_layout, stage_to_screen_rect, ActiveSession, FramedLayout, StageBounds,
    };

    const HWND_TOP: HWND = std::ptr::null_mut();
    const EMBED_SUBCLASS_ID: usize = 0x434C_4144_4501;
    const WRY_SUBCLASS_ID: usize = 0x434C_4144_4502;
    const WM_CLADE_SYNC_STRIP: u32 = WM_APP + 0x41;

    pub(super) struct HostSubclassState {
        pub(super) controller: ICoreWebView2Controller,
        pub(super) wry_hwnd: HWND,
        pub(super) session: Arc<ActiveSession>,
    }

    pub(super) struct WrySubclassState {
        pub(super) host_hwnd: HWND,
        pub(super) controller: ICoreWebView2Controller,
        pub(super) session: Arc<ActiveSession>,
        pub(super) sync_pending: AtomicBool,
    }

    pub unsafe fn install_host_subclass(host: HWND, state: HostSubclassState) {
        uninstall_host_subclass(host);
        let raw = Rc::into_raw(Rc::new(state));
        if SetWindowSubclass(
            host,
            Some(embed_host_subclass_proc),
            EMBED_SUBCLASS_ID,
            raw as usize,
        ) == 0
        {
            drop(Rc::from_raw(raw));
        }
    }

    pub unsafe fn uninstall_host_subclass(host: HWND) {
        let mut data: usize = 0;
        if GetWindowSubclass(
            host,
            Some(embed_host_subclass_proc),
            EMBED_SUBCLASS_ID,
            &mut data,
        ) != 0
            && RemoveWindowSubclass(host, Some(embed_host_subclass_proc), EMBED_SUBCLASS_ID) != 0
            && data != 0
        {
            drop(Rc::from_raw(data as *const HostSubclassState));
        }
    }

    unsafe fn clone_host_subclass_state(
        hwnd: HWND,
        expected: usize,
    ) -> Option<Rc<HostSubclassState>> {
        if expected == 0 || IsWindow(hwnd) == 0 {
            return None;
        }
        let mut current: usize = 0;
        if GetWindowSubclass(
            hwnd,
            Some(embed_host_subclass_proc),
            EMBED_SUBCLASS_ID,
            &mut current,
        ) == 0
            || current != expected
        {
            return None;
        }
        let ptr = expected as *const HostSubclassState;
        Rc::increment_strong_count(ptr);
        Some(Rc::from_raw(ptr))
    }

    unsafe extern "system" fn embed_host_subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        uid_subclass: usize,
        dw_ref_data: usize,
    ) -> LRESULT {
        match msg {
            WM_NCDESTROY => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if RemoveWindowSubclass(hwnd, Some(embed_host_subclass_proc), uid_subclass) != 0
                    && dw_ref_data != 0
                {
                    drop(Rc::from_raw(dw_ref_data as *const HostSubclassState));
                }
                return res;
            }
            WM_SIZE if wparam != SIZE_MINIMIZED as usize => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                    sync_strip_layout_on_host(hwnd, &state);
                }
                return res;
            }
            WM_DPICHANGED => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                    sync_strip_layout_on_host(hwnd, &state);
                }
                return res;
            }
            WM_WINDOWPOSCHANGED => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if IsWindow(hwnd) != 0 && IsIconic(hwnd) == 0 && IsWindowVisible(hwnd) != 0 {
                    if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                        sync_stage_position_on_host(hwnd, &state);
                    }
                }
                return res;
            }
            WM_EXITSIZEMOVE => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                    sync_stage_position_on_host(hwnd, &state);
                    if GetForegroundWindow() == hwnd && should_hand_focus_to_game(hwnd) {
                        let mc_id = state.session.game_win.load(Ordering::Acquire);
                        if mc_id != 0 {
                            focus_window(mc_id as usize as HWND);
                        }
                    }
                }
                return res;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    unsafe fn sync_strip_layout_on_host(hwnd: HWND, state: &HostSubclassState) {
        if !state.session.strip_active.load(Ordering::Acquire) {
            return;
        }
        let Some(layout) = layout_for_host(hwnd) else {
            return;
        };
        apply_webview_bounds(
            &state.controller,
            state.wry_hwnd,
            layout.webview_width,
            layout.webview_height,
        );
        sync_stage_bounds_for_layout(hwnd, &state.session, layout.stage, false);
    }

    unsafe fn sync_stage_position_on_host(hwnd: HWND, state: &HostSubclassState) {
        if !state.session.strip_active.load(Ordering::Acquire) {
            return;
        }
        let Some(layout) = layout_for_host(hwnd) else {
            return;
        };
        sync_stage_bounds_for_layout(hwnd, &state.session, layout.stage, true);
    }

    pub(super) fn sync_stage_bounds_for_layout(
        host: HWND,
        session: &ActiveSession,
        stage: StageBounds,
        only_if_needed: bool,
    ) {
        let mc_id = session.game_win.load(Ordering::Acquire);
        if mc_id == 0 {
            return;
        }
        let mc_hwnd = mc_id as usize as HWND;
        let Some(target) = stage_screen_rect(host, stage) else {
            return;
        };
        if !only_if_needed || needs_bounds_sync(mc_hwnd, host, target) {
            apply_screen_bounds(
                mc_hwnd,
                target.0,
                target.1,
                target.2,
                target.3,
                false,
                Some(host),
            );
        }
    }

    pub unsafe fn install_wry_subclass(wry_hwnd: HWND, state: WrySubclassState) {
        if wry_hwnd.is_null() || IsWindow(wry_hwnd) == 0 {
            return;
        }
        uninstall_wry_subclass(wry_hwnd);
        let raw = Rc::into_raw(Rc::new(state));
        if SetWindowSubclass(
            wry_hwnd,
            Some(wry_strip_subclass_proc),
            WRY_SUBCLASS_ID,
            raw as usize,
        ) == 0
        {
            drop(Rc::from_raw(raw));
        }
    }

    pub unsafe fn uninstall_wry_subclass(wry_hwnd: HWND) {
        if wry_hwnd.is_null() || IsWindow(wry_hwnd) == 0 {
            return;
        }
        let mut data: usize = 0;
        if GetWindowSubclass(
            wry_hwnd,
            Some(wry_strip_subclass_proc),
            WRY_SUBCLASS_ID,
            &mut data,
        ) != 0
            && RemoveWindowSubclass(wry_hwnd, Some(wry_strip_subclass_proc), WRY_SUBCLASS_ID) != 0
            && data != 0
        {
            drop(Rc::from_raw(data as *const WrySubclassState));
        }
    }

    unsafe fn clone_wry_subclass_state(
        hwnd: HWND,
        expected: usize,
    ) -> Option<Rc<WrySubclassState>> {
        if expected == 0 || IsWindow(hwnd) == 0 {
            return None;
        }
        let mut current: usize = 0;
        if GetWindowSubclass(
            hwnd,
            Some(wry_strip_subclass_proc),
            WRY_SUBCLASS_ID,
            &mut current,
        ) == 0
            || current != expected
        {
            return None;
        }
        let ptr = expected as *const WrySubclassState;
        Rc::increment_strong_count(ptr);
        Some(Rc::from_raw(ptr))
    }

    unsafe fn clamp_controller_bounds_if_needed(
        controller: &ICoreWebView2Controller,
        width: i32,
        height: i32,
    ) {
        let w = width.max(1);
        let h = height.max(1);
        if !controller_matches_bounds(controller, w, h) {
            let _ = controller.SetBounds(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            });
            let _ = controller.NotifyParentWindowPositionChanged();
        }
    }

    unsafe extern "system" fn wry_strip_subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        uid_subclass: usize,
        dw_ref_data: usize,
    ) -> LRESULT {
        match msg {
            WM_NCDESTROY => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if RemoveWindowSubclass(hwnd, Some(wry_strip_subclass_proc), uid_subclass) != 0
                    && dw_ref_data != 0
                {
                    drop(Rc::from_raw(dw_ref_data as *const WrySubclassState));
                }
                return res;
            }
            WM_WINDOWPOSCHANGING if lparam != 0 => {
                if let Some(state) = clone_wry_subclass_state(hwnd, dw_ref_data) {
                    if state.session.strip_active.load(Ordering::Acquire) {
                        if let Some(layout) = layout_for_host(state.host_hwnd) {
                            let wp = &mut *(lparam as *mut WINDOWPOS);
                            if (wp.flags & SWP_NOMOVE) == 0 {
                                wp.x = 0;
                                wp.y = 0;
                            }
                            if (wp.flags & SWP_NOSIZE) == 0 {
                                wp.cx = layout.webview_width.max(1);
                                wp.cy = layout.webview_height.max(1);
                            }
                            if state
                                .sync_pending
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                            {
                                let _ = PostMessageW(hwnd, WM_CLADE_SYNC_STRIP, 0, 0);
                            }
                        }
                    }
                }
            }
            WM_WINDOWPOSCHANGED => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_wry_subclass_state(hwnd, dw_ref_data) {
                    if state.session.strip_active.load(Ordering::Acquire) {
                        if let Some(layout) = layout_for_host(state.host_hwnd) {
                            clamp_controller_bounds_if_needed(
                                &state.controller,
                                layout.webview_width,
                                layout.webview_height,
                            );
                        }
                    }
                }
                return res;
            }
            WM_CLADE_SYNC_STRIP => {
                if let Some(state) = clone_wry_subclass_state(hwnd, dw_ref_data) {
                    state.sync_pending.store(false, Ordering::Release);
                    if IsWindow(hwnd) != 0 && state.session.strip_active.load(Ordering::Acquire) {
                        if let Some(layout) = layout_for_host(state.host_hwnd) {
                            clamp_controller_bounds_if_needed(
                                &state.controller,
                                layout.webview_width,
                                layout.webview_height,
                            );
                        }
                    }
                }
                return 0;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

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
        if !owner.is_null() && owner != search.host && parent != search.host {
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

    pub fn is_host_alive(hwnd: HWND) -> bool {
        // SAFETY: Win32 handles invalid pointers safely by returning 0.
        unsafe { !hwnd.is_null() && IsWindow(hwnd) != 0 }
    }

    pub fn is_valid(hwnd: HWND) -> bool {
        // SAFETY: Win32 handles non-window pointers safely by returning 0.
        unsafe { !hwnd.is_null() && IsWindow(hwnd) != 0 && IsWindowVisible(hwnd) != 0 }
    }

    pub fn layout_for_host(host: HWND) -> Option<FramedLayout> {
        let mut cr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: reads live client area dimensions and monitor DPI on `host`.
        let dpi = unsafe {
            if !is_host_alive(host) || GetClientRect(host, &mut cr) == 0 {
                return None;
            }
            let raw = GetDpiForWindow(host);
            if raw == 0 {
                96
            } else {
                raw
            }
        };
        compute_framed_layout(cr.right - cr.left, cr.bottom - cr.top, dpi)
    }

    pub fn controller_wry_hwnd(controller: &ICoreWebView2Controller) -> HWND {
        let mut raw = windows::Win32::Foundation::HWND::default();
        // SAFETY: `ParentWindow` writes WebView2's host child HWND (`WRY_WEBVIEW`).
        if unsafe { controller.ParentWindow(&mut raw) }.is_ok() {
            raw.0.cast()
        } else {
            std::ptr::null_mut()
        }
    }

    pub fn apply_webview_bounds(
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
        width: i32,
        height: i32,
    ) {
        let w = width.max(1);
        let h = height.max(1);
        // SAFETY: invoked on the main UI STA thread owning `controller` and `wry_hwnd`.
        unsafe {
            let _ = controller.SetBounds(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            });
            if !wry_hwnd.is_null() && IsWindow(wry_hwnd) != 0 {
                SetWindowPos(
                    wry_hwnd,
                    HWND_TOP,
                    0,
                    0,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            let _ = controller.SetIsVisible(true);
            let _ = controller.NotifyParentWindowPositionChanged();
        }
    }

    fn controller_matches_bounds(
        controller: &ICoreWebView2Controller,
        width: i32,
        height: i32,
    ) -> bool {
        let mut rect = windows::Win32::Foundation::RECT::default();
        // SAFETY: invoked on the main UI STA thread owning `controller`.
        unsafe { controller.Bounds(&mut rect) }.is_ok()
            && rect.left == 0
            && rect.top == 0
            && rect.right == width.max(1)
            && rect.bottom == height.max(1)
    }

    pub fn sync_webview_bounds_if_needed(
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
        width: i32,
        height: i32,
    ) {
        let ctrl_matches = controller_matches_bounds(controller, width, height);
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let hwnd_matches = !wry_hwnd.is_null()
            && unsafe { GetClientRect(wry_hwnd, &mut wr) } != 0
            && (wr.right - wr.left) == width.max(1)
            && (wr.bottom - wr.top) == height.max(1);
        if !ctrl_matches || !hwnd_matches {
            apply_webview_bounds(controller, wry_hwnd, width, height);
        }
    }

    pub fn restore_webview_to_client(
        host: HWND,
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
    ) {
        restore_webview_to_client_with_visibility(host, controller, wry_hwnd, true);
    }

    pub fn restore_webview_to_client_with_visibility(
        host: HWND,
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
        visible: bool,
    ) {
        let mut cr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: invoked on the main UI STA thread owning `host` and `controller`.
        unsafe {
            if !is_host_alive(host) || GetClientRect(host, &mut cr) == 0 {
                return;
            }
            let w = (cr.right - cr.left).max(1);
            let h = (cr.bottom - cr.top).max(1);
            let _ = controller.SetBounds(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            });
            if !wry_hwnd.is_null() && IsWindow(wry_hwnd) != 0 {
                SetWindowPos(
                    wry_hwnd,
                    HWND_TOP,
                    0,
                    0,
                    w,
                    h,
                    SWP_NOACTIVATE | if visible { SWP_SHOWWINDOW } else { 0 },
                );
            }
            if visible {
                let _ = controller.SetIsVisible(true);
            }
            let _ = controller.NotifyParentWindowPositionChanged();
        }
    }

    pub fn focus_webview_if_foreground(host: HWND, controller: &ICoreWebView2Controller) {
        // SAFETY: invoked on the main UI STA thread; raises WebView2 focus only if host is active.
        unsafe {
            if is_host_alive(host) && GetForegroundWindow() == host {
                let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
    }

    pub fn redraw_all(host: HWND) {
        // SAFETY: invalidates and repaints `host` and child surfaces on the UI thread.
        unsafe {
            if is_host_alive(host) {
                RedrawWindow(
                    host,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
                );
            }
        }
    }

    pub fn host_has_clip_children(host: HWND) -> bool {
        // SAFETY: reads style flags on live `host`.
        unsafe {
            is_host_alive(host)
                && ((GetWindowLongPtrW(host, GWL_STYLE) as u32) & WS_CLIPCHILDREN) != 0
        }
    }

    pub fn set_host_clip_children(host: HWND, enable: bool) {
        // SAFETY: toggles `WS_CLIPCHILDREN` on live `host` owned by the UI thread.
        unsafe {
            if !is_host_alive(host) {
                return;
            }
            let style = GetWindowLongPtrW(host, GWL_STYLE) as u32;
            let next = if enable {
                style | WS_CLIPCHILDREN
            } else {
                style & !WS_CLIPCHILDREN
            };
            if next != style {
                SetWindowLongPtrW(host, GWL_STYLE, next as i32 as isize);
            }
        }
    }

    pub fn ensure_child_clipsiblings(child: HWND) {
        // SAFETY: ensures `WRY_WEBVIEW` has `WS_CLIPSIBLINGS` set.
        unsafe {
            if child.is_null() || IsWindow(child) == 0 {
                return;
            }
            let style = GetWindowLongPtrW(child, GWL_STYLE) as u32;
            if (style & WS_CLIPSIBLINGS) == 0 {
                SetWindowLongPtrW(child, GWL_STYLE, (style | WS_CLIPSIBLINGS) as i32 as isize);
            }
        }
    }

    pub fn stage_screen_rect(host: HWND, stage: StageBounds) -> Option<(i32, i32, i32, i32)> {
        // SAFETY: reads host visibility/iconic state and client origin on screen.
        unsafe {
            if !is_host_alive(host) || IsIconic(host) != 0 || IsWindowVisible(host) == 0 {
                return None;
            }
            let mut origin = POINT { x: 0, y: 0 };
            if ClientToScreen(host, &mut origin) == 0 {
                return None;
            }
            Some(stage_to_screen_rect(origin.x, origin.y, stage))
        }
    }

    pub fn attach_and_place(mc: HWND, host: HWND, b: StageBounds) -> bool {
        let Some((sx, sy, sw, sh)) = stage_screen_rect(host, b) else {
            return false;
        };
        // SAFETY: valid live HWNDs belonging to the game window and launcher host.
        unsafe {
            // Assign ownership before stripping the native frame so a refused owner assignment never
            // leaves the standalone Minecraft window borderless and off the taskbar.
            SetWindowLongPtrW(mc, GWLP_HWNDPARENT, host as isize);
            if GetWindow(mc, GW_OWNER) != host {
                return false;
            }

            let style = GetWindowLongPtrW(mc, GWL_STYLE) as u32;
            let strip = WS_CAPTION
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_SYSMENU
                | WS_CHILD;
            let add = WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
            SetWindowLongPtrW(mc, GWL_STYLE, ((style & !strip) | add) as i32 as isize);

            let ex = GetWindowLongPtrW(mc, GWL_EXSTYLE) as u32;
            let strip_ex = WS_EX_APPWINDOW
                | WS_EX_DLGMODALFRAME
                | WS_EX_WINDOWEDGE
                | WS_EX_CLIENTEDGE
                | WS_EX_STATICEDGE;
            SetWindowLongPtrW(mc, GWL_EXSTYLE, (ex & !strip_ex) as i32 as isize);

            // Clear any stale region once without re-applying per resize so WGL flip swapchains stay intact.
            windows_sys::Win32::Graphics::Gdi::SetWindowRgn(mc, std::ptr::null_mut(), 0);
            set_corner_preference(mc, true);
            apply_screen_bounds(mc, sx, sy, sw, sh, true, Some(host));
            if GetForegroundWindow() == host {
                let _ = SetForegroundWindow(mc);
            }
            true
        }
    }

    pub fn is_window_above(upper: HWND, lower: HWND) -> bool {
        if upper.is_null() || lower.is_null() || upper == lower {
            return false;
        }
        // SAFETY: walks global Z-order chain starting at `upper` with a bounded iteration cap.
        unsafe {
            let mut cur = upper;
            for _ in 0..512 {
                cur = GetWindow(cur, GW_HWNDNEXT);
                if cur.is_null() {
                    return false;
                }
                if cur == lower {
                    return true;
                }
            }
        }
        false
    }

    fn insert_after_above_host(mc: HWND, host: HWND) -> HWND {
        // SAFETY: finds the non-topmost sibling immediately above `host` in Z-order.
        unsafe {
            let mut prev = GetWindow(host, GW_HWNDPREV);
            for _ in 0..16 {
                if prev.is_null()
                    || ((GetWindowLongPtrW(prev, GWL_EXSTYLE) as u32) & WS_EX_TOPMOST) != 0
                {
                    return HWND_TOP;
                }
                if prev != mc {
                    return prev;
                }
                prev = GetWindow(prev, GW_HWNDPREV);
            }
        }
        HWND_TOP
    }

    pub fn apply_screen_bounds(
        mc: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        frame_changed: bool,
        above_host: Option<HWND>,
    ) {
        let needs_z_order = above_host.is_some_and(|host| !is_window_above(mc, host));
        let insert_after = match above_host {
            Some(host) if needs_z_order => insert_after_above_host(mc, host),
            _ => HWND_TOP,
        };
        let mut flags = SWP_NOACTIVATE
            | SWP_ASYNCWINDOWPOS
            | if frame_changed {
                SWP_FRAMECHANGED | SWP_SHOWWINDOW
            } else {
                0
            };
        if !needs_z_order {
            flags |= SWP_NOZORDER;
        }
        // SAFETY: `mc` is an owned borderless top-level window; async flags avoid blocking Clade's
        // UI thread on cross-process OpenGL frame swaps.
        unsafe {
            SetWindowPos(mc, insert_after, x, y, w.max(64), h.max(64), flags);
        }
    }

    pub fn detach_to_standalone(mc: HWND) {
        // SAFETY: restores independent top-level window styles and removes `host` ownership.
        unsafe {
            if mc.is_null() || IsWindow(mc) == 0 {
                return;
            }
            SetWindowLongPtrW(mc, GWLP_HWNDPARENT, 0);
            let style = GetWindowLongPtrW(mc, GWL_STYLE) as u32;
            let next_style = (style & !(WS_POPUP | WS_CHILD)) | WS_OVERLAPPEDWINDOW | WS_VISIBLE;
            SetWindowLongPtrW(mc, GWL_STYLE, next_style as i32 as isize);
            let ex = GetWindowLongPtrW(mc, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(mc, GWL_EXSTYLE, (ex | WS_EX_APPWINDOW) as i32 as isize);
            set_corner_preference(mc, false);
            SetWindowPos(
                mc,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    pub fn is_attached_window_alive(mc: HWND, host: HWND) -> bool {
        // SAFETY: checks owned game HWND liveness while tolerating OS-automatic hide on host minimize.
        unsafe {
            if mc.is_null() || IsWindow(mc) == 0 {
                return false;
            }
            if !is_host_alive(host) || IsIconic(host) != 0 || IsWindowVisible(host) == 0 {
                return true;
            }
            IsWindowVisible(mc) != 0
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
                let saved = *session
                    .pre_expand_win_rect
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some((rx, ry, rw, rh)) = saved {
                    set_window_rect(host, rx, ry, rw, rh);
                }
            }
        }
    }

    pub fn needs_bounds_sync(mc: HWND, host: HWND, target: (i32, i32, i32, i32)) -> bool {
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: checks live owned game window visibility, owner, Z-order, and screen coordinates.
        unsafe {
            if IsWindowVisible(mc) == 0
                || GetWindow(mc, GW_OWNER) != host
                || GetWindowRect(mc, &mut wr) == 0
            {
                return false;
            }
            if !is_window_above(mc, host) {
                return true;
            }
            wr.left != target.0
                || wr.top != target.1
                || (wr.right - wr.left) != target.2
                || (wr.bottom - wr.top) != target.3
        }
    }

    pub fn is_host_foreground(host: HWND) -> bool {
        // SAFETY: queries live window handle validity and current foreground HWND.
        unsafe { is_host_alive(host) && GetForegroundWindow() == host }
    }

    pub fn should_hand_focus_to_game(host: HWND) -> bool {
        // SAFETY: checks live host focus state and avoids interrupting pointer interactions on the
        // 44px Clade top header strip.
        unsafe {
            if !is_host_foreground(host) || IsIconic(host) != 0 {
                return false;
            }
            let mut pt = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pt) != 0 && ScreenToClient(host, &mut pt) != 0 {
                if let Some(layout) = layout_for_host(host) {
                    if pt.x >= 0
                        && pt.x < layout.webview_width
                        && pt.y >= 0
                        && pt.y < layout.webview_height
                    {
                        return false;
                    }
                }
            }
            true
        }
    }

    pub fn focus_window(mc: HWND) {
        // SAFETY: activates the owned top-level game window so GLFW's `GetActiveWindow()` matches.
        unsafe {
            if !mc.is_null() && IsWindow(mc) != 0 && IsWindowVisible(mc) != 0 {
                let _ = SetForegroundWindow(mc);
            }
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
    fn compute_framed_layout_scales_cleanly_and_is_disjoint_across_dpis() {
        assert!(compute_framed_layout(0, 720, 96).is_none());
        assert!(compute_framed_layout(1280, 63, 96).is_none());
        assert!(compute_framed_layout(63, 720, 96).is_none());

        for (dpi, expected_header) in [(96, 44), (120, 55), (144, 66), (192, 88)] {
            let layout = compute_framed_layout(1280, 720, dpi).expect("valid layout");
            assert_eq!(layout.webview_width, 1280);
            assert_eq!(layout.webview_height, expected_header);
            assert_eq!(
                layout.stage,
                StageBounds {
                    x: 0,
                    y: expected_header,
                    width: 1280,
                    height: (720 - expected_header) as u32,
                    radius: 0,
                }
            );
            assert_eq!(layout.webview_height, layout.stage.y);
            assert_eq!(
                layout.webview_height as u32 + layout.stage.height,
                720,
                "rectangles must partition full host height cleanly at dpi {dpi}"
            );
        }
    }

    #[test]
    fn embed_state_tracks_early_detach_reattach_and_idempotent_finish() {
        let state = EmbedState::new();
        assert_eq!(state.status(), GameEmbedStatus::default());

        let s1 = state.begin("survival".into(), 1001);
        assert!(
            !state.is_active(),
            "must remain inactive until window attaches"
        );
        assert!(state.current_bounds().0.is_none());

        state.set_bounds(StageBounds::clamped(0, 44, 1280, 676, 0));
        let (bounds, seq1) = state.current_bounds();
        assert_eq!(
            bounds,
            Some(StageBounds {
                x: 0,
                y: 44,
                width: 1280,
                height: 676,
                radius: 0,
            })
        );
        assert_eq!(seq1, 1);

        s1.game_win.store(42, Ordering::Release);
        s1.attached.store(true, Ordering::Release);
        s1.ever_attached.store(true, Ordering::Release);
        s1.strip_active.store(true, Ordering::Release);
        s1.maximized.store(true, Ordering::Release);
        s1.prev_host_clipchildren.store(false, Ordering::Release);
        *s1.attach_win_rect.lock().unwrap() = Some((100, 100, 1280, 720));

        assert_eq!(
            state.status(),
            GameEmbedStatus {
                active: true,
                instance_id: Some("survival".into()),
                maximized: true,
            }
        );

        // Simulated early window destruction before JVM exits:
        assert!(s1.mark_window_detached());
        assert!(!s1.mark_window_detached(), "second detach call is a no-op");
        assert!(!state.is_active());
        assert_eq!(s1.game_win.load(Ordering::Acquire), 0);
        assert!(!s1.maximized.load(Ordering::Acquire));
        assert!(s1.ever_attached.load(Ordering::Acquire));
        assert_eq!(
            *s1.attach_win_rect.lock().unwrap(),
            Some((100, 100, 1280, 720))
        );

        assert!(state.finish("other").is_none());
        let ended = state.finish("survival").unwrap();
        assert!(!ended.was_attached);
        assert!(ended.ever_attached);
        assert!(!ended.strip_active);
        assert!(!ended.prev_host_clipchildren);
        assert_eq!(ended.attach_win_rect, Some((100, 100, 1280, 720)));
        assert!(s1.stop.load(Ordering::Acquire));
        assert!(!state.is_active());
    }

    #[test]
    fn stage_to_screen_rect_maps_positive_and_negative_monitor_origins() {
        let stage = StageBounds::clamped(0, 44, 1280, 676, 0);
        assert_eq!(stage_to_screen_rect(100, 200, stage), (100, 244, 1280, 676));
        assert_eq!(
            stage_to_screen_rect(-1920, 0, stage),
            (-1920, 44, 1280, 676)
        );
    }
}
