//! The Support actions in Settings: open the Clade folder, open the logs folder, export diagnostics.
//!
//! Every action is fixed: the folders are the launcher's own (never a path from the frontend) and
//! the diagnostics ZIP is written to `logs/diagnostics/` from an allowlist (see `diagnostics.rs`).
//! Failures are stable codes the frontend words in the selected language.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::coded_error::CodedError;
use crate::diagnostics::{self, Exported, Facts};
use crate::ui_settings::SettingsState;
use crate::AppState;

fn open_folder(app: &AppHandle, dir: &Path) -> Result<(), CodedError> {
    fs::create_dir_all(dir).map_err(CodedError::storage)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| CodedError::new("open_failed", e.to_string()))
}

/// Opens the folder that contains Clade.exe and everything Clade manages.
#[tauri::command]
pub fn open_clade_folder(app: AppHandle, state: State<'_, AppState>) -> Result<(), CodedError> {
    open_folder(&app, state.root.path())
}

/// Opens `logs/`.
#[tauri::command]
pub fn open_logs_folder(app: AppHandle, state: State<'_, AppState>) -> Result<(), CodedError> {
    open_folder(&app, &state.root.logs_dir())
}

/// Creates the diagnostics ZIP and shows it in its folder. Returns only its file name.
#[tauri::command]
pub fn export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: State<'_, SettingsState>,
) -> Result<Exported, CodedError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let dto = settings.snapshot();
    let (exported, path) = diagnostics::export(
        &state.root,
        &settings.settings_copy(),
        &Facts {
            version: env!("CARGO_PKG_VERSION"),
            layout_version: crate::portable::root::LAYOUT_VERSION,
            webview: &format!("{:?}", state.webview),
            webview_version: &state.webview_version,
            system_locale: dto.system_locale.as_deref(),
            total_memory_mb: dto.memory.total,
            free_disk_bytes: crate::system::free_disk_bytes(state.root.path()),
            now_unix: now,
        },
    )?;
    // Showing the file is a courtesy: the export itself already succeeded.
    if let Err(e) = app.opener().reveal_item_in_dir(&path) {
        tracing::warn!(error = %e, "could not show the diagnostics file");
    }
    tracing::info!(name = %exported.name, "diagnostics exported");
    Ok(exported)
}
