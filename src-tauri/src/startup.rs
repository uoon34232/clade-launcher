//! Startup sequencing and fatal-error reporting.
//!
//! The release binary has no console (`windows_subsystem = "windows"`), so a failed start
//! would be invisible. Fatal problems are shown in a native message box instead.

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Mutex;

use crate::portable::PortableRoot;
use crate::startup_text::{self, StartupMessage};

/// The one native error dialog, shown in the selected language (see `startup_text`), then exit.
/// `root` is the launcher folder when it is already known (for its saved language); `detail` is
/// an English technical line for troubleshooting and must not carry secrets or user paths.
pub fn fail(id: StartupMessage, root: Option<&Path>, detail: Option<&str>) -> ! {
    let lang = startup_text::current_language(root);
    show_error(startup_text::TITLE, &startup_text::render(id, lang, detail));
    std::process::exit(1)
}

/// Development only: `Clade.exe --preview-startup-dialog=N` shows startup message N (1-based) in
/// the language that would be used and exits, so every dialog can be read without breaking the
/// runtime or the folder. Compiled out of release builds.
#[cfg(debug_assertions)]
pub fn preview_requested(root: &Path) {
    let Some(n) = std::env::args()
        .find_map(|a| {
            a.strip_prefix("--preview-startup-dialog=")
                .map(str::to_owned)
        })
        .and_then(|n| n.parse::<usize>().ok())
    else {
        return;
    };
    let Some(id) = n
        .checked_sub(1)
        .and_then(|i| startup_text::ALL_MESSAGES.get(i))
    else {
        return;
    };
    fail(
        *id,
        Some(root),
        Some("preview of this dialog (development build)"),
    );
}

#[cfg(not(debug_assertions))]
pub fn preview_requested(_root: &Path) {}

fn show_error(title: &str, message: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |s: &str| {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        // SAFETY: both buffers are NUL-terminated and outlive the call; a null owner is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                wide(message).as_ptr(),
                wide(title).as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{title}: {message}");
}

/// Another Clade on this PC already owns the folder: bring its window back (it may be hidden because
/// it was closed while a game runs) instead of reporting an error. `false` when there is no such
/// window, which means the folder is in use somewhere else (another PC).
#[cfg(windows)]
pub fn wake_running_instance(title: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the calls; a null class name is allowed; the
    // window handle is only passed back to the same API family and checked for null first.
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        ShowWindow(hwnd, SW_SHOW);
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
        true
    }
}

#[cfg(not(windows))]
pub fn wake_running_instance(_title: &str) -> bool {
    false
}

/// Logs go to `logs/clade.log` inside the folder. One rotation keeps the stick tidy.
/// Secrets must never reach this log: only log ids and states, never tokens or passphrases.
pub fn init_logging(root: &PortableRoot) {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    let path = root.logs_dir().join("clade.log");
    if fs::metadata(&path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(&path, root.logs_dir().join("clade.old.log"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = tracing_subscriber::fmt()
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .try_init();
    }
}
