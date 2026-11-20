//! WebView2 portability.
//!
//! The launcher UI runs in Microsoft's WebView2. Two things would otherwise tie the folder to
//! a particular PC:
//!
//! 1. The *runtime* itself. Windows 11 ships the evergreen runtime, but Windows 10 machines may
//!    not have it. A portable release therefore bundles a **Fixed Version runtime** in
//!    `runtimes/webview2/` and points WebView2 at it with `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`.
//!    On Windows 10, Chromium's AppContainer sandbox requires read and execute (`RX`) access on
//!    that folder for `ALL APPLICATION PACKAGES` (`S-1-15-2-1`) and
//!    `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`); Clade grants those ACLs idempotently
//!    on ACL-capable volumes and falls back to the machine's Evergreen runtime on Windows 10
//!    FAT32/exFAT volumes that lack persistent ACLs. We never download or run an installer at
//!    startup.
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

fn set_bundled_folder(dir: &std::path::Path) {
    // portability-allow: points WebView2 at the runtime bundled inside the launcher folder
    std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", dir);
}

#[cfg(windows)]
mod win10_acl {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{LocalFree, GENERIC_EXECUTE, GENERIC_READ, HLOCAL};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
        EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID,
        TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetAce, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR,
        PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE: u8 = 1;
    const GENERIC_ALL_MASK: u32 = 0x1000_0000;
    pub(super) const RX_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

    /// `ALL APPLICATION PACKAGES` (`S-1-15-2-1`).
    pub(super) const SID_ALL_APP_PACKAGES: &str = "S-1-15-2-1";
    /// `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`).
    pub(super) const SID_ALL_RESTRICTED_APP_PACKAGES: &str = "S-1-15-2-2";

    pub(super) struct LocalMem(pub(super) HLOCAL);

    impl Drop for LocalMem {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by a Win32 API documented to be freed with
                // `LocalFree` (`ConvertStringSidToSidW`, `GetNamedSecurityInfoW`, or
                // `SetEntriesInAclW`).
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    pub(super) fn parse_sid(sid_str: &str) -> Result<LocalMem, String> {
        let wide: Vec<u16> = sid_str.encode_utf16().chain(Some(0)).collect();
        let mut sid: PSID = std::ptr::null_mut();
        let ok;
        // SAFETY: `wide` is NUL-terminated and valid; `sid` receives a buffer freed via
        // `LocalFree` by `LocalMem`.
        unsafe {
            ok = ConvertStringSidToSidW(wide.as_ptr(), &mut sid);
        }
        if ok == 0 || sid.is_null() {
            return Err(format!("ConvertStringSidToSidW({sid_str}) failed"));
        }
        Ok(LocalMem(sid))
    }

    const fn expand_file_rights(mask: u32) -> u32 {
        let mut out = mask;
        if (mask & GENERIC_READ) != 0 {
            out |= FILE_GENERIC_READ;
        }
        if (mask & GENERIC_EXECUTE) != 0 {
            out |= FILE_GENERIC_EXECUTE;
        }
        if (mask & GENERIC_ALL_MASK) != 0 {
            out |= RX_MASK;
        }
        out
    }

    /// Returns `(self_rights, oi_rights, ci_rights)` granted to `target_sid` by `dacl`, or
    /// `(0, 0, 0)` when `dacl` is null or any deny ACE for `target_sid` blocks `RX_MASK`.
    ///
    /// # Safety
    /// `dacl` must be null or a valid `ACL` pointer, and `target_sid` must be a valid `PSID`.
    pub(super) unsafe fn dacl_rx_rights(dacl: *mut ACL, target_sid: PSID) -> (u32, u32, u32) {
        if dacl.is_null() {
            return (0, 0, 0);
        }
        let mut self_rights = 0u32;
        let mut oi_rights = 0u32;
        let mut ci_rights = 0u32;
        // SAFETY: `dacl` was checked non-null and points to a valid `ACL` header.
        let count = unsafe { u32::from((*dacl).AceCount) };
        for i in 0..count {
            let mut ace_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            // SAFETY: `dacl` is valid and `i < AceCount`; `ace_ptr` receives a pointer inside
            // `dacl`.
            let got_ace = unsafe { GetAce(dacl, i, &mut ace_ptr) };
            if got_ace == 0 || ace_ptr.is_null() {
                continue;
            }
            // SAFETY: `GetAce` succeeded, so `ace_ptr` begins with a valid `ACE_HEADER`.
            let header = unsafe { &*ace_ptr.cast::<ACE_HEADER>() };
            if header.AceType == ACCESS_DENIED_ACE_TYPE {
                // `ACCESS_DENIED_ACE` has the same layout (`Header`, `Mask`, `SidStart`) as
                // `ACCESS_ALLOWED_ACE`. Derive the SID pointer via `addr_of_mut!` so pointer
                // provenance spans the full variable-length SID rather than 4-byte `SidStart`.
                let ace = ace_ptr.cast::<ACCESS_ALLOWED_ACE>();
                // SAFETY: `ace` points to a valid allow/deny ACE whose inline SID starts at
                // `SidStart`.
                unsafe {
                    let ace_sid: PSID = std::ptr::addr_of_mut!((*ace).SidStart).cast();
                    if EqualSid(ace_sid, target_sid) != 0
                        && (expand_file_rights((*ace).Mask) & RX_MASK) != 0
                    {
                        return (0, 0, 0);
                    }
                }
            } else if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
                let ace = ace_ptr.cast::<ACCESS_ALLOWED_ACE>();
                // SAFETY: `ace` points to a valid `ACCESS_ALLOWED_ACE` whose inline SID starts at
                // `SidStart`.
                unsafe {
                    let ace_sid: PSID = std::ptr::addr_of_mut!((*ace).SidStart).cast();
                    if EqualSid(ace_sid, target_sid) != 0 {
                        let rights = expand_file_rights((*ace).Mask);
                        let flags = u32::from(header.AceFlags);
                        if (flags & INHERIT_ONLY_ACE) == 0 {
                            self_rights |= rights;
                        }
                        if (flags & OBJECT_INHERIT_ACE) != 0 {
                            oi_rights |= rights;
                        }
                        if (flags & CONTAINER_INHERIT_ACE) != 0 {
                            ci_rights |= rights;
                        }
                    }
                }
            }
        }
        (self_rights, oi_rights, ci_rights)
    }

    /// True when `dacl` grants `RX_MASK` to `target_sid` on the directory itself and via both
    /// `(OI)` and `(CI)` inheritance (whether stored as a single ACE or split by NTFS into a
    /// folder ACE plus an `INHERIT_ONLY_ACE` child ACE).
    ///
    /// # Safety
    /// `dacl` must be null or a valid `ACL` pointer, and `target_sid` must be a valid `PSID`.
    pub(super) unsafe fn dacl_grants_inheritable_rx(dacl: *mut ACL, target_sid: PSID) -> bool {
        // SAFETY: caller upholds `dacl` and `target_sid` validity.
        let (self_rights, oi_rights, ci_rights) = unsafe { dacl_rx_rights(dacl, target_sid) };
        (self_rights & RX_MASK) == RX_MASK
            && (oi_rights & RX_MASK) == RX_MASK
            && (ci_rights & RX_MASK) == RX_MASK
    }

    fn explicit_rx_entry(sid: PSID) -> EXPLICIT_ACCESS_W {
        EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_READ | GENERIC_EXECUTE,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                ptstrName: sid.cast(),
            },
        }
    }

    pub(super) fn encode_verbatim_nul(path: &Path) -> Vec<u16> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let verbatim = crate::system::to_verbatim_win32(&abs);
        verbatim.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub(super) fn ensure_appcontainer_rx_acl(dir: &Path) -> Result<bool, String> {
        let sid_app_pkg = parse_sid(SID_ALL_APP_PACKAGES)?;
        let sid_restricted_pkg = parse_sid(SID_ALL_RESTRICTED_APP_PACKAGES)?;
        let wide = encode_verbatim_nul(dir);

        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and valid; `dacl` points inside `sd`, which `_sd_guard`
        // frees via `LocalFree` on drop.
        let err = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        let _sd_guard = LocalMem(sd);
        if err != 0 {
            return Err(format!(
                "GetNamedSecurityInfoW failed with Win32 error {err}"
            ));
        }

        // SAFETY: `dacl` is null or valid within `_sd_guard`, and both SIDs are valid within their
        // `LocalMem` guards.
        let already_granted = !dacl.is_null()
            && unsafe {
                dacl_grants_inheritable_rx(dacl, sid_app_pkg.0)
                    && dacl_grants_inheritable_rx(dacl, sid_restricted_pkg.0)
            };
        if already_granted {
            return Ok(false);
        }

        let entries = [
            explicit_rx_entry(sid_app_pkg.0),
            explicit_rx_entry(sid_restricted_pkg.0),
        ];
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let err;
        // SAFETY: `entries` holds 2 initialized structs whose `ptstrName` SIDs outlive `new_dacl`;
        // `dacl` is null or valid; `_new_dacl_guard` frees `new_dacl` via `LocalFree`.
        unsafe {
            err = SetEntriesInAclW(2, entries.as_ptr(), dacl, &mut new_dacl);
        }
        let _new_dacl_guard = LocalMem(new_dacl.cast());
        if err != 0 {
            return Err(format!("SetEntriesInAclW failed with Win32 error {err}"));
        }

        // SAFETY: `wide` is NUL-terminated and valid; `new_dacl` is a valid ACL kept alive by
        // `_new_dacl_guard` for the duration of the call.
        let err = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null_mut(),
            )
        };
        if err != 0 {
            return Err(format!(
                "SetNamedSecurityInfoW failed with Win32 error {err}"
            ));
        }
        Ok(true)
    }
}

/// Ensures `ALL APPLICATION PACKAGES` (`S-1-15-2-1`) and
/// `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`) hold inheritable read and execute (`RX`)
/// permissions on `dir` so WebView2's AppContainer renderer can load from it on Windows 10.
/// Returns `Ok(true)` when ACLs were updated and `Ok(false)` when they were already present.
#[cfg(windows)]
pub fn ensure_appcontainer_rx_acl(dir: &std::path::Path) -> Result<bool, String> {
    win10_acl::ensure_appcontainer_rx_acl(dir)
}

#[cfg(not(windows))]
pub fn ensure_appcontainer_rx_acl(_dir: &std::path::Path) -> Result<bool, String> {
    Ok(false)
}

fn select_runtime_with(
    root: &PortableRoot,
    is_windows_10: bool,
    volume_supports_acls: impl FnOnce(&std::path::Path) -> Option<bool>,
    ensure_acl: impl FnOnce(&std::path::Path) -> Result<bool, String>,
    has_system_runtime: impl FnOnce() -> bool,
) -> RuntimeSource {
    scrub_overrides(cfg!(debug_assertions));
    let dir = root.fixed_webview2_dir();
    if is_unc_path(&dir) {
        tracing::warn!(
            "launcher is running from a UNC share; falling back to system WebView2 runtime"
        );
        return RuntimeSource::System;
    }
    if !dir.join("msedgewebview2.exe").is_file() {
        return RuntimeSource::System;
    }
    if !is_windows_10 {
        set_bundled_folder(&dir);
        return RuntimeSource::Bundled;
    }
    if volume_supports_acls(&dir) == Some(false) {
        if has_system_runtime() {
            tracing::warn!(
                "bundled WebView2 folder is on a volume without persistent ACLs on Windows 10; falling back to system Evergreen WebView2 runtime"
            );
            return RuntimeSource::System;
        }
        tracing::warn!(
            "bundled WebView2 folder is on a volume without persistent ACLs on Windows 10 and no system WebView2 runtime was found; attempting bundled runtime"
        );
        set_bundled_folder(&dir);
        return RuntimeSource::Bundled;
    }
    match ensure_acl(&dir) {
        Ok(_) => {
            set_bundled_folder(&dir);
            RuntimeSource::Bundled
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not grant AppContainer ACL on bundled WebView2 folder on Windows 10"
            );
            if has_system_runtime() {
                RuntimeSource::System
            } else {
                set_bundled_folder(&dir);
                RuntimeSource::Bundled
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
const LINUX_NVIDIA_WEBKIT_ENV: &[(&str, &str)] = &[
    ("__NV_DISABLE_EXPLICIT_SYNC", "1"),
    ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
];

#[cfg(any(target_os = "linux", test))]
fn apply_linux_webkit_env_with(nvidia_present: bool) {
    if !nvidia_present {
        return;
    }
    for &(key, val) in LINUX_NVIDIA_WEBKIT_ENV {
        // Called once at startup while single-threaded, before GTK/WebKitGTK initializes.
        // Respects any explicit value the user set in their environment.
        // portability-allow: guards WebKitGTK against Wayland protocol error 71 and blank surfaces on NVIDIA drivers
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, val);
        }
    }
}

#[cfg(target_os = "linux")]
fn apply_linux_webkit_env() {
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists()
        || std::path::Path::new("/dev/nvidiactl").exists();
    apply_linux_webkit_env_with(nvidia);
}

/// Must run before any WebView is created.
pub fn select_runtime(root: &PortableRoot) -> RuntimeSource {
    #[cfg(target_os = "linux")]
    apply_linux_webkit_env();
    select_runtime_with(
        root,
        crate::system::is_windows_10(),
        crate::system::volume_supports_acls,
        ensure_appcontainer_rx_acl,
        || runtime_version().is_ok(),
    )
}

/// Verifies a WebView2 runtime can actually be loaded; returns its version.
#[cfg(windows)]
pub fn runtime_version() -> Result<String, String> {
    wry::webview_version().map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn runtime_version() -> Result<String, String> {
    tauri::webview_version().map_err(|e| e.to_string())
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

/// Disposable Chromium component-updater and telemetry directories inside `data/webview2/EBWebView`
/// that serve no purpose in a local Tauri UI (DRM modules, speech models, URL blocklists, CRX
/// caches, and metrics buffers — easily 60+ MB on a USB drive).
const DISPOSABLE_EBWEBVIEW_DIRS: &[&str] = &[
    "WidevineCdm",
    "Speech Recognition",
    "Subresource Filter",
    "component_crx_cache",
    "extensions_crx_cache",
    "BrowserMetrics",
    "ZxcvbnData",
    "OptimizationHints",
    "SafetyTips",
    "MEIPreload",
    "OnDeviceHeadSuggestModel",
    "Crowd Deny",
    "SSLErrorAssistant",
];

const DISPOSABLE_EBWEBVIEW_FILES: &[&str] = &["BrowserMetrics-spare.pma"];

/// Removes disposable Chromium component-updater and telemetry bloat from `data/webview2/EBWebView`
/// before WebView2 starts, while keeping `Default`, `Local State`, and compiled shader caches
/// untouched. Never traverses symlinks or NTFS junctions.
pub fn sweep_profile_bloat(root: &PortableRoot) {
    use crate::portable::fsutil::{is_link, reject_links_between};

    let eb = root.webview_data_dir().join("EBWebView");
    if !eb.exists() {
        return;
    }
    if reject_links_between(root.path(), &eb).is_err() || !matches!(is_link(&eb), Ok(false)) {
        return;
    }

    for dir_name in DISPOSABLE_EBWEBVIEW_DIRS {
        let target = eb.join(dir_name);
        if target.exists() && matches!(is_link(&target), Ok(false)) {
            let _ = std::fs::remove_dir_all(&target);
        }
    }
    for file_name in DISPOSABLE_EBWEBVIEW_FILES {
        let target = eb.join(file_name);
        if target.exists() && matches!(is_link(&target), Ok(false)) {
            let _ = std::fs::remove_file(&target);
        }
    }
    let crash_reports = eb.join("Crashpad").join("reports");
    if crash_reports.exists()
        && reject_links_between(&eb, &crash_reports).is_ok()
        && matches!(is_link(&crash_reports), Ok(false))
    {
        if let Ok(entries) = std::fs::read_dir(&crash_reports) {
            for entry in entries.flatten() {
                let path = entry.path();
                if matches!(is_link(&path), Ok(false)) {
                    if path.is_dir() {
                        let _ = std::fs::remove_dir_all(&path);
                    } else {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
    }
}

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

    #[cfg(windows)]
    fn inspect_rx_on_path(path: &Path, sid_str: &str) -> Result<(u32, bool), String> {
        use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
        use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};

        let sid = win10_acl::parse_sid(sid_str)?;
        let wide = win10_acl::encode_verbatim_nul(path);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and valid; `_sd_guard` frees `sd` via `LocalFree`.
        let err = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        let _sd_guard = win10_acl::LocalMem(sd);
        if err != 0 {
            return Err(format!(
                "GetNamedSecurityInfoW failed with Win32 error {err}"
            ));
        }
        // SAFETY: `dacl` is null or valid within `_sd_guard`, and `sid` is valid.
        unsafe {
            let (self_rights, _, _) = win10_acl::dacl_rx_rights(dacl, sid.0);
            let inheritable = win10_acl::dacl_grants_inheritable_rx(dacl, sid.0);
            Ok((self_rights, inheritable))
        }
    }

    #[cfg(windows)]
    #[test]
    fn appcontainer_rx_acl_is_granted_idempotently_and_inherited_by_children() {
        let tmp = tempfile::tempdir().unwrap();
        let rt_dir = tmp.path().join("runtimes").join("webview2");
        std::fs::create_dir_all(&rt_dir).unwrap();
        let preexisting = rt_dir.join("EmbeddedBrowserWebView.dll");
        std::fs::write(&preexisting, b"dll").unwrap();

        assert_eq!(ensure_appcontainer_rx_acl(&rt_dir), Ok(true));
        for sid in [
            win10_acl::SID_ALL_APP_PACKAGES,
            win10_acl::SID_ALL_RESTRICTED_APP_PACKAGES,
        ] {
            let (self_rights, inheritable) = inspect_rx_on_path(&rt_dir, sid).unwrap();
            assert_eq!(self_rights & win10_acl::RX_MASK, win10_acl::RX_MASK);
            assert!(inheritable, "expected inheritable RX on {sid}");

            let (pre_rights, _) = inspect_rx_on_path(&preexisting, sid).unwrap();
            assert_eq!(pre_rights & win10_acl::RX_MASK, win10_acl::RX_MASK);
        }

        // Second call is a no-op once both SIDs already hold inheritable RX.
        assert_eq!(ensure_appcontainer_rx_acl(&rt_dir), Ok(false));

        // Child subdirectory and child file created afterwards inherit RX automatically.
        let child_dir = rt_dir.join("EBWebView");
        std::fs::create_dir_all(&child_dir).unwrap();
        let child_file = rt_dir.join("msedgewebview2.exe");
        std::fs::write(&child_file, b"exe").unwrap();

        for sid in [
            win10_acl::SID_ALL_APP_PACKAGES,
            win10_acl::SID_ALL_RESTRICTED_APP_PACKAGES,
        ] {
            let (file_rights, _) = inspect_rx_on_path(&child_file, sid).unwrap();
            assert_eq!(
                file_rights & win10_acl::RX_MASK,
                win10_acl::RX_MASK,
                "child file should inherit effective RX for {sid}"
            );

            let (dir_rights, dir_inheritable) = inspect_rx_on_path(&child_dir, sid).unwrap();
            assert_eq!(dir_rights & win10_acl::RX_MASK, win10_acl::RX_MASK);
            assert!(
                dir_inheritable,
                "child directory should inherit inheritable RX for {sid}"
            );
        }
    }

    #[test]
    fn select_runtime_decision_matrix_covers_win11_win10_fat32_ntfs_and_acl_errors() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // 1. UNC share always falls back to System without touching ACLs or setting env var.
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        let unc_root = PortableRoot::at(r"\\server\share\Clade");
        let mut acl_calls = 0usize;
        assert_eq!(
            select_runtime_with(
                &unc_root,
                true,
                |_| Some(true),
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || false,
            ),
            RuntimeSource::System
        );
        assert_eq!(acl_calls, 0);
        assert!(std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none());

        // 2. Missing bundled executable falls back to System without touching ACLs.
        let local = tempfile::tempdir().unwrap();
        let local_root = PortableRoot::at(local.path());
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| Some(true),
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || false,
            ),
            RuntimeSource::System
        );
        assert_eq!(acl_calls, 0);
        assert!(std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none());

        // Populate bundled msedgewebview2.exe for the remaining matrix rows.
        let rt_dir = local_root.fixed_webview2_dir();
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::write(rt_dir.join("msedgewebview2.exe"), b"stub").unwrap();

        // 3. Windows 11 on NTFS uses Bundled without calling ACL grant.
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                false,
                |_| Some(true),
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || true,
            ),
            RuntimeSource::Bundled
        );
        assert_eq!(acl_calls, 0);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );

        // 4. Windows 11 on FAT32/exFAT still uses Bundled without calling ACL grant.
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                false,
                |_| Some(false),
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || true,
            ),
            RuntimeSource::Bundled
        );
        assert_eq!(acl_calls, 0);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );

        // 5. Windows 10 on FAT32/exFAT + Evergreen present -> System.
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| Some(false),
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || true,
            ),
            RuntimeSource::System
        );
        assert_eq!(acl_calls, 0);
        assert!(std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none());

        // 6. Windows 10 on FAT32/exFAT + Evergreen missing -> Bundled fallback.
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| Some(false),
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || false,
            ),
            RuntimeSource::Bundled
        );
        assert_eq!(acl_calls, 0);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );

        // 7. Windows 10 on NTFS + ACL newly granted Ok(true) -> Bundled.
        acl_calls = 0;
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| Some(true),
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || true,
            ),
            RuntimeSource::Bundled
        );
        assert_eq!(acl_calls, 1);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );

        // 8. Windows 10 on NTFS + ACL already present Ok(false) -> Bundled.
        acl_calls = 0;
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| Some(true),
                |_| {
                    acl_calls += 1;
                    Ok(false)
                },
                || true,
            ),
            RuntimeSource::Bundled
        );
        assert_eq!(acl_calls, 1);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );

        // 9. Windows 10 on unknown volume capability (None) + ACL Ok -> Bundled.
        acl_calls = 0;
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| None,
                |_| {
                    acl_calls += 1;
                    Ok(true)
                },
                || true,
            ),
            RuntimeSource::Bundled
        );
        assert_eq!(acl_calls, 1);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );

        // 10. Windows 10 on NTFS + ACL Err + Evergreen present -> System.
        acl_calls = 0;
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| Some(true),
                |_| {
                    acl_calls += 1;
                    Err("access denied".into())
                },
                || true,
            ),
            RuntimeSource::System
        );
        assert_eq!(acl_calls, 1);
        assert!(std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none());

        // 11. Windows 10 on NTFS + ACL Err + Evergreen missing -> Bundled last-resort attempt.
        acl_calls = 0;
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "stale-override");
        assert_eq!(
            select_runtime_with(
                &local_root,
                true,
                |_| Some(true),
                |_| {
                    acl_calls += 1;
                    Err("access denied".into())
                },
                || false,
            ),
            RuntimeSource::Bundled
        );
        assert_eq!(acl_calls, 1);
        assert_eq!(
            std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").as_deref(),
            Some(rt_dir.as_os_str())
        );

        std::env::remove_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER");
    }

    #[test]
    fn sweep_profile_bloat_removes_disposable_components_and_preserves_shaders_and_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(tmp.path().join("Clade"));
        root.ensure_data_dir().unwrap();
        let eb = root.webview_data_dir().join("EBWebView");

        // Bloat that Chromium's component updater / metrics write to EBWebView.
        for bloat_dir in [
            "WidevineCdm/4.10.3112.0",
            "Speech Recognition/1.15.0.1",
            "Subresource Filter/Indexed Rules",
            "component_crx_cache",
            "BrowserMetrics",
            "ZxcvbnData",
        ] {
            let dir = eb.join(bloat_dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("payload.bin"), vec![0u8; 64]).unwrap();
        }
        std::fs::write(eb.join("BrowserMetrics-spare.pma"), vec![0u8; 64]).unwrap();
        let reports = eb.join("Crashpad").join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("minidump.dmp"), b"dump").unwrap();

        // Useful state that must remain untouched for fast cold starts.
        let shaders = eb.join("GrShaderCache");
        let default_profile = eb.join("Default");
        std::fs::create_dir_all(&shaders).unwrap();
        std::fs::create_dir_all(&default_profile).unwrap();
        std::fs::write(shaders.join("data_1"), b"shader").unwrap();
        std::fs::write(default_profile.join("Preferences"), b"{}").unwrap();
        std::fs::write(eb.join("Local State"), b"{}").unwrap();

        sweep_profile_bloat(&root);

        for gone in [
            "WidevineCdm",
            "Speech Recognition",
            "Subresource Filter",
            "component_crx_cache",
            "BrowserMetrics",
            "ZxcvbnData",
            "BrowserMetrics-spare.pma",
        ] {
            assert!(!eb.join(gone).exists(), "{gone} should have been swept");
        }
        assert!(!reports.join("minidump.dmp").exists());
        assert!(shaders.join("data_1").is_file());
        assert!(default_profile.join("Preferences").is_file());
        assert!(eb.join("Local State").is_file());
    }

    #[test]
    fn linux_nvidia_webkit_guards_apply_only_when_nvidia_present_and_unset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(&str, Option<std::ffi::OsString>)> = LINUX_NVIDIA_WEBKIT_ENV
            .iter()
            .map(|&(k, _)| (k, std::env::var_os(k)))
            .collect();
        for &(k, _) in LINUX_NVIDIA_WEBKIT_ENV {
            std::env::remove_var(k);
        }

        apply_linux_webkit_env_with(false);
        for &(k, _) in LINUX_NVIDIA_WEBKIT_ENV {
            assert!(
                std::env::var_os(k).is_none(),
                "{k} should remain unset without NVIDIA"
            );
        }

        std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "0");
        apply_linux_webkit_env_with(true);
        assert_eq!(
            std::env::var("__NV_DISABLE_EXPLICIT_SYNC").as_deref(),
            Ok("0"),
            "explicit user override must be preserved"
        );
        assert_eq!(
            std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").as_deref(),
            Ok("1")
        );

        for (k, prev) in saved {
            if let Some(v) = prev {
                std::env::set_var(k, v);
            } else {
                std::env::remove_var(k);
            }
        }
    }
}
