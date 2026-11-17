//! Text for the native dialogs that can appear before the React UI exists (a folder already in
//! use, a missing WebView2, a failed start).
//!
//! This is deliberately small and independent of the frontend and of the main settings system, so
//! a failure there can still be reported: the language is read straight from `data/settings.json`
//! with a tolerant, self-contained parser. The rule matches the frontend: an explicit
//! `ui.language` (`en` / `ru`) wins; otherwise the Windows display language decides (`ru...` is
//! Russian, anything else English). Missing, unreadable or malformed settings fall back to the
//! Windows language and never fail. Brand names and technical details stay as they are; the
//! English detail line is for troubleshooting.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Ru,
}

/// Every message a native startup dialog can show. One id per situation; the text is chosen at
/// the final native boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupMessage {
    /// The folder is locked by another Clade (on another PC; one on this PC is woken instead).
    FolderInUse,
    /// The folder or stick cannot be written to.
    FolderNotWritable,
    /// The executable's own location could not be determined.
    CannotLocateFolder,
    /// The folder was written by a newer Clade.
    FolderTooNew,
    /// Any other failure while initialising (unreadable settings, storage errors).
    StartupFailed,
    /// Neither the bundled nor an installed WebView2 runtime can be loaded.
    WebViewMissing,
    /// The main window could not be created or the app failed while running.
    WindowFailed,
}

pub const ALL_MESSAGES: [StartupMessage; 7] = [
    StartupMessage::FolderInUse,
    StartupMessage::FolderNotWritable,
    StartupMessage::CannotLocateFolder,
    StartupMessage::FolderTooNew,
    StartupMessage::StartupFailed,
    StartupMessage::WebViewMissing,
    StartupMessage::WindowFailed,
];

/// The window title of every native dialog: the formal product name, never translated.
pub const TITLE: &str = "Clade Launcher";

/// The language rule shared with the frontend, as a pure function over what was read.
/// `settings_json` is the raw content of `data/settings.json` if it could be read.
pub fn resolve_language(settings_json: Option<&[u8]>, system_locale: Option<&str>) -> Language {
    if let Some(bytes) = settings_json {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
            match v.pointer("/ui/language").and_then(|l| l.as_str()) {
                Some("en") => return Language::En,
                Some("ru") => return Language::Ru,
                _ => {} // absent or not a shipped language: same as never chosen
            }
        }
    }
    if system_locale.is_some_and(is_russian_locale) {
        Language::Ru
    } else {
        Language::En
    }
}

fn is_russian_locale(locale: &str) -> bool {
    let l = locale.trim().to_ascii_lowercase();
    l == "ru" || l.starts_with("ru-") || l.starts_with("ru_")
}

/// The language for a dialog shown now. `root` is the launcher folder if it is already known;
/// without it (or with unreadable settings) the Windows language decides. Never fails.
pub fn current_language(root: Option<&Path>) -> Language {
    let settings = root.and_then(|r| {
        crate::portable::fsutil::read_bounded(&r.join("data").join("settings.json"), 64 * 1024).ok()
    });
    resolve_language(
        settings.as_deref(),
        crate::system::system_locale().as_deref(),
    )
}

/// The user-facing text (heading, then a short explanation) for a message.
pub fn text(id: StartupMessage, lang: Language) -> &'static str {
    use Language::{En, Ru};
    use StartupMessage::*;
    match (id, lang) {
        (FolderInUse, En) => "Clade is already using this folder.\n\nIt may be running on another PC. Close it there, then try again.",
        (FolderInUse, Ru) => "Clade уже использует эту папку.\n\nВозможно, он запущен на другом компьютере. Закрой его там и попробуй снова.",
        (FolderNotWritable, En) => "Clade can't write to its folder.\n\nCheck that the folder or USB stick isn't write-protected.",
        (FolderNotWritable, Ru) => "Clade не может записывать в свою папку.\n\nПроверь, что папка или флешка не защищена от записи.",
        (CannotLocateFolder, En) => "Clade couldn't work out which folder it runs from.\n\nStart Clade.exe from its own folder and try again.",
        (CannotLocateFolder, Ru) => "Clade не смог определить свою папку.\n\nЗапусти Clade.exe из его собственной папки и попробуй снова.",
        (FolderTooNew, En) => "This folder was made by a newer version of Clade.\n\nUse the newer Clade.exe with this folder.",
        (FolderTooNew, Ru) => "Эта папка создана более новой версией Clade.\n\nИспользуй с ней более новый Clade.exe.",
        (StartupFailed, En) => "Clade couldn't start.\n\nCheck that its folder is readable and not damaged, then try again.",
        (StartupFailed, Ru) => "Clade не удалось запустить.\n\nПроверь, что его папка доступна для чтения и не повреждена, и попробуй снова.",
        (WebViewMissing, En) => "Clade couldn't start its web view.\n\nClade normally carries its own WebView2 in the runtimes\\webview2 folder next to Clade.exe, but it wasn't found and WebView2 isn't installed on this PC.\n\nExtract the complete Clade-Launcher-Portable.zip again (don't copy Clade.exe on its own) and make sure your antivirus didn't remove files from it.",
        (WebViewMissing, Ru) => "Clade не смог запустить встроенный веб-компонент.\n\nОбычно Clade содержит собственный WebView2 в папке runtimes\\webview2 рядом с Clade.exe, но его там нет, и WebView2 не установлен на этом компьютере.\n\nРаспакуй Clade-Launcher-Portable.zip целиком заново (не копируй один Clade.exe) и проверь, что антивирус ничего не удалил.",
        (WindowFailed, En) => "Clade couldn't open its window.\n\nTry starting it again. If it keeps happening, the WebView2 files may be damaged.",
        (WindowFailed, Ru) => "Clade не удалось открыть окно.\n\nПопробуй запустить его снова. Если ошибка повторяется, файлы WebView2 могли повредиться.",
    }
}

/// The label that introduces the technical detail line.
fn detail_label(lang: Language) -> &'static str {
    match lang {
        Language::En => "Details",
        Language::Ru => "Подробности",
    }
}

/// The full dialog body: the localized text and, when there is one, the technical detail kept in
/// English (it is what helps troubleshooting). Details must never carry secrets or user paths.
pub fn render(id: StartupMessage, lang: Language, detail: Option<&str>) -> String {
    let mut s = text(id, lang).to_owned();
    if let Some(d) = detail.map(str::trim).filter(|d| !d.is_empty()) {
        s.push_str(&format!("\n\n{}: {d}", detail_label(lang)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use Language::{En, Ru};

    fn settings(language: &str) -> Vec<u8> {
        format!(r#"{{"ui":{{"language":"{language}"}}}}"#).into_bytes()
    }

    #[test]
    fn an_explicit_language_in_settings_is_used() {
        assert_eq!(resolve_language(Some(&settings("en")), Some("ru-RU")), En);
        assert_eq!(resolve_language(Some(&settings("ru")), Some("en-US")), Ru);
    }

    #[test]
    fn without_a_choice_the_windows_language_decides() {
        let none = br#"{"ui":{"sound":{"enabled":true}}}"#;
        assert_eq!(resolve_language(Some(none), Some("ru-RU")), Ru);
        assert_eq!(resolve_language(None, Some("ru")), Ru);
        assert_eq!(resolve_language(None, Some("RU_ru")), Ru);
        assert_eq!(resolve_language(None, Some("en-US")), En);
        assert_eq!(resolve_language(None, Some("de-DE")), En);
        assert_eq!(resolve_language(None, Some("uk-UA")), En);
        assert_eq!(resolve_language(None, Some("rue")), En);
        assert_eq!(resolve_language(None, None), En);
        assert_eq!(resolve_language(None, Some("")), En);
    }

    #[test]
    fn missing_malformed_or_invalid_settings_fall_back_safely() {
        for bad in [
            &b""[..],
            b"not json at all",
            b"{\"ui\":",
            b"[1,2,3]",
            b"null",
            br#"{"ui":{"language":"de"}}"#,
            br#"{"ui":{"language":""}}"#,
            br#"{"ui":{"language":5}}"#,
            br#"{"ui":{"language":null}}"#,
            br#"{"ui":"ru"}"#,
            br#"{"ui":{"language":"RU"}}"#,
            &[0xff, 0xfe, 0x00],
        ] {
            assert_eq!(resolve_language(Some(bad), Some("ru-RU")), Ru, "{bad:?}");
            assert_eq!(resolve_language(Some(bad), Some("en-US")), En, "{bad:?}");
        }
    }

    #[test]
    fn reading_the_real_file_is_tolerant() {
        let d = tempfile::tempdir().unwrap();
        // A folder with no data/settings.json at all, and an unknown root, both just work.
        let _ = current_language(Some(d.path()));
        let _ = current_language(None);
        std::fs::create_dir_all(d.path().join("data")).unwrap();
        let file = d.path().join("data").join("settings.json");
        std::fs::write(&file, settings("ru")).unwrap();
        assert_eq!(current_language(Some(d.path())), Ru);
        std::fs::write(&file, settings("en")).unwrap();
        assert_eq!(current_language(Some(d.path())), En);
        std::fs::write(&file, b"{ broken").unwrap();
        let _ = current_language(Some(d.path())); // falls back to Windows, no panic
                                                  // An oversized file (> 64 KiB) is rejected by read_bounded without unbounded allocation.
        let mut oversized = br#"{"ui":{"language":"ru","pad":""#.to_vec();
        oversized.extend(std::iter::repeat_n(b'x', 70 * 1024));
        oversized.extend_from_slice(br#""}}"#);
        std::fs::write(&file, oversized).unwrap();
        let _ = current_language(Some(d.path()));
        // A directory where the file should be is unreadable, not fatal.
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let _ = current_language(Some(d.path()));
    }

    #[test]
    fn every_startup_message_has_both_languages_and_keeps_its_technical_names() {
        for id in ALL_MESSAGES {
            let en = text(id, En);
            let ru = text(id, Ru);
            assert!(en.len() > 20 && ru.len() > 20, "{id:?}");
            assert_ne!(en, ru, "{id:?} must not be English in both");
            assert!(ru.chars().any(|c| ('а'..='я').contains(&c)), "{id:?}");
            assert!(
                !en.chars().any(|c| ('а'..='я').contains(&c)),
                "{id:?} English text contains Cyrillic"
            );
            // Dialogs stay short.
            assert!(en.len() < 520 && ru.len() < 760, "{id:?}");
            // Brand and product names are never translated.
            assert!(en.contains("Clade") == ru.contains("Clade"), "{id:?}");
            assert_eq!(en.contains("WebView2"), ru.contains("WebView2"), "{id:?}");
            assert_eq!(en.contains("Clade.exe"), ru.contains("Clade.exe"), "{id:?}");
        }
    }

    #[test]
    fn details_stay_english_after_a_localized_label() {
        let s = render(
            StartupMessage::StartupFailed,
            Ru,
            Some("settings io error: boom"),
        );
        assert!(s.starts_with("Clade не удалось запустить."));
        assert!(s.ends_with("Подробности: settings io error: boom"));
        assert_eq!(
            render(StartupMessage::StartupFailed, En, Some("  ")),
            text(StartupMessage::StartupFailed, En)
        );
        assert!(render(StartupMessage::FolderInUse, En, Some("x")).contains("Details: x"));
    }

    /// Guard: no native dialog may carry its own text. Everything goes through `StartupMessage`, so
    /// both languages exist by construction; this scans the startup code for a raw string given to
    /// `fatal(...)` (the only native message box).
    #[test]
    fn the_only_way_to_show_a_native_dialog_is_a_startup_message() {
        let src = include_str!("lib.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        for (n, line) in prod.lines().enumerate() {
            if line.contains("startup::fatal(") || line.contains("MessageBox") {
                panic!(
                    "lib.rs:{} calls a native dialog with raw text: {line}",
                    n + 1
                );
            }
        }
        let startup = include_str!("startup.rs");
        let prod = startup.split("#[cfg(test)]").next().unwrap();
        assert_eq!(
            prod.matches("MessageBoxW(").count(),
            1,
            "exactly one native message box, behind `fail`"
        );
        assert!(prod.contains("pub fn fail(id: StartupMessage"));
        assert!(
            !prod.contains("pub fn fatal("),
            "the raw-text entry point is gone"
        );
    }
}
