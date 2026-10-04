//! The bridge between the frontend and the launcher's portable settings (`data/settings.json`).
//!
//! Launcher-owned UI state (sound switch, UI volume, the last headline shown) is authoritative
//! here, in the existing Rust settings service. The WebView's own storage (`data/webview2`) is
//! only disposable browser-engine cache and holds none of it; deleting that folder loses nothing.
//!
//! The frontend reads the values once at start (`get_ui_settings`), writes changes through
//! `set_sound_settings` / `set_last_phrase`, and hears about changes on the
//! `ui-settings-changed` event (so a future second window or settings screen stays in sync).

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::coded_error::CodedError;
use crate::instances::InstanceId;
use crate::settings::LauncherSettings;

pub const CHANGED_EVENT: &str = "ui-settings-changed";

/// The settings plus where they are saved. One instance, shared by the whole app.
pub struct SettingsState {
    path: PathBuf,
    inner: Mutex<LauncherSettings>,
}

impl SettingsState {
    pub fn new(path: PathBuf, settings: LauncherSettings) -> Self {
        Self {
            path,
            inner: Mutex::new(settings),
        }
    }
}

/// What the frontend sees. camelCase on the wire.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSettingsDto {
    pub sound_enabled: bool,
    pub sound_volume: f32,
    pub last_phrase: Option<String>,
    /// The language the person chose, if they ever did (`en` / `ru`).
    pub language: Option<String>,
    /// The Windows display language (a locale name such as `ru-RU`), for the first-run default.
    pub system_locale: Option<String>,
    /// `system`, `on` or `off`.
    pub reduced_motion: String,
    /// False until the first-run introduction was finished or skipped.
    pub onboarding_done: bool,
    /// The global game memory in MiB, or `None` for the automatic policy.
    pub memory_mb: Option<u32>,
    /// What this machine allows, and what "Automatic" means here, so the UI never guesses.
    pub memory: MemoryInfo,
    /// `keep_open` or `hide`.
    pub on_launch: String,
    /// `show` or `stay_hidden`.
    pub on_exit: String,
    /// The application version (from Cargo.toml, the single source of truth).
    pub version: String,
}

/// Memory facts for the Settings screen (MiB).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    pub min: u32,
    pub max: u32,
    pub auto: u32,
    pub total: Option<u64>,
}

impl UiSettingsDto {
    pub fn from_settings(s: &LauncherSettings) -> Self {
        let total = crate::system::total_memory_mb();
        let (min, max) = crate::system::memory_limits_mb(total);
        Self {
            sound_enabled: s.ui.sound.enabled,
            sound_volume: s.ui.sound.volume,
            last_phrase: s.ui.last_phrase.clone(),
            language: s.ui.language.clone(),
            system_locale: crate::system::system_locale(),
            reduced_motion: s.ui.reduced_motion.clone(),
            onboarding_done: s.ui.onboarding_done,
            memory_mb: s.minecraft.memory_mb,
            memory: MemoryInfo {
                min,
                max,
                auto: crate::system::auto_memory_mb(total),
                total,
            },
            on_launch: s.behavior.on_launch.clone(),
            on_exit: s.behavior.on_exit.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

impl SettingsState {
    /// How many files may download at once (1 to 16).
    pub fn max_concurrent_downloads(&self) -> usize {
        usize::from(
            self.inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .max_concurrent_downloads,
        )
        .clamp(1, 16)
    }

    /// The instance selected in the dock, if any (an id, never a path).
    pub fn last_instance(&self) -> Option<InstanceId> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_instance
            .clone()
    }

    /// Remembers the selected instance in `data/settings.json`.
    pub fn set_last_instance(&self, id: Option<InstanceId>) -> Result<(), String> {
        self.update(|s| {
            let changed = s.last_instance != id;
            s.last_instance = id;
            changed
        })
        .map(|_| ())
    }

    /// The global game memory (MiB), if the person chose one; `None` means automatic.
    pub fn default_memory_mb(&self) -> Option<u32> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .minecraft
            .memory_mb
    }

    /// What the window does around a game.
    pub fn behavior(&self) -> crate::settings::BehaviorPrefs {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .behavior
            .clone()
    }

    /// A copy of the settings (for the diagnostics export).
    pub fn settings_copy(&self) -> LauncherSettings {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn snapshot(&self) -> UiSettingsDto {
        UiSettingsDto::from_settings(&self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Applies `change` under the lock and saves atomically if it modified anything. On a save
    /// failure the in-memory value is rolled back, so memory never disagrees with the file.
    fn update(
        &self,
        change: impl FnOnce(&mut LauncherSettings) -> bool,
    ) -> Result<(UiSettingsDto, bool), String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let previous = guard.clone();
        let changed = change(&mut guard);
        if changed {
            if let Err(e) = guard.save(&self.path) {
                *guard = previous;
                return Err(format!("could not save settings: {e}"));
            }
        }
        Ok((UiSettingsDto::from_settings(&guard), changed))
    }
}

#[tauri::command]
pub fn get_ui_settings(state: State<'_, SettingsState>) -> UiSettingsDto {
    state.snapshot()
}

#[tauri::command]
pub fn set_sound_settings(
    app: AppHandle,
    state: State<'_, SettingsState>,
    enabled: Option<bool>,
    volume: Option<f32>,
) -> Result<UiSettingsDto, String> {
    let (dto, changed) = state.update(|s| s.set_sound(enabled, volume))?;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto.clone());
    }
    Ok(dto)
}

#[tauri::command]
pub fn set_last_phrase(
    app: AppHandle,
    state: State<'_, SettingsState>,
    id: String,
) -> Result<(), String> {
    let (dto, changed) = state.update(|s| {
        let before = s.ui.last_phrase.clone();
        s.set_last_phrase(&id) && s.ui.last_phrase != before
    })?;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto);
    }
    Ok(())
}

#[tauri::command]
pub fn set_language(
    app: AppHandle,
    state: State<'_, SettingsState>,
    language: String,
) -> Result<(), CodedError> {
    if !crate::settings::is_supported_language(&language) {
        return Err(CodedError::new(
            "unsupported_language",
            "unknown language code",
        ));
    }
    let (dto, changed) = state
        .update(|s| {
            let before = s.ui.language.clone();
            s.set_language(&language) && s.ui.language != before
        })
        .map_err(CodedError::storage)?;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto);
    }
    Ok(())
}

/// Saves, announces and reports the outcome of one settings change.
fn apply(
    app: &AppHandle,
    state: &SettingsState,
    change: impl FnOnce(&mut LauncherSettings) -> bool,
) -> Result<(), CodedError> {
    let (dto, changed) = state.update(change).map_err(CodedError::storage)?;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto);
    }
    Ok(())
}

fn unsupported() -> CodedError {
    CodedError::new("unsupported_value", "that setting value is not supported")
}

#[tauri::command]
pub fn set_reduced_motion(
    app: AppHandle,
    state: State<'_, SettingsState>,
    mode: String,
) -> Result<(), CodedError> {
    if !crate::settings::MOTION_MODES.contains(&mode.as_str()) {
        return Err(unsupported());
    }
    apply(&app, &state, |s| {
        let before = s.ui.reduced_motion.clone();
        s.set_reduced_motion(&mode) && s.ui.reduced_motion != before
    })
}

/// Sets the global game memory in MiB, or `null` for automatic. A value outside the safe range for
/// this machine is refused with that range.
#[tauri::command]
pub fn set_memory(
    app: AppHandle,
    state: State<'_, SettingsState>,
    mb: Option<u32>,
) -> Result<(), CodedError> {
    let total = crate::system::total_memory_mb();
    let mut refused = None;
    let result = apply(&app, &state, |s| match s.set_memory(mb, total) {
        Ok(changed) => changed,
        Err(range) => {
            refused = Some(range);
            false
        }
    });
    if let Some((min, max)) = refused {
        return Err(
            CodedError::new("invalid_memory", "memory is outside the safe range").with(
                crate::coded_error::params([("min", min.into()), ("max", max.into())]),
            ),
        );
    }
    result
}

#[tauri::command]
pub fn set_behavior(
    app: AppHandle,
    state: State<'_, SettingsState>,
    on_launch: Option<String>,
    on_exit: Option<String>,
) -> Result<(), CodedError> {
    let mut valid = true;
    let result = apply(&app, &state, |s| {
        let before = s.behavior.clone();
        valid = s.set_behavior(on_launch.as_deref(), on_exit.as_deref());
        s.behavior != before
    });
    if !valid {
        return Err(unsupported());
    }
    result
}

#[tauri::command]
pub fn complete_onboarding(
    app: AppHandle,
    state: State<'_, SettingsState>,
) -> Result<(), CodedError> {
    apply(&app, &state, |s| s.complete_onboarding())
}

/// Puts the user-configurable preferences back to their defaults. Profiles, accounts, the vault,
/// game files and playtime are not stored in the settings file and are never touched.
#[tauri::command]
pub fn reset_settings(app: AppHandle, state: State<'_, SettingsState>) -> Result<(), CodedError> {
    apply(&app, &state, |s| s.reset_user_prefs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn state(dir: &std::path::Path) -> SettingsState {
        SettingsState::new(
            dir.join("data").join("settings.json"),
            LauncherSettings::default(),
        )
    }

    #[test]
    fn defaults_are_exposed_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let dto = state(d.path()).snapshot();
        assert_eq!(
            dto,
            UiSettingsDto {
                sound_enabled: true,
                sound_volume: 0.5,
                last_phrase: None,
                language: None,
                system_locale: dto.system_locale.clone(),
                reduced_motion: "system".into(),
                onboarding_done: false,
                memory_mb: None,
                memory: dto.memory.clone(),
                on_launch: "keep_open".into(),
                on_exit: "show".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            }
        );
        assert!(dto.memory.min <= dto.memory.auto.max(dto.memory.min));
        assert!(dto.memory.min < dto.memory.max);
    }

    #[test]
    fn changes_are_saved_to_the_portable_file_and_reported() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        let (dto, changed) = st.update(|s| s.set_sound(Some(false), Some(0.2))).unwrap();
        assert!(changed && !dto.sound_enabled && (dto.sound_volume - 0.2).abs() < f32::EPSILON);
        let (_, changed) = st.update(|s| s.set_sound(Some(false), Some(0.2))).unwrap();
        assert!(!changed, "no-op updates neither write nor emit");
        let (dto, _) = st.update(|s| s.set_last_phrase("one-more-world")).unwrap();
        assert_eq!(dto.last_phrase.as_deref(), Some("one-more-world"));

        // A fresh load from disk (a new launch, possibly on another PC) sees the same values.
        let reloaded =
            LauncherSettings::load_or_default(&d.path().join("data").join("settings.json"))
                .unwrap();
        assert_eq!(UiSettingsDto::from_settings(&reloaded), st.snapshot());
        assert!(!reloaded.ui.sound.enabled);
    }

    #[test]
    fn an_explicit_language_is_saved_and_survives_a_reload() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        let (dto, changed) = st.update(|s| s.set_language("ru")).unwrap();
        assert!(changed && dto.language.as_deref() == Some("ru"));
        let (_, changed) = st
            .update(|s| {
                let before = s.ui.language.clone();
                s.set_language("ru") && s.ui.language != before
            })
            .unwrap();
        assert!(!changed, "choosing the same language again is not a write");
        let (dto, _) = st.update(|s| s.set_language("de")).unwrap();
        assert_eq!(
            dto.language.as_deref(),
            Some("ru"),
            "unknown codes change nothing"
        );
        let reloaded =
            LauncherSettings::load_or_default(&d.path().join("data").join("settings.json"))
                .unwrap();
        assert_eq!(reloaded.ui.language.as_deref(), Some("ru"));
    }

    #[test]
    fn invalid_input_never_corrupts_the_state() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        st.update(|s| s.set_last_phrase("welcome-back")).unwrap();
        let (dto, changed) = st.update(|s| s.set_last_phrase("../../evil")).unwrap();
        assert!(!changed);
        assert_eq!(dto.last_phrase.as_deref(), Some("welcome-back"));
        let (dto, _) = st.update(|s| s.set_sound(None, Some(99.0))).unwrap();
        assert!((dto.sound_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_failed_save_rolls_the_in_memory_value_back() {
        let d = tempfile::tempdir().unwrap();
        // Make the target path unwritable: its parent "directory" is a file.
        let blocker = d.path().join("data");
        fs::write(&blocker, b"not a directory").unwrap();
        let st = SettingsState::new(blocker.join("settings.json"), LauncherSettings::default());
        assert!(st.update(|s| s.set_sound(Some(false), None)).is_err());
        assert!(
            st.snapshot().sound_enabled,
            "memory must still match what is on disk"
        );
    }

    #[test]
    fn the_wire_format_is_camel_case() {
        let json = serde_json::to_value(UiSettingsDto {
            sound_enabled: true,
            sound_volume: 0.5,
            last_phrase: Some("welcome-back".into()),
            language: Some("ru".into()),
            system_locale: Some("ru-RU".into()),
            reduced_motion: "on".into(),
            onboarding_done: true,
            memory_mb: Some(4096),
            memory: MemoryInfo {
                min: 1024,
                max: 12288,
                auto: 4096,
                total: Some(16384),
            },
            on_launch: "hide".into(),
            on_exit: "stay_hidden".into(),
            version: "0.1.0".into(),
        })
        .unwrap();
        assert_eq!(json["reducedMotion"], "on");
        assert_eq!(json["onboardingDone"], true);
        assert_eq!(json["memoryMb"], 4096);
        assert_eq!(json["memory"]["max"], 12288);
        assert_eq!(json["onLaunch"], "hide");
        assert_eq!(json["onExit"], "stay_hidden");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["language"], "ru");
        assert_eq!(json["systemLocale"], "ru-RU");
        assert_eq!(json["soundEnabled"], true);
        assert_eq!(json["soundVolume"], 0.5);
        assert_eq!(json["lastPhrase"], "welcome-back");
    }
}
