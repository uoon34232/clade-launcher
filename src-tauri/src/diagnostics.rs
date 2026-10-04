//! Diagnostics export: a small ZIP a person can attach to a bug report.
//!
//! The archive is built from a fixed allowlist of entries (see [`ENTRIES`]) and nothing else: no
//! arbitrary file is ever read on request, and the vault, account files, tokens, saves, mods and
//! Minecraft files are never candidates. Text that does come from a log is redacted before it is
//! written: the Clade folder and the Windows user folder are replaced by placeholders, e-mail
//! addresses and anything that looks like a token or a password are removed.
//!
//! The ZIP is written inside the Clade folder (`logs/diagnostics/`), so it stays portable.

use std::fs;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::coded_error::CodedError;
use crate::instances::{InstanceStore, Profile, SetupState};
use crate::portable::fsutil::atomic_write;
use crate::portable::PortableRoot;
use crate::settings::LauncherSettings;

/// The only entries a diagnostics ZIP can contain.
pub const ENTRIES: [&str; 6] = [
    "launcher-version.txt",
    "system-info.txt",
    "profile-summary.txt",
    "settings-redacted.json",
    "logs/clade.log",
    "logs/minecraft-latest.log",
];

/// How much of the end of a log is kept.
const LOG_TAIL_BYTES: u64 = 512 * 1024;
/// How many exports are kept in `logs/diagnostics/`.
const KEEP: usize = 5;

/// Facts the export needs from the running application.
pub struct Facts<'a> {
    pub version: &'a str,
    pub layout_version: u32,
    pub webview: &'a str,
    pub webview_version: &'a str,
    pub system_locale: Option<&'a str>,
    pub total_memory_mb: Option<u64>,
    pub free_disk_bytes: Option<u64>,
    pub now_unix: i64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Exported {
    /// File name only, e.g. `Clade-Diagnostics-20261004-153000.zip`.
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DiagError {
    #[error("could not write the diagnostics file: {0}")]
    Io(#[from] io::Error),
    #[error("could not build the diagnostics archive: {0}")]
    Zip(#[from] zip::result::ZipError),
}

impl From<DiagError> for CodedError {
    fn from(e: DiagError) -> Self {
        CodedError::new("diagnostics_failed", e.to_string())
    }
}

// ----------------------------------------------------------------------------------- redaction

/// Words whose value (after `=`, `:` or a space) is a secret.
const SECRET_KEYS: [&str; 14] = [
    "access_token",
    "accesstoken",
    "refresh_token",
    "id_token",
    "client_secret",
    "authorization",
    "password",
    "passphrase",
    "session",
    "xuid",
    "usertype_token",
    "code",
    "token",
    "bearer",
];

fn is_value_char(c: char) -> bool {
    !(c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '&' | ';' | '}' | ')' | '<' | '>'))
}

/// Replaces the value that follows each secret-looking key.
fn redact_key_values(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    'outer: while i < line.len() {
        for key in SECRET_KEYS {
            if lower[i..].starts_with(key) {
                // a whole word only (`barcode` is not `code`)
                let before_ok = i == 0
                    || !lower.as_bytes()[i - 1].is_ascii_alphanumeric()
                        && lower.as_bytes()[i - 1] != b'_';
                let mut j = i + key.len();
                let after = &line[j..];
                let sep = after
                    .chars()
                    .take_while(|c| matches!(c, ' ' | '=' | ':' | '"' | '\''))
                    .count();
                let has_sep = after[..sep].contains(['=', ':']) || (key == "bearer" && sep > 0);
                if before_ok && has_sep {
                    j += sep;
                    // An Authorization header carries `Bearer <token>`: the whole rest of the line.
                    let value_len: usize = if key == "authorization" {
                        line[j..].len()
                    } else {
                        line[j..]
                            .chars()
                            .take_while(|c| is_value_char(*c))
                            .map(char::len_utf8)
                            .sum()
                    };
                    if value_len > 0 {
                        out.push_str(&line[i..j]);
                        out.push_str("<redacted>");
                        i = j + value_len;
                        continue 'outer;
                    }
                }
            }
        }
        let ch = line[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Removes the value after `--accessToken` / `--session` style launch arguments, and bare tokens
/// that are recognisable by shape (a JWT, a Microsoft refresh token).
fn redact_tokens(line: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut hide_next = false;
    for word in line.split(' ') {
        let w = word.trim_matches(|c| matches!(c, '"' | '\''));
        let lw = w.to_ascii_lowercase();
        if hide_next && !w.is_empty() {
            out.push("<redacted>".to_owned());
            hide_next = false;
            continue;
        }
        if matches!(
            lw.as_str(),
            "--accesstoken" | "--session" | "--uuidtoken" | "--xuid" | "--clientid"
        ) {
            hide_next = true;
            out.push(word.to_owned());
            continue;
        }
        let jwt = w.starts_with("eyJ") && w.len() >= 20;
        let ms_refresh = (w.starts_with("M.") || w.starts_with("0.A")) && w.len() >= 40;
        out.push(if jwt || ms_refresh {
            "<redacted>".to_owned()
        } else {
            word.to_owned()
        });
    }
    out.join(" ")
}

/// Replaces e-mail addresses.
fn redact_emails(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('@') {
        let local_start = rest[..at]
            .rfind(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')))
            .map_or(0, |p| p + 1);
        let domain_len: usize = rest[at + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '.' | '-'))
            .map(char::len_utf8)
            .sum();
        let domain = &rest[at + 1..at + 1 + domain_len];
        if local_start < at && domain.contains('.') {
            out.push_str(&rest[..local_start]);
            out.push_str("<email>");
            rest = &rest[at + 1 + domain_len..];
        } else {
            out.push_str(&rest[..=at]);
            rest = &rest[at + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// Replaces `C:\Users\<name>` (either slash direction) with `C:\Users\<user>`.
fn redact_user_folder(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let lower = text.to_ascii_lowercase();
    while i < text.len() {
        let rest = &lower[i..];
        let hit = ["\\users\\", "/users/"]
            .iter()
            .find(|p| rest.starts_with(**p));
        if let Some(p) = hit {
            let start = i + p.len();
            let name_len: usize = text[start..]
                .chars()
                .take_while(|c| !matches!(c, '\\' | '/' | '"' | '\'' | '\r' | '\n'))
                .map(char::len_utf8)
                .sum();
            out.push_str(&text[i..start]);
            out.push_str("<user>");
            i = start + name_len;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Everything a log line may be cleaned of before it leaves the machine.
pub fn redact(root: &Path, text: &str) -> String {
    let mut out = text.to_owned();
    // The Clade folder (also in the `\\?\` form and with forward slashes) becomes a placeholder.
    let plain = root.to_string_lossy().replace("\\\\?\\", "");
    for variant in [plain.clone(), plain.replace('\\', "/")] {
        if variant.len() > 3 {
            out = replace_ignore_case(&out, &variant, "<clade>");
        }
    }
    out = redact_user_folder(&out);
    out.lines()
        .map(|l| redact_emails(&redact_tokens(&redact_key_values(l))))
        .collect::<Vec<_>>()
        .join("\n")
}

fn replace_ignore_case(text: &str, needle: &str, with: &str) -> String {
    let (lt, ln) = (text.to_ascii_lowercase(), needle.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lt[i..].find(&ln) {
        out.push_str(&text[i..i + pos]);
        out.push_str(with);
        i += pos + ln.len();
    }
    out.push_str(&text[i..]);
    out
}

// ------------------------------------------------------------------------------------- content

/// The last `LOG_TAIL_BYTES` of a text file, starting at a line boundary. A missing or unreadable
/// file is simply absent from the export.
fn tail(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let cut = len > LOG_TAIL_BYTES;
    if cut {
        f.seek(SeekFrom::Start(len - LOG_TAIL_BYTES)).ok()?;
    }
    let mut bytes = Vec::new();
    f.take(LOG_TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if cut {
        text.split_once('\n')
            .map_or(text.clone(), |(_, r)| r.to_owned())
    } else {
        text
    })
}

/// Minecraft's own latest log: of the profile played last, else of the most recently written one.
fn minecraft_latest_log(root: &PortableRoot, settings: &LauncherSettings) -> Option<PathBuf> {
    let latest = |id: &str| {
        let p = root
            .instances_dir()
            .join(id)
            .join("game")
            .join("logs")
            .join("latest.log");
        p.is_file().then_some(p)
    };
    if let Some(p) = settings
        .last_instance
        .as_ref()
        .and_then(|i| latest(i.as_str()))
    {
        return Some(p);
    }
    let listing = InstanceStore::new(root).list().ok()?;
    listing
        .instances
        .iter()
        .filter_map(|i| latest(i.id.as_str()))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p)
}

fn profile_summary(root: &PortableRoot) -> String {
    let Ok(listing) = InstanceStore::new(root).list() else {
        return "Profiles could not be read.\n".to_owned();
    };
    let mut out = format!("Profiles: {}\n", listing.instances.len());
    for i in &listing.instances {
        let kind = match &i.profile {
            Profile::Vanilla => "vanilla".to_owned(),
            Profile::Modded { loader, .. } => format!("modded ({loader:?})").to_lowercase(),
        };
        let state = match &i.setup {
            SetupState::Pending => "not set up".to_owned(),
            SetupState::Ready { .. } => "ready".to_owned(),
        };
        out.push_str(&format!(
            "- {} | Minecraft {} | {kind} | {state} | memory override: {} | extra JVM arguments: {}\n",
            i.id.as_str(),
            i.minecraft.as_str(),
            i.memory_mb.map_or("none".to_owned(), |m| format!("{m} MB")),
            i.extra_jvm_args.len()
        ));
    }
    if !listing.problems.is_empty() {
        out.push_str(&format!(
            "Profiles that could not be loaded: {}\n",
            listing.problems.len()
        ));
    }
    out
}

/// The UTC date and time as `YYYYMMDD-HHMMSS`.
pub fn timestamp(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// The files of the export as (name inside the ZIP, text). Only [`ENTRIES`] can appear.
pub fn collect(
    root: &PortableRoot,
    settings: &LauncherSettings,
    facts: &Facts<'_>,
) -> Vec<(&'static str, String)> {
    let mut files: Vec<(&'static str, String)> = vec![
        (
            "launcher-version.txt",
            format!(
                "Clade Launcher {}\nFolder layout: {}\nExported (UTC): {}\n",
                facts.version,
                facts.layout_version,
                timestamp(facts.now_unix)
            ),
        ),
        (
            "system-info.txt",
            format!(
                "OS: {} {}\nWindows display language: {}\nMemory: {}\nFree space on the Clade drive: {}\nWebView2: {} {}\n",
                std::env::consts::OS,
                std::env::consts::ARCH,
                facts.system_locale.unwrap_or("unknown"),
                facts
                    .total_memory_mb
                    .map_or("unknown".to_owned(), |m| format!("{m} MB")),
                facts.free_disk_bytes.map_or("unknown".to_owned(), |b| format!(
                    "{} MB",
                    b / (1024 * 1024)
                )),
                facts.webview,
                facts.webview_version
            ),
        ),
        ("profile-summary.txt", profile_summary(root)),
        (
            "settings-redacted.json",
            // The settings file holds preferences only (no secrets, no paths); it is still passed
            // through the redactor.
            redact(
                root.path(),
                &serde_json::to_string_pretty(settings).unwrap_or_default(),
            ),
        ),
    ];
    if let Some(t) = tail(&root.logs_dir().join("clade.log")) {
        files.push(("logs/clade.log", redact(root.path(), &t)));
    }
    if let Some(p) = minecraft_latest_log(root, settings) {
        if let Some(t) = tail(&p) {
            files.push(("logs/minecraft-latest.log", redact(root.path(), &t)));
        }
    }
    files
}

fn zip_bytes(files: &[(&str, String)]) -> Result<Vec<u8>, DiagError> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, text) in files {
        debug_assert!(ENTRIES.contains(name), "{name} is not an allowed entry");
        zip.start_file(*name, opts)?;
        zip.write_all(text.as_bytes())?;
    }
    Ok(zip.finish()?.into_inner())
}

/// Builds the ZIP and saves it as `logs/diagnostics/Clade-Diagnostics-<UTC time>.zip`.
pub fn export(
    root: &PortableRoot,
    settings: &LauncherSettings,
    facts: &Facts<'_>,
) -> Result<(Exported, PathBuf), DiagError> {
    let bytes = zip_bytes(&collect(root, settings, facts))?;
    let dir = root.logs_dir().join("diagnostics");
    let name = format!("Clade-Diagnostics-{}.zip", timestamp(facts.now_unix));
    let path = dir.join(&name);
    atomic_write(&path, &bytes)?;
    prune(&dir);
    Ok((Exported { name }, path))
}

/// Keeps the newest few exports.
fn prune(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut zips: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "zip")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("Clade-Diagnostics-"))
        })
        .collect();
    zips.sort();
    while zips.len() > KEEP {
        let _ = fs::remove_file(zips.remove(0));
    }
}

#[cfg(test)]
mod tests;
