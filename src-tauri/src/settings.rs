//! Launcher settings (`data/settings.json`).
//!
//! Every path stored here is a `RelPath`: relative to the launcher root, validated on load.
//! A settings file copied between PCs, or edited by hand, cannot point outside the folder.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::instances::InstanceId;
use crate::portable::fsutil::atomic_write;
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
    /// `system` (follow Windows), `on` or `off`.
    pub reduced_motion: String,
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
            reduced_motion: "system".to_owned(),
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
    /// A damaged or hand-edited value is repaired to a safe one; it never stops the launcher.
    pub fn sanitize(&mut self, total_mb: Option<u64>) {
        if let Some(mb) = self.memory_mb {
            let (min, max) = crate::system::memory_limits_mb(total_mb);
            self.memory_mb = Some(mb.clamp(min, max) / 256 * 256);
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
            on_launch: "keep_open".to_owned(),
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
        if !MOTION_MODES.contains(&self.reduced_motion.as_str()) {
            self.reduced_motion = Self::default().reduced_motion;
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
    pub fn load_or_default(path: &Path) -> Result<Self, SettingsError> {
        let mut s: Self = match fs::read(path) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        if s.layout_version > LAYOUT_VERSION {
            return Err(SettingsError::FolderTooNew {
                found: s.layout_version,
                supported: LAYOUT_VERSION,
            });
        }
        s.ui.sanitize();
        s.behavior.sanitize();
        s.minecraft.sanitize(crate::system::total_memory_mb());
        Ok(s)
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

    /// Records the reduced-motion choice. Returns false (and changes nothing) for an unknown mode.
    pub fn set_reduced_motion(&mut self, mode: &str) -> bool {
        if !MOTION_MODES.contains(&mode) {
            return false;
        }
        self.ui.reduced_motion = mode.to_owned();
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
    fn absolute_or_escaping_paths_in_a_copied_file_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        for bad in ["C:/Users/x/.minecraft", "../../outside", "/etc", "Upper"] {
            fs::write(&p, format!("{{\"last_instance\":\"{bad}\"}}")).unwrap();
            assert!(LauncherSettings::load_or_default(&p).is_err(), "{bad}");
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
        assert!(
            !s.ui.onboarding_done,
            "an old folder shows the introduction once"
        );
        assert_eq!(s.minecraft, MinecraftPrefs { memory_mb: None });
        assert_eq!(s.behavior.on_launch, "keep_open");
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
        m.sanitize(Some(16_384));
        assert_eq!(m.memory_mb, Some(12_288));
        let mut m = MinecraftPrefs { memory_mb: Some(5) };
        m.sanitize(Some(16_384));
        assert_eq!(m.memory_mb, Some(1024));
        let mut m = MinecraftPrefs::default();
        m.sanitize(Some(16_384));
        assert_eq!(m.memory_mb, None, "automatic stays automatic");
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
            s.behavior.on_launch, "keep_open",
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
        s.set_memory(Some(6144), Some(16_384)).unwrap();
        s.set_behavior(Some("hide"), Some("stay_hidden"));
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
}
