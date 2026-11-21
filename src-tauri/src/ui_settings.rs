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
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::coded_error::CodedError;
use crate::instances::InstanceId;
use crate::settings::LauncherSettings;

pub const CHANGED_EVENT: &str = "ui-settings-changed";

struct Inner {
    path: PathBuf,
    /// Held only in memory to clone or swap; never held across disk `save()` / `sync_all()`.
    current: RwLock<LauncherSettings>,
    /// Serialises writers against other writers while leaving `current` readable during disk I/O.
    writer: Mutex<()>,
}

/// The settings plus where they are saved. Cheaply cloneable and shared by the whole app.
#[derive(Clone)]
pub struct SettingsState {
    inner: Arc<Inner>,
}

impl SettingsState {
    pub fn new(path: PathBuf, settings: LauncherSettings) -> Self {
        Self {
            inner: Arc::new(Inner {
                path,
                current: RwLock::new(settings),
                writer: Mutex::new(()),
            }),
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
    /// `simple` or `advanced`.
    pub ui_mode: String,
    /// `system`, `on` or `off`.
    pub reduced_motion: String,
    /// `system`, `on` or `off`: the window-material ("Transparency effects") choice.
    pub transparency: String,
    /// False until the first-run introduction was finished or skipped.
    pub onboarding_done: bool,
    /// The global game memory in MiB, or `None` for the automatic policy.
    pub memory_mb: Option<u32>,
    /// What this machine allows, and what "Automatic" means here, so the UI never guesses.
    pub memory: MemoryInfo,
    /// `embed`, `keep_open` or `hide`.
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
            ui_mode: s.ui.ui_mode.clone(),
            reduced_motion: s.ui.reduced_motion.clone(),
            transparency: s.ui.transparency.clone(),
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
            version: crate::update_check::installed_version_tag()
                .trim_start_matches('v')
                .to_owned(),
        }
    }
}

impl SettingsState {
    fn read(&self) -> RwLockReadGuard<'_, LauncherSettings> {
        self.inner.current.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, LauncherSettings> {
        self.inner
            .current
            .write()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// How many files may download at once (1 to 16).
    pub fn max_concurrent_downloads(&self) -> usize {
        usize::from(self.read().max_concurrent_downloads).clamp(1, 16)
    }

    /// The transparency-effects choice (`system`, `on` or `off`).
    pub fn transparency(&self) -> String {
        self.read().ui.transparency.clone()
    }

    /// The reduced-motion choice (`system`, `on` or `off`).
    pub fn reduced_motion(&self) -> String {
        self.read().ui.reduced_motion.clone()
    }

    /// The instance selected in the dock, if any (an id, never a path).
    pub fn last_instance(&self) -> Option<InstanceId> {
        self.read().last_instance.clone()
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
        self.read().minecraft.memory_mb
    }

    /// What the window does around a game.
    pub fn behavior(&self) -> crate::settings::BehaviorPrefs {
        self.read().behavior.clone()
    }

    /// A copy of the settings (for the diagnostics export).
    pub fn settings_copy(&self) -> LauncherSettings {
        self.read().clone()
    }

    pub(crate) fn snapshot(&self) -> UiSettingsDto {
        let copy = self.settings_copy();
        UiSettingsDto::from_settings(&copy)
    }

    /// Serialises writers, clones `current`, saves atomically while `current` stays unlocked so
    /// readers never stall on USB `sync_all()`, and commits `next` into `current` only after `save`
    /// succeeds. On a save failure `current` is never touched.
    fn update(
        &self,
        change: impl FnOnce(&mut LauncherSettings) -> bool,
    ) -> Result<(UiSettingsDto, bool), String> {
        let _writer = self.inner.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = self.settings_copy();
        let changed = change(&mut next);
        if changed {
            next.save(&self.inner.path)
                .map_err(|e| format!("could not save settings: {e}"))?;
            *self.write() = next.clone();
        }
        Ok((UiSettingsDto::from_settings(&next), changed))
    }
}

#[tauri::command]
pub fn get_ui_settings(state: State<'_, SettingsState>) -> UiSettingsDto {
    state.snapshot()
}

#[tauri::command]
pub async fn set_sound_settings(
    app: AppHandle,
    state: State<'_, SettingsState>,
    enabled: Option<bool>,
    volume: Option<f32>,
) -> Result<UiSettingsDto, String> {
    let st = state.inner().clone();
    let (dto, changed) =
        tokio::task::spawn_blocking(move || st.update(|s| s.set_sound(enabled, volume)))
            .await
            .map_err(|e| format!("settings task failed: {e}"))??;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto.clone());
    }
    Ok(dto)
}

#[tauri::command]
pub async fn set_last_phrase(
    app: AppHandle,
    state: State<'_, SettingsState>,
    id: String,
) -> Result<(), String> {
    let st = state.inner().clone();
    let (dto, changed) = tokio::task::spawn_blocking(move || {
        st.update(|s| {
            let before = s.ui.last_phrase.clone();
            s.set_last_phrase(&id) && s.ui.last_phrase != before
        })
    })
    .await
    .map_err(|e| format!("settings task failed: {e}"))??;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto);
    }
    Ok(())
}

/// Saves on a blocking worker, announces on change, and refreshes the window backdrop only when
/// `refresh_glass` is true (`set_transparency` and `reset_settings`).
async fn apply_blocking(
    app: &AppHandle,
    state: &SettingsState,
    refresh_glass: bool,
    change: impl FnOnce(&mut LauncherSettings) -> bool + Send + 'static,
) -> Result<(), CodedError> {
    let st = state.clone();
    let app_for_glass = app.clone();
    let (dto, changed) = tokio::task::spawn_blocking(move || {
        let out = st.update(change)?;
        if out.1 && refresh_glass {
            crate::glass::refresh(&app_for_glass);
        }
        Ok::<_, String>(out)
    })
    .await
    .map_err(|e| CodedError::storage(format!("settings task failed: {e}")))?
    .map_err(CodedError::storage)?;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto);
    }
    Ok(())
}

#[tauri::command]
pub async fn set_language(
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
    apply_blocking(&app, &state, false, move |s| {
        let before = s.ui.language.clone();
        s.set_language(&language) && s.ui.language != before
    })
    .await
}

fn unsupported() -> CodedError {
    CodedError::new("unsupported_value", "that setting value is not supported")
}

#[tauri::command]
pub async fn set_ui_mode(
    app: AppHandle,
    state: State<'_, SettingsState>,
    mode: String,
) -> Result<(), CodedError> {
    if !crate::settings::UI_MODES.contains(&mode.as_str()) {
        return Err(unsupported());
    }
    apply_blocking(&app, &state, false, move |s| {
        let before = s.ui.ui_mode.clone();
        s.set_ui_mode(&mode) && s.ui.ui_mode != before
    })
    .await
}

#[tauri::command]
pub async fn set_reduced_motion(
    app: AppHandle,
    state: State<'_, SettingsState>,
    mode: String,
) -> Result<(), CodedError> {
    if !crate::settings::MOTION_MODES.contains(&mode.as_str()) {
        return Err(unsupported());
    }
    apply_blocking(&app, &state, false, move |s| {
        let before = s.ui.reduced_motion.clone();
        s.set_reduced_motion(&mode) && s.ui.reduced_motion != before
    })
    .await
}

#[tauri::command]
pub async fn set_transparency(
    app: AppHandle,
    state: State<'_, SettingsState>,
    mode: String,
) -> Result<(), CodedError> {
    if !crate::settings::TRANSPARENCY_MODES.contains(&mode.as_str()) {
        return Err(unsupported());
    }
    apply_blocking(&app, &state, true, move |s| {
        let before = s.ui.transparency.clone();
        s.set_transparency(&mode) && s.ui.transparency != before
    })
    .await
}

/// Sets the global game memory in MiB, or `null` for automatic. A value outside the safe range for
/// this machine is refused with that range.
#[tauri::command]
pub async fn set_memory(
    app: AppHandle,
    state: State<'_, SettingsState>,
    mb: Option<u32>,
) -> Result<(), CodedError> {
    let st = state.inner().clone();
    let (res, refused) = tokio::task::spawn_blocking(move || {
        let total = crate::system::total_memory_mb();
        let mut refused = None;
        let res = st.update(|s| match s.set_memory(mb, total) {
            Ok(changed) => changed,
            Err(range) => {
                refused = Some(range);
                false
            }
        });
        (res, refused)
    })
    .await
    .map_err(|e| CodedError::storage(format!("settings task failed: {e}")))?;
    if let Some((min, max)) = refused {
        return Err(
            CodedError::new("invalid_memory", "memory is outside the safe range").with(
                crate::coded_error::params([("min", min.into()), ("max", max.into())]),
            ),
        );
    }
    let (dto, changed) = res.map_err(CodedError::storage)?;
    if changed {
        let _ = app.emit(CHANGED_EVENT, dto);
    }
    Ok(())
}

#[tauri::command]
pub async fn set_behavior(
    app: AppHandle,
    state: State<'_, SettingsState>,
    on_launch: Option<String>,
    on_exit: Option<String>,
) -> Result<(), CodedError> {
    if on_launch
        .as_deref()
        .is_some_and(|v| !crate::settings::ON_LAUNCH_MODES.contains(&v))
        || on_exit
            .as_deref()
            .is_some_and(|v| !crate::settings::ON_EXIT_MODES.contains(&v))
    {
        return Err(unsupported());
    }
    apply_blocking(&app, &state, false, move |s| {
        let before = s.behavior.clone();
        s.set_behavior(on_launch.as_deref(), on_exit.as_deref()) && s.behavior != before
    })
    .await
}

#[tauri::command]
pub async fn complete_onboarding(
    app: AppHandle,
    state: State<'_, SettingsState>,
) -> Result<(), CodedError> {
    apply_blocking(&app, &state, false, LauncherSettings::complete_onboarding).await
}

/// Puts the user-configurable preferences back to their defaults. Profiles, accounts, the vault,
/// game files and playtime are not stored in the settings file and are never touched.
#[tauri::command]
pub async fn reset_settings(
    app: AppHandle,
    state: State<'_, SettingsState>,
) -> Result<(), CodedError> {
    apply_blocking(&app, &state, true, LauncherSettings::reset_user_prefs).await
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
                ui_mode: "simple".into(),
                reduced_motion: "system".into(),
                transparency: "system".into(),
                onboarding_done: false,
                memory_mb: None,
                memory: dto.memory.clone(),
                on_launch: "embed".into(),
                on_exit: "show".into(),
                version: crate::update_check::installed_version_tag()
                    .trim_start_matches('v')
                    .into(),
            }
        );
        assert!(dto.memory.min <= dto.memory.auto.max(dto.memory.min));
        assert!(dto.memory.min <= dto.memory.max);
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
    fn a_failed_save_leaves_in_memory_state_completely_untouched() {
        let d = tempfile::tempdir().unwrap();
        // Make the target path unwritable: its parent "directory" is a file.
        let blocker = d.path().join("data");
        fs::write(&blocker, b"not a directory").unwrap();
        let initial = LauncherSettings {
            max_concurrent_downloads: 6,
            ..LauncherSettings::default()
        };
        let st = SettingsState::new(blocker.join("settings.json"), initial.clone());
        let reader = st.clone();
        assert!(st.update(|s| s.set_sound(Some(false), Some(0.1))).is_err());
        assert!(st.update(|s| s.set_language("ru")).is_err());
        assert!(st.update(|s| s.set_transparency("on")).is_err());
        assert!(st
            .set_last_instance(Some(InstanceId::new("survival").unwrap()))
            .is_err());
        assert_eq!(
            reader.settings_copy(),
            initial,
            "failed saves must never mutate in-memory SettingsState"
        );
        assert!(reader.snapshot().sound_enabled);
        assert_eq!(reader.transparency(), "system");
        assert_eq!(reader.last_instance(), None);
    }

    #[test]
    fn the_wire_format_is_camel_case() {
        let json = serde_json::to_value(UiSettingsDto {
            sound_enabled: true,
            sound_volume: 0.5,
            last_phrase: Some("welcome-back".into()),
            language: Some("ru".into()),
            system_locale: Some("ru-RU".into()),
            ui_mode: "advanced".into(),
            reduced_motion: "on".into(),
            transparency: "off".into(),
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
        assert_eq!(json["uiMode"], "advanced");
        assert_eq!(json["reducedMotion"], "on");
        assert_eq!(json["transparency"], "off");
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
