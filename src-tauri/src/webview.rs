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

/// Must run before any WebView is created.
pub fn select_runtime(root: &PortableRoot) -> RuntimeSource {
    let dir = root.fixed_webview2_dir();
    if dir.join("msedgewebview2.exe").is_file() {
        // Called once at startup, before any other thread exists.
        // portability-allow: points WebView2 at the runtime bundled inside the launcher folder
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &dir);
        RuntimeSource::Bundled
    } else {
        // Make sure a stray variable inherited from the environment cannot redirect us.
        // portability-allow: drops an inherited override so only the bundled runtime can be chosen
        std::env::remove_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER");
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
