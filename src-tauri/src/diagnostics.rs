//! Diagnostics export: a small ZIP a person can attach to a bug report.
//!
//! The archive is built from a fixed allowlist of entries (see [`ENTRIES`]) and nothing else: no
//! arbitrary file is ever read on request, and the vault, account files, tokens, saves, mods and
//! Minecraft files are never candidates. Text that does come from a log is redacted before it is
//! written: the Clade folder and the Windows user folder are replaced by placeholders, e-mail
//! addresses and anything that looks like a token or a password are removed, and the Minecraft
//! profile names and UUIDs of the accounts Clade knows about (read from the metadata-only
//! `accounts.json`, never the vault) are replaced by placeholders. Local game logs are not touched.
//!
//! The ZIP is written inside the Clade folder (`logs/diagnostics/`), so it stays portable.

use std::fs;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::accounts::AccountsFile;
use crate::coded_error::CodedError;
use crate::instances::{InstanceStore, Profile, SetupState};
use crate::portable::fsutil::{atomic_write, is_link, reject_links_between};
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
const SECRET_KEYS: [&str; 21] = [
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
    // Microsoft / Xbox / Minecraft sign-in vocabulary and common secret names
    "signedjwt",
    "jwt",
    "rpsticket",
    "xbl3.0",
    "secret",
    "apikey",
    "x-api-key",
];

/// Keys whose value follows after spaces only (`Bearer <token>`, `XBL3.0 x=<hash>;<token>`).
const SPACE_KEYS: [&str; 3] = ["bearer", "xbl3.0", "rpsticket"];

/// `code` is both an OAuth authorization code (a secret) and the launcher's own harmless result codes
/// (`code=vault_locked`, `code=Some(-1`). Only values that are plainly one of the harmless shapes are
/// kept readable: a small number, or lower-case words joined by underscores.
fn is_harmless_code(value: &str) -> bool {
    let v = value.strip_prefix("Some(").unwrap_or(value);
    if v.is_empty() || v.len() > 40 {
        return false;
    }
    let number = v.strip_prefix('-').unwrap_or(v);
    if number.len() <= 9 && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    v.split('_')
        .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()))
}

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
                let has_sep =
                    after[..sep].contains(['=', ':']) || (SPACE_KEYS.contains(&key) && sep > 0);
                if before_ok && has_sep {
                    j += sep;
                    // An Authorization header carries `Bearer <token>`: the whole rest of the line.
                    let value_len: usize = if key == "authorization" {
                        line[j..].len()
                    } else if key == "xbl3.0" {
                        line[j..]
                            .chars()
                            .take_while(|c| is_value_char(*c) || *c == ';')
                            .map(char::len_utf8)
                            .sum()
                    } else {
                        line[j..]
                            .chars()
                            .take_while(|c| is_value_char(*c))
                            .map(char::len_utf8)
                            .sum()
                    };
                    let harmless = key == "code" && is_harmless_code(&line[j..j + value_len]);
                    if value_len > 0 && !harmless {
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

/// Whether a name (`api-key`, `X_Secret_Key`, `dbPassword`, ...) is the name of a secret. Judged by its
/// shape, not by a list of spellings: separators and case are ignored and the name has to END in a
/// secret word, so `secretary`, `hotkey`, `primary_key` and `token_count` are not secrets.
fn is_secret_name(name: &str) -> bool {
    const ENDINGS: [&str; 11] = [
        "apikey",
        "secretkey",
        "privatekey",
        "accesskey",
        "authkey",
        "secret",
        "password",
        "passwd",
        "passphrase",
        "token",
        "credentials",
    ];
    let normalized: String = name
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | '.'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    ENDINGS.iter().any(|e| normalized.ends_with(e))
}

/// Replaces the value of any `name=value`, `name: value` or `"name":"value"` whose name is a secret name.
fn redact_named_secrets(line: &str) -> String {
    let is_name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        let starts_name = line[i..].chars().next().is_some_and(is_name_char)
            && (i == 0 || !line[..i].chars().next_back().is_some_and(is_name_char));
        if starts_name {
            let name_len: usize = line[i..]
                .chars()
                .take_while(|c| is_name_char(*c))
                .map(char::len_utf8)
                .sum();
            let after = &line[i + name_len..];
            let sep_len = after
                .chars()
                .take_while(|c| matches!(c, ' ' | '=' | ':' | '"' | '\''))
                .map(char::len_utf8)
                .sum::<usize>();
            let has_sep = after[..sep_len].contains(['=', ':']);
            let value_start = i + name_len + sep_len;
            let value_len: usize = line[value_start..]
                .chars()
                .take_while(|c| is_value_char(*c))
                .map(char::len_utf8)
                .sum();
            if has_sep && value_len > 0 && is_secret_name(&line[i..i + name_len]) {
                out.push_str(&line[i..value_start]);
                out.push_str("<redacted>");
                i = value_start + value_len;
                continue;
            }
            // not a secret name: copy the name through unchanged
            out.push_str(&line[i..i + name_len]);
            i += name_len;
            continue;
        }
        let ch = line[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn is_msa_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '/' | '=' | '!' | '*' | '$')
}

/// The length of the credential at the start of `rest`, if what is there is recognisable by shape:
/// a JWT (`eyJ...`, or three long base64url segments), a 5-segment JWE, a Microsoft access token
/// (`Ew...`), a Microsoft refresh token (`M.` / `0.A`), or the development fixture token. Anything
/// else is not a secret.
fn secret_shape_len(rest: &str) -> Option<usize> {
    if rest.starts_with("Ew") || rest.starts_with("M.") || rest.starts_with("0.A") {
        let msa_run: usize = rest
            .chars()
            .take_while(|c| is_msa_token_char(*c))
            .map(char::len_utf8)
            .sum();
        if (rest.starts_with("Ew") && msa_run >= 80)
            || ((rest.starts_with("M.") || rest.starts_with("0.A")) && msa_run >= 32)
        {
            return Some(msa_run);
        }
    }
    let run: usize = rest
        .chars()
        .take_while(|c| is_token_char(*c))
        .map(char::len_utf8)
        .sum();
    let w = &rest[..run];
    let jwt = w.starts_with("eyJ") && run >= 20;
    let parts: Vec<&str> = w.trim_end_matches('.').split('.').collect();
    let three_segments = parts.len() == 3 && parts.iter().all(|p| p.len() >= 16);
    let five_segments = parts.len() == 5
        && run >= 64
        && parts[0].len() >= 12
        && parts[2].len() >= 12
        && parts[3].len() >= 16
        && parts[4].len() >= 16;
    let fixture = w.len() >= 12 && w[..12].eq_ignore_ascii_case("DEV-FIXTURE-");
    (jwt || three_segments || five_segments || fixture).then_some(run)
}

/// Replaces credentials that are recognisable by shape wherever they stand in the line (also after
/// `=`, `:`, `;` or inside brackets), not only after a secret-looking key.
fn redact_shapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        let at_word_start = i == 0
            || !line[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if at_word_start {
            if let Some(len) = secret_shape_len(&line[i..]) {
                out.push_str("<redacted>");
                i += len;
                continue;
            }
        }
        let ch = line[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Removes the value after `--accessToken` / `--session` style launch arguments, and bare tokens
/// that are recognisable by shape.
fn redact_tokens(line: &str) -> String {
    let line = redact_shapes(line);
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
        out.push(word.to_owned());
    }
    out.join(" ")
}

/// Replaces e-mail addresses.
fn redact_emails(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('@') {
        let local_start = rest[..at]
            .char_indices()
            .rev()
            .find(|&(_, c)| !(c.is_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')))
            .map_or(0, |(p, c)| p + c.len_utf8());
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

/// Replaces the user profile folder segment (either slash direction or JSON-escaped) with `<user>`.
fn redact_user_folder(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let lower = text.to_ascii_lowercase();
    while i < text.len() {
        let rest = &lower[i..];
        if let Some(prefix) = ["\\users", "/users"].iter().find(|p| rest.starts_with(**p)) {
            let after_prefix = i + prefix.len();
            let sep_len: usize = text[after_prefix..]
                .chars()
                .take_while(|c| matches!(*c, '\\' | '/'))
                .map(char::len_utf8)
                .sum();
            if sep_len > 0 {
                let start = after_prefix + sep_len;
                let name_len: usize = text[start..]
                    .chars()
                    .take_while(|c| !matches!(*c, '\\' | '/' | '"' | '\'' | '\r' | '\n'))
                    .map(char::len_utf8)
                    .sum();
                if name_len > 0 {
                    out.push_str(&text[i..start]);
                    out.push_str("<user>");
                    i = start + name_len;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Everything a log line may be cleaned of before it leaves the machine.
pub fn redact(root: &Path, text: &str) -> String {
    let trailing_newline = text.ends_with('\n');
    let mut out = text.to_owned();
    // The Clade folder (also verbatim, JSON-escaped, forward-slash and file:// forms) becomes a placeholder.
    let plain = crate::portable::root::normalize_verbatim_path(root.to_path_buf())
        .to_string_lossy()
        .replace("\\\\?\\", "");
    let plain = plain.trim_end_matches(['\\', '/']).to_owned();
    if plain.len() > 3 {
        let slash = plain.replace('\\', "/");
        let mut variants: Vec<(String, &'static str)> = vec![
            (plain.replace('\\', "\\\\\\\\"), "<clade>"),
            (plain.replace('\\', "\\\\"), "<clade>"),
            (slash.replace(' ', "%20"), "<clade>"),
            (plain.clone(), "<clade>"),
            (slash, "<clade>"),
        ];
        if let Ok(u) = url::Url::from_file_path(&plain) {
            let full = u.as_str().trim_end_matches('/').to_owned();
            variants.push((full, "file:///<clade>"));
            let p = u.path().trim_end_matches('/');
            let p = match p.as_bytes() {
                [b'/', d, b':', ..] if d.is_ascii_alphabetic() => &p[1..],
                _ => p,
            };
            variants.push((p.to_owned(), "<clade>"));
        }
        variants.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
        variants.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
        for (variant, placeholder) in variants {
            if variant.len() > 3 {
                out = replace_ignore_case(&out, &variant, placeholder);
            }
        }
    }
    out = redact_user_folder(&out);
    let mut redacted = out
        .lines()
        .map(|l| redact_emails(&redact_tokens(&redact_named_secrets(&redact_key_values(l)))))
        .collect::<Vec<_>>()
        .join("\n");
    if trailing_newline {
        redacted.push('\n');
    }
    redacted
}

/// The Minecraft identities Clade knows about, taken from `accounts.json` (metadata only: no token
/// is stored there). A missing or unreadable file simply means nothing extra is redacted.
fn known_identities(root: &PortableRoot) -> Vec<String> {
    let Ok(file) = AccountsFile::load(&root.accounts_file()) else {
        return Vec::new();
    };
    file.accounts
        .iter()
        .flat_map(|a| [a.username.clone(), a.minecraft_uuid.clone()])
        .filter(|v| !v.trim().is_empty())
        .collect()
}

/// Replaces each known profile name and UUID (dashed or not) with a placeholder. Only the exact
/// known values are touched, a name only as a whole word, so ordinary words are never redacted.
pub fn redact_identities(text: &str, known: &[String]) -> String {
    let mut out = text.to_owned();
    for value in known {
        let looks_like_uuid =
            value.len() >= 32 && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if looks_like_uuid {
            let plain = value.replace('-', "");
            for v in [value.as_str(), plain.as_str()] {
                out = replace_ignore_case(&out, v, "<mc-uuid>");
            }
        } else {
            out = replace_word_ignore_case(&out, value, "<mc-name>");
        }
    }
    out
}

fn replace_word_ignore_case(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let (lt, ln) = if needle.is_ascii() {
        (text.to_ascii_lowercase(), needle.to_ascii_lowercase())
    } else {
        let (lt, ln) = (text.to_lowercase(), needle.to_lowercase());
        // Lower-casing can change byte lengths for some non-ASCII scripts; fall back safely then.
        if lt.len() != text.len() || ln.is_empty() {
            return text.to_owned();
        }
        (lt, ln)
    };
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lt[i..].find(&ln) {
        let (a, b) = (i + pos, i + pos + ln.len());
        if !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            return text.to_owned();
        }
        let before_ok = !text[..a].chars().next_back().is_some_and(word);
        let after_ok = !text[b..].chars().next().is_some_and(word);
        out.push_str(&text[i..a]);
        out.push_str(if before_ok && after_ok {
            with
        } else {
            &text[a..b]
        });
        i = b;
    }
    out.push_str(&text[i..]);
    out
}

fn replace_ignore_case(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
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

/// The last `max_bytes` of a text file inside `root`, starting at a line boundary. A missing,
/// unreadable, or symlink/junction-redirected file is simply absent from the export.
fn tail_bytes_in(root: &Path, path: &Path, max_bytes: u64) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }
    reject_links_between(root, path).ok()?;
    if is_link(path).unwrap_or(true) {
        return None;
    }
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let cut = len > max_bytes;
    if cut {
        f.seek(SeekFrom::Start(len - max_bytes)).ok()?;
    }
    let mut bytes = Vec::new();
    f.take(max_bytes).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if cut {
        match text.find('\n') {
            Some(pos) => text[pos + 1..].to_owned(),
            None => text,
        }
    } else {
        text
    })
}

/// The last `LOG_TAIL_BYTES` of a text file, starting at a line boundary. A missing or unreadable
/// file is simply absent from the export.
#[cfg(test)]
fn tail(path: &Path) -> Option<String> {
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(path);
    tail_bytes_in(base, path, LOG_TAIL_BYTES)
}

/// Reads `logs/clade.log`, prepending the tail of `logs/clade.old.log` when `clade.log` is smaller
/// than [`LOG_TAIL_BYTES`] so a fresh rotation does not discard recent history.
fn clade_log_tail(root: &PortableRoot) -> Option<String> {
    let logs_dir = root.logs_dir();
    let cur_path = logs_dir.join("clade.log");
    let cur = tail_bytes_in(root.path(), &cur_path, LOG_TAIL_BYTES);
    let cur_meta_len = if cur.is_some() {
        fs::symlink_metadata(&cur_path).ok().map(|m| m.len())
    } else {
        None
    };
    if cur_meta_len.is_some_and(|len| len >= LOG_TAIL_BYTES) {
        return cur;
    }
    let cur_len = cur.as_ref().map_or(0, |s| s.len() as u64);
    let rem = LOG_TAIL_BYTES.saturating_sub(cur_len);
    let old_path = logs_dir.join("clade.old.log");
    let old = tail_bytes_in(root.path(), &old_path, rem).filter(|s| !s.is_empty());
    match (old, cur) {
        (Some(mut o), Some(c)) => {
            if !c.is_empty() {
                if !o.ends_with('\n') {
                    o.push('\n');
                }
                o.push_str(&c);
            }
            Some(o)
        }
        (Some(o), None) => Some(o),
        (None, c) => c,
    }
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
        reject_links_between(root.path(), &p).ok()?;
        if is_link(&p).unwrap_or(true) {
            return None;
        }
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
        .filter_map(|p| Some((fs::symlink_metadata(&p).ok()?.modified().ok()?, p)))
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
        // Their files are left exactly as they are; only the folder name and the reason are listed.
        for (folder, reason) in &listing.problems {
            out.push_str(&format!(
                "- {folder}: {}\n",
                reason.lines().next().unwrap_or("")
            ));
        }
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
    let known = known_identities(root);
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
        (
            "profile-summary.txt",
            redact_identities(&redact(root.path(), &profile_summary(root)), &known),
        ),
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
    if let Some(t) = clade_log_tail(root) {
        files.push((
            "logs/clade.log",
            redact_identities(&redact(root.path(), &t), &known),
        ));
    }
    if let Some(p) = minecraft_latest_log(root, settings) {
        if let Some(t) = tail_bytes_in(root.path(), &p, LOG_TAIL_BYTES) {
            files.push((
                "logs/minecraft-latest.log",
                redact_identities(&redact(root.path(), &t), &known),
            ));
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
    reject_links_between(root.path(), &dir)?;
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
