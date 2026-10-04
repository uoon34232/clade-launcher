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

/// Launcher-owned UI state. This is authoritative; the WebView profile in `data/webview2` is
/// disposable browser-engine cache and may be deleted at any time without losing any of it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub sound: SoundPrefs,
    /// Id of the home-screen headline shown last, so the next start shows a different one.
    pub last_phrase: Option<String>,
    /// The language the person chose (`en` or `ru`). `None` means "never chosen": the UI then
    /// follows the Windows language, and an explicit choice is never overridden afterwards.
    pub language: Option<String>,
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
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            layout_version: LAYOUT_VERSION,
            last_instance: None,
            max_concurrent_downloads: 8,
            ui: UiPrefs::default(),
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
}
