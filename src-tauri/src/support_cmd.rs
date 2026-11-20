//! The Support actions in Settings: open the Clade folder, open the logs folder, export diagnostics.
//!
//! Every action is fixed: the folders are the launcher's own (never a path from the frontend) and
//! the diagnostics ZIP is written to `logs/diagnostics/` from an allowlist (see `diagnostics.rs`).
//! Failures are stable codes the frontend words in the selected language.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::auth::{lock, AuthState};
use crate::coded_error::CodedError;
use crate::diagnostics::{self, Exported, Facts};
use crate::portable::fsutil::{
    is_link, is_transient_win_lock, reject_links_between, remove_file_with_retry,
};
use crate::portable::PortableRoot;
use crate::settings::LauncherSettings;
use crate::ui_settings::{SettingsState, UiSettingsDto};
use crate::vault_recovery::RETIRED_DIR;
use crate::AppState;

async fn run_blocking<T, F>(code: &'static str, work: F) -> Result<T, CodedError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CodedError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| CodedError::new(code, e.to_string()))?
}

fn open_folder(app: &AppHandle, dir: &Path) -> Result<(), CodedError> {
    fs::create_dir_all(dir).map_err(CodedError::storage)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| CodedError::new("open_failed", e.to_string()))
}

fn build_export(
    root: &PortableRoot,
    launcher_settings: &LauncherSettings,
    webview: &str,
    webview_version: &str,
    now_unix: i64,
) -> Result<(Exported, PathBuf), CodedError> {
    let dto = UiSettingsDto::from_settings(launcher_settings);
    diagnostics::export(
        root,
        launcher_settings,
        &Facts {
            version: env!("CARGO_PKG_VERSION"),
            layout_version: crate::portable::root::LAYOUT_VERSION,
            webview,
            webview_version,
            system_locale: dto.system_locale.as_deref(),
            total_memory_mb: dto.memory.total,
            free_disk_bytes: crate::system::free_disk_bytes(root.path()),
            now_unix,
        },
    )
    .map_err(CodedError::from)
}

/// Opens the folder that contains Clade.exe and everything Clade manages.
#[tauri::command]
pub async fn open_clade_folder(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), CodedError> {
    let dir = state.root.path().to_path_buf();
    let work = move || open_folder(&app, &dir);
    run_blocking("open_failed", work).await
}

/// Opens `logs/`.
#[tauri::command]
pub async fn open_logs_folder(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), CodedError> {
    let dir = state.root.logs_dir();
    let work = move || open_folder(&app, &dir);
    run_blocking("open_failed", work).await
}

/// Creates the diagnostics ZIP and shows it in its folder. Returns only its file name.
#[tauri::command]
pub async fn export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: State<'_, SettingsState>,
) -> Result<Exported, CodedError> {
    let root = state.root.clone();
    let webview = format!("{:?}", state.webview);
    let webview_version = state.webview_version.clone();
    let launcher_settings = settings.settings_copy();
    run_blocking("diagnostics_failed", move || {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let (exported, path) =
            build_export(&root, &launcher_settings, &webview, &webview_version, now)?;
        // Showing the file is a courtesy: the export itself already succeeded.
        if let Err(e) = app.opener().reveal_item_in_dir(&path) {
            tracing::warn!(error = %e, "could not show the diagnostics file");
        }
        tracing::info!(name = %exported.name, "diagnostics exported");
        Ok(exported)
    })
    .await
}

fn retired_vaults_dir(root: &PortableRoot) -> PathBuf {
    root.data_dir().join(RETIRED_DIR)
}

fn is_lower_hex_12(s: &str) -> bool {
    s.len() == 12
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_retired_vault_dir_name(name: &str) -> bool {
    name.split_once('-').is_some_and(|(ts, hex)| {
        !ts.is_empty()
            && ts.len() <= 20
            && ts.bytes().all(|b| b.is_ascii_digit())
            && is_lower_hex_12(hex)
    })
}

fn is_retired_credential_file_name(name: &str) -> bool {
    if name == "vault.bin" || name == "vault.bin.bak" {
        return true;
    }
    name.strip_prefix("vault.bin.")
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(is_lower_hex_12)
}

fn remove_empty_dir_with_retry(path: &Path) -> io::Result<()> {
    const RETRY_DELAYS_MS: &[u64] = &[10, 25, 50, 100, 200, 400, 750];
    let mut attempt = 0usize;
    loop {
        match fs::remove_dir(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if is_transient_win_lock(&e) && attempt < RETRY_DELAYS_MS.len() => {
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

pub(crate) fn count_retired_vaults(root: &PortableRoot) -> Result<u32, CodedError> {
    let dir = retired_vaults_dir(root);
    reject_links_between(root.path(), &dir).map_err(CodedError::storage)?;
    let entries = match fs::read_dir(&dir) {
        Ok(it) => it,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(CodedError::storage(e)),
    };
    let mut count: u32 = 0;
    for entry in entries {
        let entry = entry.map_err(CodedError::storage)?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !is_retired_vault_dir_name(name) {
            continue;
        }
        let path = entry.path();
        if !is_link(&path).unwrap_or(true) && fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir())
        {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

pub(crate) fn clear_retired_vaults_in(root: &PortableRoot) -> Result<u32, CodedError> {
    let fail = |m: String| CodedError::new("retired_vaults_clear_failed", m);
    let dir = retired_vaults_dir(root);
    reject_links_between(root.path(), &dir).map_err(|e| fail(e.to_string()))?;
    match fs::symlink_metadata(&dir) {
        Ok(md) if md.is_dir() && !is_link(&dir).unwrap_or(true) => {}
        Ok(_) => return Err(fail("retired-vaults is not a plain directory".into())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(fail(e.to_string())),
    }

    // Pass 1: validate every matching `<ts>-<12 hex>` archive and every child file inside it
    // before deleting anything so unexpected entries or reparse points abort cleanly.
    let mut archives: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| fail(e.to_string()))? {
        let entry = entry.map_err(|e| fail(e.to_string()))?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !is_retired_vault_dir_name(name) {
            continue;
        }
        let archive_dir = entry.path();
        let md = fs::symlink_metadata(&archive_dir).map_err(|e| fail(e.to_string()))?;
        if !md.is_dir() || is_link(&archive_dir).unwrap_or(true) {
            return Err(fail("unexpected entry in retired-vaults".into()));
        }
        let mut files = Vec::new();
        for child in fs::read_dir(&archive_dir).map_err(|e| fail(e.to_string()))? {
            let child = child.map_err(|e| fail(e.to_string()))?;
            let child_name = child.file_name();
            let Some(cn) = child_name.to_str() else {
                return Err(fail("unexpected entry in retired vault archive".into()));
            };
            if !is_retired_credential_file_name(cn) {
                return Err(fail("unexpected entry in retired vault archive".into()));
            }
            let child_path = child.path();
            let child_md = fs::symlink_metadata(&child_path).map_err(|e| fail(e.to_string()))?;
            if !child_md.is_file() || is_link(&child_path).unwrap_or(true) {
                return Err(fail("unexpected entry in retired vault archive".into()));
            }
            files.push(child_path);
        }
        archives.push((archive_dir, files));
    }

    // Pass 2: remove only verified credential files and now-empty archive folders.
    for (archive_dir, files) in &archives {
        for f in files {
            remove_file_with_retry(f).map_err(|e| fail(e.to_string()))?;
        }
        remove_empty_dir_with_retry(archive_dir).map_err(|e| fail(e.to_string()))?;
    }
    let _ = fs::remove_dir(&dir);
    Ok(u32::try_from(archives.len()).unwrap_or(u32::MAX))
}

/// Counts archived vault folders created by previous "Start fresh" resets under `data/retired-vaults/`.
#[tauri::command]
pub async fn retired_vaults_count(
    app: AppHandle,
    state: State<'_, AppState>,
    auth: State<'_, AuthState>,
) -> Result<u32, CodedError> {
    let _ = &auth;
    let root = state.root.clone();
    run_blocking("storage", move || {
        let auth = app.state::<AuthState>();
        let _vault = lock(&auth.vault);
        count_retired_vaults(&root)
    })
    .await
}

/// Permanently deletes archived vault files under `data/retired-vaults/` (never the active vault).
#[tauri::command]
pub async fn clear_retired_vaults(
    app: AppHandle,
    state: State<'_, AppState>,
    auth: State<'_, AuthState>,
) -> Result<u32, CodedError> {
    let _ = &auth;
    let root = state.root.clone();
    run_blocking("retired_vaults_clear_failed", move || {
        let auth = app.state::<AuthState>();
        let _vault = lock(&auth.vault);
        let removed = clear_retired_vaults_in(&root)?;
        tracing::info!(removed, "retired vaults cleared");
        Ok(removed)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_runs_on_blocking_pool_and_writes_archive() {
        let dir = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(dir.path());
        root.ensure_layout().unwrap();
        let settings = LauncherSettings::default();
        let (exported, path) = run_blocking("diagnostics_failed", move || {
            build_export(&root, &settings, "Bundled", "154.0", 1_700_000_000)
        })
        .await
        .unwrap();
        assert_eq!(exported.name, "Clade-Diagnostics-20231114-221320.zip");
        assert!(path.is_file());
    }

    #[tokio::test]
    async fn blocking_task_failures_map_to_stable_coded_errors() {
        let fail = || Err(CodedError::storage("read-only"));
        let err = run_blocking::<(), _>("open_failed", fail)
            .await
            .unwrap_err();
        assert_eq!(err.code, "storage");

        let join_err = run_blocking::<(), _>("diagnostics_failed", || {
            panic!("simulated worker panic");
        })
        .await
        .unwrap_err();
        assert_eq!(join_err.code, "diagnostics_failed");
    }

    #[test]
    fn retired_vaults_count_and_clear_delete_only_valid_archives_and_preserve_active_vault() {
        let dir = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(dir.path());
        root.ensure_layout().unwrap();

        assert_eq!(count_retired_vaults(&root).unwrap(), 0);
        assert_eq!(clear_retired_vaults_in(&root).unwrap(), 0);

        let active_vault = root.data_dir().join("vault.bin");
        let active_bak = root.data_dir().join("vault.bin.bak");
        let active_accounts = root.data_dir().join("accounts.json");
        fs::write(&active_vault, b"active-vault-bytes").unwrap();
        fs::write(&active_bak, b"active-bak-bytes").unwrap();
        fs::write(&active_accounts, b"{\"schema\":1,\"accounts\":[]}").unwrap();

        let retired = root.data_dir().join(RETIRED_DIR);
        let a1 = retired.join("1700000000-0123456789ab");
        let a2 = retired.join("1700000005-abcdef012345");
        fs::create_dir_all(&a1).unwrap();
        fs::create_dir_all(&a2).unwrap();
        fs::write(a1.join("vault.bin"), b"old-1").unwrap();
        fs::write(a1.join("vault.bin.bak"), b"old-1-bak").unwrap();
        fs::write(a2.join("vault.bin"), b"old-2").unwrap();
        fs::write(a2.join("vault.bin.001122334455.tmp"), b"old-2-tmp").unwrap();

        assert_eq!(count_retired_vaults(&root).unwrap(), 2);
        assert_eq!(clear_retired_vaults_in(&root).unwrap(), 2);
        assert!(!retired.exists());
        assert_eq!(count_retired_vaults(&root).unwrap(), 0);
        assert_eq!(fs::read(&active_vault).unwrap(), b"active-vault-bytes");
        assert_eq!(fs::read(&active_bak).unwrap(), b"active-bak-bytes");
        assert_eq!(
            fs::read(&active_accounts).unwrap(),
            b"{\"schema\":1,\"accounts\":[]}"
        );
    }

    #[test]
    fn clear_retired_vaults_ignores_bystanders_and_refuses_unexpected_files_or_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(dir.path());
        root.ensure_layout().unwrap();

        let retired = root.data_dir().join(RETIRED_DIR);
        fs::create_dir_all(&retired).unwrap();
        let bystander = retired.join("notes.txt");
        fs::write(&bystander, b"keep-me").unwrap();
        assert_eq!(count_retired_vaults(&root).unwrap(), 0);
        assert_eq!(clear_retired_vaults_in(&root).unwrap(), 0);
        assert_eq!(fs::read(&bystander).unwrap(), b"keep-me");

        // An unexpected file inside a retired folder causes abort before deleting anything.
        let a1 = retired.join("1700000000-0123456789ab");
        fs::create_dir_all(&a1).unwrap();
        let archived_vault = a1.join("vault.bin");
        let unexpected = a1.join("keep.txt");
        fs::write(&archived_vault, b"old-1").unwrap();
        fs::write(&unexpected, b"unexpected").unwrap();
        let err = clear_retired_vaults_in(&root).unwrap_err();
        assert_eq!(err.code, "retired_vaults_clear_failed");
        assert!(archived_vault.is_file());
        fs::remove_file(&unexpected).unwrap();

        // A junction masquerading as an archive folder is skipped by count and refused by clear.
        let outside = dir.path().join("outside-vault");
        fs::create_dir_all(&outside).unwrap();
        let outside_vault = outside.join("vault.bin");
        fs::write(&outside_vault, b"outside").unwrap();
        let linked = retired.join("1700000001-fedcba987654");
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&linked)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(out.status.success());

        assert_eq!(count_retired_vaults(&root).unwrap(), 1);
        let err = clear_retired_vaults_in(&root).unwrap_err();
        assert_eq!(err.code, "retired_vaults_clear_failed");
        assert!(archived_vault.is_file());
        assert!(outside_vault.is_file());
        fs::remove_dir(&linked).unwrap();
    }
}
