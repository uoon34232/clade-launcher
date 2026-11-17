//! The Support actions in Settings: open the Clade folder, open the logs folder, export diagnostics.
//!
//! Every action is fixed: the folders are the launcher's own (never a path from the frontend) and
//! the diagnostics ZIP is written to `logs/diagnostics/` from an allowlist (see `diagnostics.rs`).
//! Failures are stable codes the frontend words in the selected language.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::coded_error::CodedError;
use crate::diagnostics::{self, Exported, Facts};
use crate::portable::PortableRoot;
use crate::settings::LauncherSettings;
use crate::ui_settings::{SettingsState, UiSettingsDto};
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
}
