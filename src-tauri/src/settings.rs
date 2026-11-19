//! Launcher settings (`data/settings.json`).
//!
//! Every path stored here is a `RelPath`: relative to the launcher root, validated on load.
//! A settings file copied between PCs, or edited by hand, cannot point outside the folder.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::instances::InstanceId;
use crate::portable::fsutil::{atomic_write, quarantine_corrupt};
use crate::portable::root::LAYOUT_VERSION;

/// UI sound preferences ("Sound effects" switch and volume).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundPrefs {
    pub enabled: bool,
    /// 0.0 to 1.0.
    pub volume: f32,
}

impl Default for SoundPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 0.5,
        }
    }
}

/// How the interface treats decorative motion: follow Windows, always reduce, or never reduce.
pub const MOTION_MODES: [&str; 3] = ["system", "on", "off"];
/// How the window material (the translucent "glass" treatment) is chosen: follow the Windows
/// "Transparency effects" preference, always try it, or never use it.
pub const TRANSPARENCY_MODES: [&str; 3] = ["system", "on", "off"];
/// Whether the launcher shows only essential everyday controls (`simple`) or full power-user
/// options (`advanced`).
pub const UI_MODES: [&str; 2] = ["simple", "advanced"];
/// What Clade does with its window when Minecraft starts.
pub const ON_LAUNCH_MODES: [&str; 2] = ["keep_open", "hide"];
/// What Clade does with its window when Minecraft closes.
pub const ON_EXIT_MODES: [&str; 2] = ["show", "stay_hidden"];

/// Launcher-owned UI state. This is authoritative; the WebView profile in `data/webview2` is
/// disposable browser-engine cache and may be deleted at any time without losing any of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub sound: SoundPrefs,
    /// Id of the home-screen headline shown last, so the next start shows a different one.
    pub last_phrase: Option<String>,
    /// The language the person chose (`en` or `ru`). `None` means "never chosen": the UI then
    /// follows the Windows language, and an explicit choice is never overridden afterwards.
    pub language: Option<String>,
    /// `simple` (minimal everyday controls) or `advanced` (full controls).
    pub ui_mode: String,
    /// `system` (follow Windows), `on` or `off`.
    pub reduced_motion: String,
    /// `system` (follow the Windows "Transparency effects" setting), `on` or `off`.
    pub transparency: String,
    /// True once the first-run introduction was finished or skipped. Kept in the portable folder,
    /// so moving it to another PC does not show the introduction again.
    pub onboarding_done: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            sound: SoundPrefs::default(),
            last_phrase: None,
            language: None,
            ui_mode: "simple".to_owned(),
            reduced_motion: "system".to_owned(),
            transparency: "system".to_owned(),
            onboarding_done: false,
        }
    }
}

/// Game defaults that apply to every profile (a profile's own memory setting still wins).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MinecraftPrefs {
    /// Memory for the game in MiB, or `None` for the automatic policy.
    pub memory_mb: Option<u32>,
}

impl MinecraftPrefs {
    /// A damaged or hand-edited value is repaired to a safe portable range (`1024..=32_768` MiB
    /// in 256 MiB steps) without consulting the current machine's RAM, so moving a portable folder
    /// onto a weaker PC never overwrites its saved setting (launch-time clamping caps it per PC).
    pub fn sanitize(&mut self) {
        if let Some(mb) = self.memory_mb {
            self.memory_mb = Some(mb.clamp(1024, 32_768) / 256 * 256);
        }
    }
}

/// What Clade's window does around a game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorPrefs {
    /// `keep_open` or `hide`.
    pub on_launch: String,
    /// `show` or `stay_hidden`.
    pub on_exit: String,
}

impl Default for BehaviorPrefs {
    fn default() -> Self {
        Self {
            on_launch: "hide".to_owned(),
            on_exit: "show".to_owned(),
        }
    }
}

impl BehaviorPrefs {
    pub fn sanitize(&mut self) {
        if !ON_LAUNCH_MODES.contains(&self.on_launch.as_str()) {
            self.on_launch = Self::default().on_launch;
        }
        if !ON_EXIT_MODES.contains(&self.on_exit.as_str()) {
            self.on_exit = Self::default().on_exit;
        }
    }

    pub fn hides_on_launch(&self) -> bool {
        self.on_launch == "hide"
    }

    pub fn stays_hidden_on_exit(&self) -> bool {
        self.on_exit == "stay_hidden"
    }
}

/// The two UI languages Clade ships. English is the canonical fallback.
pub const LANGUAGES: [&str; 2] = ["en", "ru"];

pub fn is_supported_language(code: &str) -> bool {
    LANGUAGES.contains(&code)
}

/// Phrase ids are short lowercase slugs; anything else in a hand-edited file is ignored.
pub fn is_valid_phrase_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl UiPrefs {
    /// Repairs out-of-range or malformed values instead of failing: a damaged preference must
    /// never stop the launcher from starting.
    pub fn sanitize(&mut self) {
        let v = self.sound.volume;
        self.sound.volume = if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            SoundPrefs::default().volume
        };
        if self
            .last_phrase
            .as_deref()
            .is_some_and(|p| !is_valid_phrase_id(p))
        {
            self.last_phrase = None;
        }
        if self
            .language
            .as_deref()
            .is_some_and(|l| !is_supported_language(l))
        {
            self.language = None;
        }
        if !UI_MODES.contains(&self.ui_mode.as_str()) {
            self.ui_mode = Self::default().ui_mode;
        }
        if !MOTION_MODES.contains(&self.reduced_motion.as_str()) {
            self.reduced_motion = Self::default().reduced_motion;
        }
        if !TRANSPARENCY_MODES.contains(&self.transparency.as_str()) {
            self.transparency = Self::default().transparency;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherSettings {
    /// Layout the folder was last written with; lets a newer build migrate an older stick
    /// and an older build refuse a newer one instead of corrupting it.
    pub layout_version: u32,
    /// Id of the instance played last (its folder is `instances/<id>`). An id, never a path.
    pub last_instance: Option<InstanceId>,
    pub max_concurrent_downloads: u8,
    pub ui: UiPrefs,
    pub minecraft: MinecraftPrefs,
    pub behavior: BehaviorPrefs,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            layout_version: LAYOUT_VERSION,
            last_instance: None,
            max_concurrent_downloads: 8,
            ui: UiPrefs::default(),
            minecraft: MinecraftPrefs::default(),
            behavior: BehaviorPrefs::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings io error: {0}")]
    Io(#[from] io::Error),
    #[error("settings file is invalid: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("this folder was written by a newer Clade Launcher (layout {found}); this build understands up to {supported}")]
    FolderTooNew { found: u32, supported: u32 },
}

impl LauncherSettings {
    /// Startup loading: like [`Self::load_or_default`], except that a file whose *contents* are
    /// damaged (empty, truncated, malformed JSON, or values that cannot be read) never stops
    /// Clade. The damaged file is moved aside (`settings.json.corrupt`, never overwriting an
    /// earlier backup) and the defaults are used. A newer folder layout and real filesystem
    /// failures (permissions, a locked or unreadable file) are still reported as errors, and so is
    /// a failure to move the damaged file aside, because carrying on would overwrite it later.
    pub fn load_or_recover(path: &Path) -> Result<Self, SettingsError> {
        match Self::load_or_default(path) {
            Err(SettingsError::Invalid(e)) => {
                // Only where the damage is is logged, never what the file said.
                let moved = quarantine_corrupt(path)?;
                tracing::warn!(
                    line = e.line(),
                    column = e.column(),
                    kind = ?e.classify(),
                    backup = %moved.file_name().unwrap_or_default().to_string_lossy(),
                    "settings.json was damaged; it was set aside and the defaults are in use"
                );
                Ok(Self::default().sanitized())
            }
            other => other,
        }
    }

    /// Reads a settings document one value at a time. A value that cannot be read as what it should
    /// be falls back to its default; every readable sibling is kept. `None` only when the document
    /// is not a JSON object. Returns the dotted names of the values that were reset (never their
    /// contents).
    fn salvage(value: &serde_json::Value) -> Option<(Self, Vec<String>)> {
        use serde::de::DeserializeOwned;
        use serde_json::{Map, Value};

        fn leaf<T: DeserializeOwned>(
            obj: &Map<String, Value>,
            key: &str,
            path: &str,
            default: T,
            bad: &mut Vec<String>,
        ) -> T {
            match obj.get(key) {
                None => default,
                Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|_| {
                    bad.push(path.to_owned());
                    default
                }),
            }
        }
        fn section(
            obj: &Map<String, Value>,
            key: &str,
            bad: &mut Vec<String>,
        ) -> Map<String, Value> {
            match obj.get(key) {
                None => Map::new(),
                Some(Value::Object(m)) => m.clone(),
                Some(_) => {
                    bad.push(key.to_owned());
                    Map::new()
                }
            }
        }

        let root = value.as_object()?;
        let d = Self::default();
        let mut bad = Vec::new();
        let ui = section(root, "ui", &mut bad);
        let sound = section(&ui, "sound", &mut bad);
        let minecraft = section(root, "minecraft", &mut bad);
        let behavior = section(root, "behavior", &mut bad);
        let settings = Self {
            layout_version: leaf(
                root,
                "layout_version",
                "layout_version",
                d.layout_version,
                &mut bad,
            ),
            last_instance: leaf(
                root,
                "last_instance",
                "last_instance",
                d.last_instance,
                &mut bad,
            ),
            max_concurrent_downloads: leaf(
                root,
                "max_concurrent_downloads",
                "max_concurrent_downloads",
                d.max_concurrent_downloads,
                &mut bad,
            ),
            ui: UiPrefs {
                sound: SoundPrefs {
                    enabled: leaf(
                        &sound,
                        "enabled",
                        "ui.sound.enabled",
                        d.ui.sound.enabled,
                        &mut bad,
                    ),
                    volume: leaf(
                        &sound,
                        "volume",
                        "ui.sound.volume",
                        d.ui.sound.volume,
                        &mut bad,
                    ),
                },
                last_phrase: leaf(
                    &ui,
                    "last_phrase",
                    "ui.last_phrase",
                    d.ui.last_phrase,
                    &mut bad,
                ),
                language: leaf(&ui, "language", "ui.language", d.ui.language, &mut bad),
                ui_mode: leaf(&ui, "ui_mode", "ui.ui_mode", d.ui.ui_mode, &mut bad),
                reduced_motion: leaf(
                    &ui,
                    "reduced_motion",
                    "ui.reduced_motion",
                    d.ui.reduced_motion,
                    &mut bad,
                ),
                transparency: leaf(
                    &ui,
                    "transparency",
                    "ui.transparency",
                    d.ui.transparency,
                    &mut bad,
                ),
                onboarding_done: leaf(
                    &ui,
                    "onboarding_done",
                    "ui.onboarding_done",
                    d.ui.onboarding_done,
                    &mut bad,
                ),
            },
            minecraft: MinecraftPrefs {
                memory_mb: leaf(
                    &minecraft,
                    "memory_mb",
                    "minecraft.memory_mb",
                    d.minecraft.memory_mb,
                    &mut bad,
                ),
            },
            behavior: BehaviorPrefs {
                on_launch: leaf(
                    &behavior,
                    "on_launch",
                    "behavior.on_launch",
                    d.behavior.on_launch,
                    &mut bad,
                ),
                on_exit: leaf(
                    &behavior,
                    "on_exit",
                    "behavior.on_exit",
                    d.behavior.on_exit,
                    &mut bad,
                ),
            },
        };
        Some((settings, bad))
    }

    fn sanitized(mut self) -> Self {
        self.max_concurrent_downloads = self.max_concurrent_downloads.clamp(1, 16);
        self.ui.sanitize();
        self.behavior.sanitize();
        self.minecraft.sanitize();
        self
    }

    pub fn load_or_default(path: &Path) -> Result<Self, SettingsError> {
        let s: Self = match fs::read(path) {
            Ok(b) => {
                // Syntax first, so a file from a newer Clade whose shape this build cannot read is
                // reported as "too new" and not mistaken for damage.
                let value: serde_json::Value = serde_json::from_slice(&b)?;
                let layout = value
                    .get("layout_version")
                    .and_then(serde_json::Value::as_u64);
                if let Some(found) = layout.filter(|l| *l > u64::from(LAYOUT_VERSION)) {
                    return Err(SettingsError::FolderTooNew {
                        found: u32::try_from(found).unwrap_or(u32::MAX),
                        supported: LAYOUT_VERSION,
                    });
                }
                match serde_json::from_value::<Self>(value.clone()) {
                    Ok(s) => s,
                    // Valid JSON in which some value has the wrong shape (a number where text
                    // belongs, a path where an id belongs): reset only those values and keep every
                    // other setting. Only a document that is not a settings object at all is an error.
                    Err(e) => match Self::salvage(&value) {
                        Some((s, unusable)) => {
                            tracing::warn!(
                                fields = ?unusable,
                                "some settings had unusable values; only those were reset to their defaults"
                            );
                            s
                        }
                        None => return Err(e.into()),
                    },
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(s.sanitized())
    }

    /// Applies a sound-preference change. Returns whether anything changed.
    pub fn set_sound(&mut self, enabled: Option<bool>, volume: Option<f32>) -> bool {
        let before = self.ui.sound.clone();
        if let Some(e) = enabled {
            self.ui.sound.enabled = e;
        }
        if let Some(v) = volume {
            self.ui.sound.volume = v;
        }
        self.ui.sanitize();
        self.ui.sound != before
    }

    /// Records the headline shown last. Returns false (and changes nothing) for an invalid id.
    pub fn set_last_phrase(&mut self, id: &str) -> bool {
        if !is_valid_phrase_id(id) {
            return false;
        }
        if self.ui.last_phrase.as_deref() == Some(id) {
            return true;
        }
        self.ui.last_phrase = Some(id.to_owned());
        true
    }

    /// Records an explicit language choice. Returns false (and changes nothing) for an unknown code.
    pub fn set_language(&mut self, code: &str) -> bool {
        if !is_supported_language(code) {
            return false;
        }
        self.ui.language = Some(code.to_owned());
        true
    }

    /// Records the interface complexity mode (`simple` or `advanced`).
    pub fn set_ui_mode(&mut self, mode: &str) -> bool {
        if !UI_MODES.contains(&mode) {
            return false;
        }
        self.ui.ui_mode = mode.to_owned();
        true
    }

    /// Records the reduced-motion choice. Returns false (and changes nothing) for an unknown mode.
    pub fn set_reduced_motion(&mut self, mode: &str) -> bool {
        if !MOTION_MODES.contains(&mode) {
            return false;
        }
        self.ui.reduced_motion = mode.to_owned();
        true
    }

    /// Records the transparency-effects choice. Returns false (and changes nothing) for an unknown mode.
    pub fn set_transparency(&mut self, mode: &str) -> bool {
        if !TRANSPARENCY_MODES.contains(&mode) {
            return false;
        }
        self.ui.transparency = mode.to_owned();
        true
    }

    /// Sets the global game memory (`None` = automatic). A value outside the safe range for this
    /// machine is refused with that range, never silently changed. Returns whether it changed.
    pub fn set_memory(
        &mut self,
        mb: Option<u32>,
        total_mb: Option<u64>,
    ) -> Result<bool, (u32, u32)> {
        let before = self.minecraft.memory_mb;
        self.minecraft.memory_mb = match mb {
            None => None,
            Some(v) => {
                let (min, max) = crate::system::memory_limits_mb(total_mb);
                if v < min || v > max {
                    return Err((min, max));
                }
                Some(v / 256 * 256)
            }
        };
        Ok(self.minecraft.memory_mb != before)
    }

    /// Sets what the window does around a game. Unknown values are refused (false, no change).
    pub fn set_behavior(&mut self, on_launch: Option<&str>, on_exit: Option<&str>) -> bool {
        if on_launch.is_some_and(|v| !ON_LAUNCH_MODES.contains(&v))
            || on_exit.is_some_and(|v| !ON_EXIT_MODES.contains(&v))
        {
            return false;
        }
        if let Some(v) = on_launch {
            self.behavior.on_launch = v.to_owned();
        }
        if let Some(v) = on_exit {
            self.behavior.on_exit = v.to_owned();
        }
        true
    }

    /// Marks the first-run introduction as done. Returns whether anything changed.
    pub fn complete_onboarding(&mut self) -> bool {
        let changed = !self.ui.onboarding_done;
        self.ui.onboarding_done = true;
        changed
    }

    /// Puts every user-configurable preference back to its default. It keeps whether the
    /// introduction was seen, the selected profile, the last headline and the download
    /// concurrency, and it never touches profiles, accounts, the vault, game files or playtime
    /// (none of them is stored in this file). Returns whether anything changed.
    pub fn reset_user_prefs(&mut self) -> bool {
        let before = self.clone();
        let (done, phrase) = (self.ui.onboarding_done, self.ui.last_phrase.clone());
        self.ui = UiPrefs::default();
        self.ui.onboarding_done = done;
        self.ui.last_phrase = phrase;
        self.minecraft = MinecraftPrefs::default();
        self.behavior = BehaviorPrefs::default();
        *self != before
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        Ok(atomic_write(path, &serde_json::to_vec_pretty(self)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_then_roundtrip_with_relative_path() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let mut s = LauncherSettings::load_or_default(&p).unwrap();
        s.last_instance = Some(InstanceId::new("fabric-1.21.4").unwrap());
        s.save(&p).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.contains("fabric-1.21.4"));
        // No drive letters or UNC prefixes anywhere in the file.
        assert!(!text.contains(":/") && !text.contains(":\\") && !text.contains("\\\\"));
        assert_eq!(LauncherSettings::load_or_default(&p).unwrap(), s);
    }

    #[test]
    fn absolute_or_escaping_paths_in_a_copied_file_never_survive_and_never_stop_startup() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        for bad in ["C:/Users/x/.minecraft", "../../outside", "/etc", "Upper"] {
            // the one unusable value is reset, the other settings are kept
            fs::write(
                &p,
                format!("{{\"last_instance\":\"{bad}\",\"ui\":{{\"language\":\"ru\"}}}}"),
            )
            .unwrap();
            let s = LauncherSettings::load_or_default(&p).unwrap();
            assert_eq!(s.last_instance, None, "{bad}");
            assert_eq!(s.ui.language.as_deref(), Some("ru"), "{bad}");
        }
    }

    #[test]
    fn newer_layout_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(&p, "{\"layout_version\": 999}").unwrap();
        assert!(matches!(
            LauncherSettings::load_or_default(&p),
            Err(SettingsError::FolderTooNew { .. })
        ));
    }

    #[test]
    fn ui_prefs_default_to_enabled_at_half_volume_and_old_files_still_load() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        // A settings file from before UI prefs existed: loads with the defaults.
        fs::write(&p, r#"{"layout_version":1,"max_concurrent_downloads":6}"#).unwrap();
        let s = LauncherSettings::load_or_default(&p).unwrap();
        assert_eq!(s.ui, UiPrefs::default());
        assert!(s.ui.sound.enabled && (s.ui.sound.volume - 0.5).abs() < f32::EPSILON);
        assert_eq!(s.ui.last_phrase, None);
        assert_eq!(s.max_concurrent_downloads, 6);
    }

    #[test]
    fn ui_prefs_round_trip_through_the_portable_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("data").join("settings.json");
        let mut s = LauncherSettings::default();
        assert!(s.set_sound(Some(false), Some(0.25)));
        assert!(s.set_last_phrase("take-your-time"));
        s.save(&p).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.contains("take-your-time") && !text.contains(":\\") && !text.contains(":/"));
        let back = LauncherSettings::load_or_default(&p).unwrap();
        assert_eq!(back, s);
        assert!(!back.ui.sound.enabled && (back.ui.sound.volume - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn damaged_ui_values_are_repaired_not_fatal() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(
            &p,
            r#"{"ui":{"sound":{"enabled":false,"volume":7.5},"last_phrase":"../../evil"}}"#,
        )
        .unwrap();
        let s = LauncherSettings::load_or_default(&p).unwrap();
        assert!(
            !s.ui.sound.enabled,
            "valid parts of a damaged section are kept"
        );
        assert!(
            (s.ui.sound.volume - 1.0).abs() < f32::EPSILON,
            "volume is clamped"
        );
        assert_eq!(s.ui.last_phrase, None, "an unsafe phrase id is dropped");
        fs::write(&p, r#"{"ui":{"sound":{"volume":-3}}}"#).unwrap();
        assert!(
            LauncherSettings::load_or_default(&p)
                .unwrap()
                .ui
                .sound
                .volume
                .abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn language_is_unset_until_chosen_then_persists_and_bad_values_are_dropped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("data").join("settings.json");
        let mut s = LauncherSettings::default();
        assert_eq!(s.ui.language, None, "never chosen: the UI follows Windows");
        assert!(!s.set_language("de"), "only English and Russian exist");
        assert!(!s.set_language("../ru"));
        assert_eq!(s.ui.language, None);
        assert!(s.set_language("ru"));
        s.save(&p).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert!(!text.contains(":\\") && !text.contains(":/"));
        assert_eq!(
            LauncherSettings::load_or_default(&p)
                .unwrap()
                .ui
                .language
                .as_deref(),
            Some("ru")
        );
        // A hand-edited or damaged value is repaired to "never chosen", not fatal.
        fs::write(&p, r#"{"ui":{"language":"klingon"}}"#).unwrap();
        assert_eq!(
            LauncherSettings::load_or_default(&p).unwrap().ui.language,
            None
        );
        // Older files without the field load as "never chosen".
        fs::write(&p, r#"{"ui":{"sound":{"enabled":true,"volume":0.5}}}"#).unwrap();
        assert_eq!(
            LauncherSettings::load_or_default(&p).unwrap().ui.language,
            None
        );
    }

    #[test]
    fn setters_validate_and_report_changes() {
        let mut s = LauncherSettings::default();
        assert!(!s.set_sound(None, None));
        assert!(
            !s.set_sound(Some(true), Some(0.5)),
            "same values are not a change"
        );
        // NaN is repaired to the default rather than stored.
        assert!(!s.set_sound(None, Some(f32::NAN)));
        assert!((s.ui.sound.volume - 0.5).abs() < f32::EPSILON);
        assert!(s.set_sound(None, Some(9.0)));
        assert!((s.ui.sound.volume - 1.0).abs() < f32::EPSILON);
        for bad in ["", "Upper", "a b", "../x", "x/y", &"a".repeat(65)] {
            assert!(!s.set_last_phrase(bad), "{bad:?}");
            assert_eq!(s.ui.last_phrase, None);
        }
        assert!(s.set_last_phrase("welcome-back"));
        assert_eq!(s.ui.last_phrase.as_deref(), Some("welcome-back"));
    }

    #[test]
    fn new_fields_default_safely_and_old_files_load_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        // A file written before motion, memory, behavior and onboarding existed.
        fs::write(
            &p,
            r#"{"layout_version":1,"last_instance":"survival","ui":{"sound":{"enabled":false,"volume":0.2},"language":"ru"}}"#,
        )
        .unwrap();
        let s = LauncherSettings::load_or_default(&p).unwrap();
        assert_eq!(s.ui.reduced_motion, "system");
        assert_eq!(s.ui.transparency, "system");
        assert!(
            !s.ui.onboarding_done,
            "an old folder shows the introduction once"
        );
        assert_eq!(s.minecraft, MinecraftPrefs { memory_mb: None });
        assert_eq!(s.behavior.on_launch, "hide");
        assert_eq!(s.behavior.on_exit, "show");
        // what was there is untouched
        assert!(!s.ui.sound.enabled && s.ui.language.as_deref() == Some("ru"));
        assert_eq!(
            s.last_instance.as_ref().map(|i| i.as_str()),
            Some("survival")
        );
    }

    #[test]
    fn the_new_fields_serialize_deterministically_and_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("data").join("settings.json");
        let mut s = LauncherSettings::default();
        assert!(s.set_reduced_motion("on"));
        assert!(s.set_transparency("off"));
        assert_eq!(s.set_memory(Some(4096), Some(16_384)), Ok(true));
        assert!(s.set_behavior(Some("hide"), Some("stay_hidden")));
        assert!(s.complete_onboarding());
        s.save(&p).unwrap();
        let first = fs::read_to_string(&p).unwrap();
        s.save(&p).unwrap();
        assert_eq!(
            first,
            fs::read_to_string(&p).unwrap(),
            "same input, same bytes"
        );
        for needle in [
            "\"reduced_motion\": \"on\"",
            "\"transparency\": \"off\"",
            "\"onboarding_done\": true",
            "\"memory_mb\": 4096",
            "\"on_launch\": \"hide\"",
            "\"on_exit\": \"stay_hidden\"",
        ] {
            assert!(first.contains(needle), "{needle} missing from {first}");
        }
        assert_eq!(LauncherSettings::load_or_default(&p).unwrap(), s);
    }

    #[test]
    fn memory_is_validated_against_the_machine() {
        let mut s = LauncherSettings::default();
        assert_eq!(s.set_memory(Some(4096), Some(16_384)), Ok(true));
        assert_eq!(
            s.set_memory(Some(4096), Some(16_384)),
            Ok(false),
            "same value, no change"
        );
        assert_eq!(
            s.set_memory(Some(4150), Some(16_384)),
            Ok(false),
            "snapped to a 256 MiB step: still 4096"
        );
        assert_eq!(s.minecraft.memory_mb, Some(4096));
        assert_eq!(
            s.set_memory(Some(512), Some(16_384)),
            Err((1024, 12_288)),
            "too small"
        );
        assert_eq!(
            s.set_memory(Some(16_000), Some(16_384)),
            Err((1024, 12_288)),
            "too large for this machine"
        );
        assert_eq!(
            s.minecraft.memory_mb,
            Some(4096),
            "a refused value changes nothing"
        );
        assert_eq!(s.set_memory(None, Some(16_384)), Ok(true));
        assert_eq!(s.minecraft.memory_mb, None, "back to automatic");
    }

    #[test]
    fn a_damaged_memory_value_in_a_hand_edited_file_is_repaired() {
        let mut m = MinecraftPrefs {
            memory_mb: Some(900_000),
        };
        m.sanitize();
        assert_eq!(m.memory_mb, Some(32_768));
        let mut m = MinecraftPrefs { memory_mb: Some(5) };
        m.sanitize();
        assert_eq!(m.memory_mb, Some(1024));
        let mut m = MinecraftPrefs::default();
        m.sanitize();
        assert_eq!(m.memory_mb, None, "automatic stays automatic");
    }

    #[test]
    fn portable_memory_survives_across_machines_and_download_concurrency_is_clamped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");

        for preserved in [8192u32, 16_384u32] {
            fs::write(
                &p,
                format!(
                    r#"{{"layout_version":1,"max_concurrent_downloads":0,"minecraft":{{"memory_mb":{preserved}}}}}"#
                ),
            )
            .unwrap();
            let loaded = LauncherSettings::load_or_default(&p).unwrap();
            assert_eq!(loaded.minecraft.memory_mb, Some(preserved));
            assert_eq!(loaded.max_concurrent_downloads, 1);
            loaded.save(&p).unwrap();
            let round = LauncherSettings::load_or_default(&p).unwrap();
            assert_eq!(round.minecraft.memory_mb, Some(preserved));
            assert_eq!(round.max_concurrent_downloads, 1);
        }

        fs::write(&p, r#"{"layout_version":1,"max_concurrent_downloads":100}"#).unwrap();
        let clamped_high = LauncherSettings::load_or_default(&p).unwrap();
        assert_eq!(clamped_high.max_concurrent_downloads, 16);
    }

    #[test]
    fn transparency_is_independent_of_reduced_motion_and_rejects_unknown_values() {
        let mut s = LauncherSettings::default();
        assert_eq!(s.ui.transparency, "system");
        assert!(!s.set_transparency("glass"));
        assert!(!s.set_transparency(""));
        assert_eq!(s.ui.transparency, "system");
        // the two preferences never imply each other
        assert!(s.set_reduced_motion("on"));
        assert_eq!(s.ui.transparency, "system");
        assert!(s.set_transparency("on"));
        assert!(s.set_transparency("off"));
        assert_eq!(s.ui.reduced_motion, "on");
        assert_eq!(s.ui.transparency, "off");
    }

    #[test]
    fn behavior_and_motion_reject_unknown_values() {
        let mut s = LauncherSettings::default();
        assert!(!s.set_behavior(Some("minimize"), None));
        assert!(!s.set_behavior(None, Some("quit")));
        assert!(!s.set_reduced_motion("sometimes"));
        assert_eq!(
            (s.behavior.clone(), s.ui.reduced_motion.as_str()),
            (BehaviorPrefs::default(), "system")
        );
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(&p, r#"{"behavior":{"on_launch":"explode","on_exit":"stay_hidden"},"ui":{"reduced_motion":"maybe"}}"#).unwrap();
        let s = LauncherSettings::load_or_default(&p).unwrap();
        assert_eq!(
            s.behavior.on_launch, "hide",
            "only the bad value is repaired"
        );
        assert_eq!(s.behavior.on_exit, "stay_hidden");
        assert_eq!(s.ui.reduced_motion, "system");
    }

    #[test]
    fn onboarding_completion_is_permanent_and_survives_a_reload() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("data").join("settings.json");
        let mut s = LauncherSettings::default();
        assert!(!s.ui.onboarding_done);
        assert!(s.complete_onboarding());
        assert!(!s.complete_onboarding(), "completing twice changes nothing");
        s.save(&p).unwrap();
        // the same file on another drive or PC
        assert!(
            LauncherSettings::load_or_default(&p)
                .unwrap()
                .ui
                .onboarding_done
        );
    }

    #[test]
    fn reset_restores_preferences_but_not_the_introduction_or_anything_else() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // user content that lives next to the settings file and must never be touched
        for (rel, bytes) in [
            ("data/vault.bin", &b"VAULT"[..]),
            ("data/accounts.json", &b"ACCOUNTS"[..]),
            ("data/playtime.json", &b"PLAYTIME"[..]),
            ("instances/p/game/saves/w/level.dat", &b"WORLD"[..]),
            ("minecraft/versions/1.21.4/1.21.4.json", &b"MC"[..]),
        ] {
            let f = root.join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, bytes).unwrap();
        }
        let p = root.join("data").join("settings.json");
        let mut s = LauncherSettings::default();
        s.set_sound(Some(false), Some(0.1));
        s.set_language("ru");
        s.set_reduced_motion("on");
        s.set_transparency("off");
        s.set_memory(Some(6144), Some(16_384)).unwrap();
        s.set_behavior(Some("keep_open"), Some("stay_hidden"));
        s.complete_onboarding();
        s.set_last_phrase("take-your-time");
        s.last_instance = Some(InstanceId::new("p").unwrap());
        s.max_concurrent_downloads = 3;
        s.save(&p).unwrap();

        assert!(s.reset_user_prefs());
        s.save(&p).unwrap();
        let back = LauncherSettings::load_or_default(&p).unwrap();
        assert!(back.ui.sound.enabled && (back.ui.sound.volume - 0.5).abs() < f32::EPSILON);
        assert_eq!(back.ui.language, None);
        assert_eq!(back.ui.reduced_motion, "system");
        assert_eq!(back.ui.transparency, "system");
        assert_eq!(back.minecraft.memory_mb, None);
        assert_eq!(back.behavior, BehaviorPrefs::default());
        // not reset: the introduction was seen, the selection and download concurrency stay
        assert!(back.ui.onboarding_done);
        assert_eq!(back.ui.last_phrase.as_deref(), Some("take-your-time"));
        assert_eq!(back.last_instance.as_ref().map(|i| i.as_str()), Some("p"));
        assert_eq!(back.max_concurrent_downloads, 3);
        assert!(!s.reset_user_prefs(), "a second reset has nothing to do");
        // and everything else on disk is exactly as it was
        for (rel, bytes) in [
            ("data/vault.bin", &b"VAULT"[..]),
            ("data/accounts.json", &b"ACCOUNTS"[..]),
            ("data/playtime.json", &b"PLAYTIME"[..]),
            ("instances/p/game/saves/w/level.dat", &b"WORLD"[..]),
            ("minecraft/versions/1.21.4/1.21.4.json", &b"MC"[..]),
        ] {
            assert_eq!(fs::read(root.join(rel)).unwrap(), bytes, "{rel}");
        }
    }

    // ---- startup recovery of a damaged settings file

    fn recover(contents: &[u8]) -> (tempfile::TempDir, std::path::PathBuf, LauncherSettings) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(&p, contents).unwrap();
        let s = LauncherSettings::load_or_recover(&p).unwrap();
        (d, p, s)
    }

    #[test]
    fn a_damaged_file_never_stops_startup_and_is_kept_aside() {
        // not a settings document at all: unreadable as JSON, or not an object
        let damaged: [&[u8]; 6] = [
            b"",
            b"{\"layout_version\": 1, \"ui\": {\"sound\": {\"enabled\": tr",
            b"this is not json at all",
            b"[1, 2, 3]",
            b"null",
            b"\"just a string\"",
        ];
        for bytes in damaged {
            let (d, p, s) = recover(bytes);
            assert_eq!(s, LauncherSettings::default(), "{bytes:?}");
            assert!(!p.exists(), "the damaged file left its place");
            assert_eq!(
                fs::read(d.path().join("settings.json.corrupt")).unwrap(),
                bytes,
                "the damaged file was preserved untouched"
            );
        }
    }

    #[test]
    fn an_earlier_corrupt_backup_is_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(d.path().join("settings.json.corrupt"), b"first").unwrap();
        fs::write(&p, b"{ broken").unwrap();
        LauncherSettings::load_or_recover(&p).unwrap();
        assert_eq!(
            fs::read(d.path().join("settings.json.corrupt")).unwrap(),
            b"first"
        );
        let others: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("settings.json.corrupt-"))
            .collect();
        assert_eq!(others.len(), 1);
        assert_eq!(fs::read(d.path().join(&others[0])).unwrap(), b"{ broken");
    }

    #[test]
    fn recovery_leaves_valid_missing_and_newer_files_alone() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        // missing: defaults, nothing created
        assert_eq!(
            LauncherSettings::load_or_recover(&p).unwrap(),
            LauncherSettings::default()
        );
        assert!(!p.exists());
        // an old file (before the newer sections existed) loads as before and stays put
        let old = r#"{"layout_version":1,"last_instance":"survival","ui":{"sound":{"enabled":false,"volume":0.2},"language":"ru"}}"#;
        fs::write(&p, old).unwrap();
        let s = LauncherSettings::load_or_recover(&p).unwrap();
        assert_eq!(s.ui.language.as_deref(), Some("ru"));
        assert_eq!(fs::read_to_string(&p).unwrap(), old);
        // a newer Clade's file is an error, not "damage": it must not be moved aside
        fs::write(&p, r#"{"layout_version": 999, "ui": 5}"#).unwrap();
        assert!(matches!(
            LauncherSettings::load_or_recover(&p),
            Err(SettingsError::FolderTooNew { .. })
        ));
        assert!(p.is_file());
    }

    #[test]
    fn a_real_filesystem_failure_is_not_turned_into_defaults() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        // a directory where the file should be: reading it fails for a reason that is not "damage"
        fs::create_dir(&p).unwrap();
        assert!(matches!(
            LauncherSettings::load_or_recover(&p),
            Err(SettingsError::Io(_))
        ));
        assert!(p.is_dir(), "nothing was moved or replaced");
        assert!(!d.path().join("settings.json.corrupt").exists());
    }

    // ---- valid JSON with unusable values: repair those values only, keep everything else

    /// A settings document in which every setting has a distinctive, valid, non-default value.
    fn full_valid() -> serde_json::Value {
        serde_json::json!({
            "layout_version": 1,
            "last_instance": "survival",
            "max_concurrent_downloads": 5,
            "ui": {
                "sound": { "enabled": false, "volume": 0.25 },
                "last_phrase": "take-your-time",
                "language": "ru",
                "reduced_motion": "on",
                "transparency": "off",
                "onboarding_done": true
            },
            "minecraft": { "memory_mb": 4096 },
            "behavior": { "on_launch": "keep_open", "on_exit": "stay_hidden" }
        })
    }

    fn load_json(v: &serde_json::Value) -> (tempfile::TempDir, LauncherSettings) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(&p, serde_json::to_vec(v).unwrap()).unwrap();
        let s = LauncherSettings::load_or_recover(&p).unwrap();
        assert!(p.is_file(), "a repairable file is never moved aside");
        let aside = fs::read_dir(d.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".corrupt")
            })
            .count();
        assert_eq!(aside, 0, "no quarantine for a file that can be repaired");
        (d, s)
    }

    #[test]
    fn a_fully_valid_file_loads_exactly_as_written() {
        let (_d, s) = load_json(&full_valid());
        assert_eq!(
            s.last_instance.as_ref().map(|i| i.as_str()),
            Some("survival")
        );
        assert_eq!(s.max_concurrent_downloads, 5);
        assert!(!s.ui.sound.enabled && (s.ui.sound.volume - 0.25).abs() < f32::EPSILON);
        assert_eq!(s.ui.last_phrase.as_deref(), Some("take-your-time"));
        assert_eq!(s.ui.language.as_deref(), Some("ru"));
        assert_eq!(s.ui.reduced_motion, "on");
        assert_eq!(s.ui.transparency, "off");
        assert!(s.ui.onboarding_done);
        assert_eq!(s.minecraft.memory_mb, Some(4096));
        assert_eq!(s.behavior.on_launch, "keep_open");
        assert_eq!(s.behavior.on_exit, "stay_hidden");
    }

    /// Every other setting of `full_valid` must be unchanged after breaking one value.
    fn assert_siblings_kept(s: &LauncherSettings, except: &str) {
        let keep = |name: &str| name != except;
        if keep("last_instance") {
            assert_eq!(
                s.last_instance.as_ref().map(|i| i.as_str()),
                Some("survival")
            );
        }
        if keep("max_concurrent_downloads") {
            assert_eq!(s.max_concurrent_downloads, 5);
        }
        if keep("ui.sound.enabled") {
            assert!(!s.ui.sound.enabled);
        }
        if keep("ui.sound.volume") {
            assert!((s.ui.sound.volume - 0.25).abs() < f32::EPSILON);
        }
        if keep("ui.last_phrase") {
            assert_eq!(s.ui.last_phrase.as_deref(), Some("take-your-time"));
        }
        if keep("ui.language") {
            assert_eq!(s.ui.language.as_deref(), Some("ru"));
        }
        if keep("ui.reduced_motion") {
            assert_eq!(s.ui.reduced_motion, "on");
        }
        if keep("ui.transparency") {
            assert_eq!(s.ui.transparency, "off");
        }
        if keep("ui.onboarding_done") {
            assert!(s.ui.onboarding_done);
        }
        if keep("minecraft.memory_mb") {
            assert_eq!(s.minecraft.memory_mb, Some(4096));
        }
        if keep("behavior.on_launch") {
            assert_eq!(s.behavior.on_launch, "keep_open");
        }
        if keep("behavior.on_exit") {
            assert_eq!(s.behavior.on_exit, "stay_hidden");
        }
    }

    #[test]
    fn a_value_that_is_valid_json_but_not_supported_is_repaired_alone() {
        // (path, bad value, what it must become, or None when only "siblings kept" is checked)
        use serde_json::json;
        let cases = [
            ("ui.reduced_motion", json!("sometimes"), "ui.reduced_motion"),
            ("ui.reduced_motion", json!(7), "ui.reduced_motion"),
            ("ui.transparency", json!("glassy"), "ui.transparency"),
            ("ui.transparency", json!(true), "ui.transparency"),
            ("behavior.on_launch", json!("explode"), "behavior.on_launch"),
            ("behavior.on_launch", json!(true), "behavior.on_launch"),
            ("behavior.on_exit", json!("quit"), "behavior.on_exit"),
            ("minecraft.memory_mb", json!(900_000), "minecraft.memory_mb"),
            ("minecraft.memory_mb", json!(-5), "minecraft.memory_mb"),
            ("minecraft.memory_mb", json!("lots"), "minecraft.memory_mb"),
            ("ui.sound.volume", json!(9.0), "ui.sound.volume"),
            ("ui.sound.volume", json!("loud"), "ui.sound.volume"),
            ("ui.sound.enabled", json!("yes"), "ui.sound.enabled"),
            ("ui.language", json!("fr"), "ui.language"),
            ("ui.language", json!(3), "ui.language"),
            ("ui.last_phrase", json!("NOT a slug!"), "ui.last_phrase"),
            ("ui.onboarding_done", json!("done"), "ui.onboarding_done"),
            (
                "max_concurrent_downloads",
                json!(100_000),
                "max_concurrent_downloads",
            ),
            ("last_instance", json!("../../elsewhere"), "last_instance"),
            ("last_instance", json!(12), "last_instance"),
        ];
        for (path, bad, name) in cases {
            let mut doc = full_valid();
            let mut cur = &mut doc;
            let parts: Vec<&str> = path.split('.').collect();
            for part in &parts[..parts.len() - 1] {
                cur = cur.get_mut(*part).unwrap();
            }
            cur[parts[parts.len() - 1]] = bad.clone();
            let (_d, s) = load_json(&doc);
            assert_siblings_kept(&s, name);
            // and the broken value itself ended up somewhere safe
            match name {
                "ui.reduced_motion" => assert_eq!(s.ui.reduced_motion, "system", "{bad}"),
                "ui.transparency" => assert_eq!(s.ui.transparency, "system", "{bad}"),
                "behavior.on_launch" => assert_eq!(s.behavior.on_launch, "hide", "{bad}"),
                "behavior.on_exit" => assert_eq!(s.behavior.on_exit, "show", "{bad}"),
                "minecraft.memory_mb" => {
                    let m = s.minecraft.memory_mb;
                    assert!(
                        m.is_none() || m.is_some_and(|v| (1024..=32_768).contains(&v)),
                        "{bad}"
                    );
                }
                "ui.sound.volume" => assert!((0.0..=1.0).contains(&s.ui.sound.volume), "{bad}"),
                "ui.language" => assert_eq!(s.ui.language, None, "{bad}"),
                "ui.last_phrase" => assert_eq!(s.ui.last_phrase, None, "{bad}"),
                "last_instance" => assert_eq!(s.last_instance, None, "{bad}"),
                _ => {}
            }
        }
    }

    #[test]
    fn a_whole_section_of_the_wrong_shape_resets_only_that_section() {
        for section in ["ui", "minecraft", "behavior"] {
            let mut doc = full_valid();
            doc[section] = serde_json::json!([1, 2, 3]);
            let (_d, s) = load_json(&doc);
            match section {
                "ui" => {
                    assert_eq!(s.ui, UiPrefs::default());
                    assert_eq!(s.minecraft.memory_mb, Some(4096));
                    assert_eq!(s.behavior.on_launch, "keep_open");
                }
                "minecraft" => {
                    assert_eq!(s.minecraft, MinecraftPrefs::default());
                    assert_eq!(s.ui.language.as_deref(), Some("ru"));
                    assert_eq!(s.behavior.on_exit, "stay_hidden");
                }
                _ => {
                    assert_eq!(s.behavior, BehaviorPrefs::default());
                    assert_eq!(s.ui.language.as_deref(), Some("ru"));
                    assert_eq!(s.minecraft.memory_mb, Some(4096));
                }
            }
            assert_eq!(
                s.last_instance.as_ref().map(|i| i.as_str()),
                Some("survival")
            );
        }
    }

    #[test]
    fn several_unusable_values_are_each_reset_and_the_rest_survive() {
        let mut doc = full_valid();
        doc["ui"]["reduced_motion"] = serde_json::json!(false);
        doc["behavior"]["on_exit"] = serde_json::json!("nope");
        doc["minecraft"]["memory_mb"] = serde_json::json!("x");
        doc["last_instance"] = serde_json::json!("C:/x");
        let (_d, s) = load_json(&doc);
        assert_eq!(s.ui.reduced_motion, "system");
        assert_eq!(s.behavior.on_exit, "show");
        assert_eq!(s.minecraft.memory_mb, None);
        assert_eq!(s.last_instance, None);
        // untouched
        assert_eq!(s.ui.language.as_deref(), Some("ru"));
        assert_eq!(s.behavior.on_launch, "keep_open");
        assert!(s.ui.onboarding_done);
        assert!(!s.ui.sound.enabled);
    }

    #[test]
    fn a_newer_layout_is_still_reported_even_if_its_shape_is_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(&p, r#"{"layout_version": 999, "ui": 5, "minecraft": "x"}"#).unwrap();
        assert!(matches!(
            LauncherSettings::load_or_recover(&p),
            Err(SettingsError::FolderTooNew { .. })
        ));
        assert!(p.is_file());
    }

    #[test]
    fn ui_mode_defaults_to_simple_switches_to_advanced_and_sanitizes_unknown_values() {
        let mut s = LauncherSettings::default();
        assert_eq!(s.ui.ui_mode, "simple");
        assert!(s.set_ui_mode("advanced"));
        assert_eq!(s.ui.ui_mode, "advanced");
        assert!(!s.set_ui_mode("expert"));
        assert_eq!(s.ui.ui_mode, "advanced");
        assert!(s.reset_user_prefs());
        assert_eq!(s.ui.ui_mode, "simple");

        let mut doc = full_valid();
        doc["ui"]["ui_mode"] = serde_json::json!("turbo");
        let (_d, salvaged) = load_json(&doc);
        assert_eq!(salvaged.ui.ui_mode, "simple");
        assert_eq!(salvaged.ui.language.as_deref(), Some("ru"));
    }
}
