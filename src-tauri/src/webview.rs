//! WebView2 portability.
//!
//! The launcher UI runs in Microsoft's WebView2. Two things would otherwise tie the folder to
//! a particular PC:
//!
//! 1. The *runtime* itself. Windows 11 ships the evergreen runtime, but Windows 10 machines may
//!    not have it. A portable release therefore bundles a **Fixed Version runtime** in
//!    `runtimes/webview2/` and points WebView2 at it with `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`.
//!    We never download or run an installer at startup.
//! 2. The *profile* (cache, cookies, local storage). WebView2 defaults to
//!    `%LOCALAPPDATA%\<identifier>\EBWebView`; we redirect it to `data/webview2/`.
//!
//! If neither a bundled nor an installed runtime is found, startup fails with a clear message
//! instead of silently falling back to anything else.

use crate::portable::PortableRoot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSource {
    /// Bundled Fixed Version runtime in `runtimes/webview2/` (the portable-release case).
    Bundled,
    /// The evergreen runtime already installed on this PC (development, or a build without the bundle).
    System,
}

/// WebView2 variables that would let the environment Clade was started from decide where its web
/// engine lives or how it runs, instead of Clade: the runtime folder and the profile folder
/// (always scrubbed), and extra browser command-line arguments (scrubbed in release builds; a
/// development build keeps this one so DevTools can be attached for debugging).
const ALWAYS_SCRUBBED: [&str; 2] = [
    "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
    "WEBVIEW2_USER_DATA_FOLDER",
];
const RELEASE_SCRUBBED: [&str; 1] = ["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"];

/// The variables removed at startup.
pub fn scrubbed_overrides(debug_build: bool) -> Vec<&'static str> {
    let mut names = ALWAYS_SCRUBBED.to_vec();
    if !debug_build {
        names.extend(RELEASE_SCRUBBED);
    }
    names
}

/// Removes inherited WebView2 overrides. Only these names are touched: nothing else Clade or Windows
/// needs is cleared.
fn scrub_overrides(debug_build: bool) {
    for name in scrubbed_overrides(debug_build) {
        // Called once at startup, before any other thread exists.
        // portability-allow: drops an inherited WebView2 override so only Clade's own choice applies
        std::env::remove_var(name);
    }
}

/// True when `path` is a Windows UNC share (`\\server\share...` or `\\?\UNC\server\share...`).
/// Chromium's Fixed Version sandbox refuses to start its renderer processes over SMB/UNC shares,
/// so Clade falls back to the machine's Evergreen WebView2 runtime in that case.
pub fn is_unc_path(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    s.get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case(r"\\?\UNC\"))
        || (s.starts_with(r"\\") && !s.starts_with(r"\\?\"))
}

/// Must run before any WebView is created.
pub fn select_runtime(root: &PortableRoot) -> RuntimeSource {
    scrub_overrides(cfg!(debug_assertions));
    let dir = root.fixed_webview2_dir();
    if is_unc_path(&dir) {
        tracing::warn!(
            "launcher is running from a UNC share; falling back to system WebView2 runtime"
        );
        return RuntimeSource::System;
    }
    if dir.join("msedgewebview2.exe").is_file() {
        // portability-allow: points WebView2 at the runtime bundled inside the launcher folder
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &dir);
        RuntimeSource::Bundled
    } else {
        RuntimeSource::System
    }
}

/// Verifies a WebView2 runtime can actually be loaded; returns its version.
#[cfg(windows)]
pub fn runtime_version() -> Result<String, String> {
    wry::webview_version().map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn runtime_version() -> Result<String, String> {
    Ok("n/a".into())
}

/// Suspends the hidden WebView2 instance so Chromium drops its working set and stops timers/GPU
/// activity while Minecraft runs with the launcher window hidden.
#[cfg(windows)]
pub fn suspend_hidden_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    };
    use webview2_com::TrySuspendCompletedHandler;
    use windows_core::Interface;

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.with_webview(|webview| {
        let controller = webview.controller();
        // SAFETY: `with_webview` runs on the main UI STA thread owning the controller.
        unsafe {
            let _ = controller.SetIsVisible(false);
        }
        if let Ok(core) = unsafe { controller.CoreWebView2() } {
            if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                unsafe {
                    let _ =
                        wv19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
                }
            }
            if let Ok(wv3) = core.cast::<ICoreWebView2_3>() {
                let handler = TrySuspendCompletedHandler::create(Box::new(|_hr, _ok| Ok(())));
                unsafe {
                    let _ = wv3.TrySuspend(&handler);
                }
            }
        }
    });
}

#[cfg(not(windows))]
pub fn suspend_hidden_window(_app: &tauri::AppHandle) {}

/// Lowers WebView2's memory target while the launcher window stays open unfocused behind a running
/// game.
#[cfg(windows)]
pub fn throttle_unfocused_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    };
    use windows_core::Interface;

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.with_webview(|webview| {
        let controller = webview.controller();
        // SAFETY: `with_webview` runs on the main UI STA thread owning the controller.
        if let Ok(core) = unsafe { controller.CoreWebView2() } {
            if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                unsafe {
                    let _ =
                        wv19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
                }
            }
        }
    });
}

#[cfg(not(windows))]
pub fn throttle_unfocused_window(_app: &tauri::AppHandle) {}

/// Restores normal memory usage, resumes from suspension, and marks the WebView2 controller
/// visible again when the launcher window returns to the screen.
#[cfg(windows)]
pub fn resume_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    use windows_core::Interface;

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.with_webview(|webview| {
        let controller = webview.controller();
        // SAFETY: `with_webview` runs on the main UI STA thread owning the controller.
        if let Ok(core) = unsafe { controller.CoreWebView2() } {
            if let Ok(wv3) = core.cast::<ICoreWebView2_3>() {
                unsafe {
                    let _ = wv3.Resume();
                }
            }
            if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                unsafe {
                    let _ = wv19
                        .SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL);
                }
            }
        }
        unsafe {
            let _ = controller.SetIsVisible(true);
        }
    });
}

#[cfg(not(windows))]
pub fn resume_window(_app: &tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn release_builds_scrub_all_three_overrides_and_nothing_else() {
        let release = scrubbed_overrides(false);
        assert_eq!(
            release,
            [
                "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
                "WEBVIEW2_USER_DATA_FOLDER",
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"
            ]
        );
        assert!(release.iter().all(|n| n.starts_with("WEBVIEW2_")));
        // a development build keeps the DevTools hook, and still protects the folders
        assert_eq!(
            scrubbed_overrides(true),
            [
                "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
                "WEBVIEW2_USER_DATA_FOLDER"
            ]
        );
    }

    #[test]
    fn inherited_overrides_are_really_removed_and_unrelated_variables_survive() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The names are WebView2's own; ENV_LOCK serializes tests touching them.
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", "somewhere-else");
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "another-runtime");
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--remote-debugging-port=1",
        );
        std::env::set_var("CLADE_TEST_UNRELATED_VARIABLE", "kept");

        scrub_overrides(true);
        assert!(std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none());
        assert!(std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none());
        assert!(
            std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_some(),
            "a development build keeps the DevTools hook"
        );

        scrub_overrides(false);
        assert!(std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_none());
        assert_eq!(
            std::env::var("CLADE_TEST_UNRELATED_VARIABLE").as_deref(),
            Ok("kept")
        );
        std::env::remove_var("CLADE_TEST_UNRELATED_VARIABLE");
    }

    #[test]
    fn unc_paths_are_detected_without_flagging_local_or_verbatim_drives() {
        assert!(is_unc_path(Path::new(r"\\server\share\Clade")));
        assert!(is_unc_path(Path::new(r"\\?\UNC\server\share\Clade")));
        assert!(is_unc_path(Path::new(r"\\?\unc\nas\games\Clade")));
        assert!(!is_unc_path(Path::new(r"D:\Games\Clade")));
        assert!(!is_unc_path(Path::new(r"\\?\D:\Games\Clade")));
        assert!(!is_unc_path(Path::new("runtimes/webview2")));
    }

    #[test]
    fn select_runtime_falls_back_to_system_on_unc_and_uses_bundled_when_local() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");

        let unc_root = PortableRoot::at(r"\\server\share\Clade");
        assert_eq!(select_runtime(&unc_root), RuntimeSource::System);
        assert!(std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none());

        let verbatim_unc = PortableRoot::at(r"\\?\UNC\server\share\Clade");
        assert_eq!(select_runtime(&verbatim_unc), RuntimeSource::System);
        assert!(std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none());

        let local = tempfile::tempdir().unwrap();
        let local_root = PortableRoot::at(local.path());
        assert_eq!(select_runtime(&local_root), RuntimeSource::System);

        let rt_dir = local_root.fixed_webview2_dir();
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::write(rt_dir.join("msedgewebview2.exe"), b"stub").unwrap();
        assert_eq!(select_runtime(&local_root), RuntimeSource::Bundled);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );
        std::env::remove_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER");
    }
}
