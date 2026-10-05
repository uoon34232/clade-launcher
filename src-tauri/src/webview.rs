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

/// Must run before any WebView is created.
pub fn select_runtime(root: &PortableRoot) -> RuntimeSource {
    scrub_overrides(cfg!(debug_assertions));
    let dir = root.fixed_webview2_dir();
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // The names are WebView2's own; no other test reads them.
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
}
