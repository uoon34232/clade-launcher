//! Startup sequencing and fatal-error reporting.
//!
//! The release binary has no console (`windows_subsystem = "windows"`), so a failed start
//! would be invisible. Fatal problems are shown in a native message box instead.

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Mutex;

use crate::portable::PortableRoot;
use crate::startup_text::{self, StartupMessage};

/// The one native error dialog, shown in the selected language (see `startup_text`), then exit.
/// `root` is the launcher folder when it is already known (for its saved language); `detail` is
/// an English technical line for troubleshooting and is redacted so user or share paths never leak.
pub fn fail(id: StartupMessage, root: Option<&Path>, detail: Option<&str>) -> ! {
    let lang = startup_text::current_language(root);
    let safe_detail = detail.map(|d| redact_detail(d, root));
    show_error(
        startup_text::TITLE,
        &startup_text::render(id, lang, safe_detail.as_deref()),
    );
    std::process::exit(1)
}

fn strip_verbatim_prefixes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < text.len() {
        if text[i..]
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("\\\\?\\UNC\\"))
        {
            out.push_str("\\\\");
            i += 8;
            continue;
        }
        if text[i..].starts_with("\\\\?\\")
            && i + 6 < bytes.len()
            && bytes[i + 4].is_ascii_alphabetic()
            && bytes[i + 5] == b':'
            && (bytes[i + 6] == b'\\' || bytes[i + 6] == b'/')
        {
            i += 4;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_ascii_ignore_case(text: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let lt = text.to_ascii_lowercase();
    let ln = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lt[i..].find(&ln) {
        out.push_str(&text[i..i + pos]);
        out.push_str(replacement);
        i += pos + ln.len();
    }
    out.push_str(&text[i..]);
    out
}

fn is_unquoted_path_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ';' | ',' | ':' | '|'
        )
}

fn trim_trailing_dots(slice: &str, min_len: usize) -> usize {
    let mut end = slice.len();
    while end > min_len {
        let Some(ch) = slice[..end].chars().next_back() else {
            break;
        };
        if ch == '.' {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn consume_path_span(text: &str, i: usize, prefix_len: usize, prev: Option<char>) -> usize {
    if prev == Some('"') {
        if let Some(p) = text[i + prefix_len..]
            .find(['"', '\r', '\n'])
            .filter(|&p| text.as_bytes()[i + prefix_len + p] == b'"')
        {
            return prefix_len + p;
        }
    }
    let raw_len: usize = text[i + prefix_len..]
        .chars()
        .take_while(|c| !is_unquoted_path_delimiter(*c))
        .map(char::len_utf8)
        .sum();
    trim_trailing_dots(&text[i..i + prefix_len + raw_len], prefix_len)
}

/// Replaces the launcher root with `<clade>`, drive/home paths with `<path>`, and UNC paths
/// with `<unc-path>` before displaying technical startup error details.
pub fn redact_detail(detail: &str, root: Option<&Path>) -> String {
    let mut text = strip_verbatim_prefixes(detail);
    if let Some(r) = root {
        let root_clean = strip_verbatim_prefixes(&r.to_string_lossy());
        let trimmed = root_clean.trim_end_matches(['\\', '/']);
        if trimmed.chars().count() > 3 {
            let backslash = trimmed.replace('/', "\\");
            let slash = trimmed.replace('\\', "/");
            text = replace_ascii_ignore_case(&text, &backslash, "<clade>");
            if slash != backslash {
                text = replace_ascii_ignore_case(&text, &slash, "<clade>");
            }
        }
    }

    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let prev = text[..i].chars().next_back();
        let boundary =
            prev.is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '\\' | '/')));

        if boundary
            && i + 2 < bytes.len()
            && bytes[i] == b'\\'
            && bytes[i + 1] == b'\\'
            && bytes[i + 2] != b'\\'
            && text[i + 2..]
                .chars()
                .next()
                .is_some_and(|c| !is_unquoted_path_delimiter(c))
        {
            let span = consume_path_span(&text, i, 2, prev);
            out.push_str("<unc-path>");
            i += span;
            continue;
        }

        if boundary
            && i + 2 < bytes.len()
            && bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && (bytes[i + 2] == b'\\' || bytes[i + 2] == b'/')
        {
            let span = consume_path_span(&text, i, 3, prev);
            out.push_str("<path>");
            i += span;
            continue;
        }

        if boundary {
            if let Some(prefix) = ["/home/", "/Users/", "/run/media/", "/media/", "/mnt/"]
                .iter()
                .find(|p| text[i..].starts_with(**p))
            {
                let span = consume_path_span(&text, i, prefix.len(), prev);
                out.push_str("<path>");
                i += span;
                continue;
            }
        }

        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Development only: `Clade.exe --preview-startup-dialog=N` shows startup message N (1-based) in
/// the language that would be used and exits, so every dialog can be read without breaking the
/// runtime or the folder. Compiled out of release builds.
#[cfg(debug_assertions)]
pub fn preview_requested(root: &Path) {
    let Some(n) = std::env::args()
        .find_map(|a| {
            a.strip_prefix("--preview-startup-dialog=")
                .map(str::to_owned)
        })
        .and_then(|n| n.parse::<usize>().ok())
    else {
        return;
    };
    let Some(id) = n
        .checked_sub(1)
        .and_then(|i| startup_text::ALL_MESSAGES.get(i))
    else {
        return;
    };
    fail(
        *id,
        Some(root),
        Some("preview of this dialog (development build)"),
    );
}

#[cfg(not(debug_assertions))]
pub fn preview_requested(_root: &Path) {}

fn show_error(title: &str, message: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |s: &str| {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        // SAFETY: both buffers are NUL-terminated and outlive the call; a null owner is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                wide(message).as_ptr(),
                wide(title).as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{title}: {message}");
}

/// Deterministic Win32 message-only window class for a specific portable folder.
/// Normalized case-insensitively over UTF-16 code units (encoded as little-endian bytes and
/// hashed with SHA-256) so casing or slash differences map to the same class while separate
/// portable Clade folders can run side by side without colliding.
pub fn wake_window_class(root: &Path) -> String {
    use sha2::{Digest, Sha256};

    let clean = crate::portable::root::normalize_verbatim_path(root.to_path_buf());
    let raw_units: Vec<u16> = {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            clean.as_os_str().encode_wide().collect()
        }
        #[cfg(not(windows))]
        {
            clean.to_string_lossy().encode_utf16().collect()
        }
    };

    let mut normalized: Vec<u16> = raw_units
        .into_iter()
        .map(|u| {
            if u == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                u
            }
        })
        .collect();
    while normalized.len() > 3 && normalized.last() == Some(&u16::from(b'\\')) {
        normalized.pop();
    }

    let mut hasher = Sha256::new();
    for decoded in char::decode_utf16(normalized) {
        match decoded {
            Ok(ch) => {
                let mut buf = [0u16; 2];
                for lower in ch.to_lowercase() {
                    for u in lower.encode_utf16(&mut buf) {
                        hasher.update(u.to_le_bytes());
                    }
                }
            }
            Err(unpaired) => {
                hasher.update(unpaired.unpaired_surrogate().to_le_bytes());
            }
        }
    }
    let hex = hex::encode(hasher.finalize());
    format!("CladeWake.{}", &hex[..16])
}

#[cfg(windows)]
const WM_CLADE_WAKE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Another Clade on this PC already owns the folder: signal its per-folder message-only window so
/// it brings its UI back (it may be hidden while a game runs) instead of reporting an error, and
/// without colliding with File Explorer windows or separate portable copies.
#[cfg(windows)]
pub fn wake_running_instance(root: &Path) -> bool {
    use std::time::Duration;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, FindWindowExW, GetWindowThreadProcessId, PostMessageW,
        HWND_MESSAGE,
    };

    let class = wake_window_class(root);
    let wide_class: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
    for attempt in 0..5 {
        // SAFETY: `wide_class` is NUL-terminated and outlives the call; `HWND_MESSAGE` scopes the
        // lookup strictly to message-only windows.
        let hwnd = unsafe {
            FindWindowExW(
                HWND_MESSAGE,
                std::ptr::null_mut(),
                wide_class.as_ptr(),
                std::ptr::null(),
            )
        };
        if !hwnd.is_null() {
            let mut pid = 0u32;
            // SAFETY: `hwnd` was returned by `FindWindowExW`; `pid` is a valid out scalar.
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid != 0 {
                    AllowSetForegroundWindow(pid);
                }
                return PostMessageW(hwnd, WM_CLADE_WAKE, 0, 0) != 0;
            }
        }
        if attempt + 1 < 5 {
            std::thread::sleep(Duration::from_millis(40));
        }
    }
    false
}

#[cfg(unix)]
fn wake_socket_path(root: &Path) -> Option<std::path::PathBuf> {
    let sock = root.join("data").join("wake.sock");
    (sock.as_os_str().len() < 104).then_some(sock)
}

#[cfg(unix)]
pub fn wake_running_instance(root: &Path) -> bool {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let Some(sock) = wake_socket_path(root) else {
        return false;
    };
    for attempt in 0..5 {
        if let Ok(mut stream) = UnixStream::connect(&sock) {
            return stream.write_all(b"w").is_ok();
        }
        if attempt + 1 < 5 {
            std::thread::sleep(Duration::from_millis(40));
        }
    }
    false
}

#[cfg(not(any(windows, unix)))]
pub fn wake_running_instance(_root: &Path) -> bool {
    false
}

#[cfg(windows)]
struct WakeState {
    on_wake: Box<dyn Fn() + Send + Sync>,
}

#[cfg(windows)]
unsafe extern "system" fn wake_wnd_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetWindowLongPtrW, SetWindowLongPtrW, CREATESTRUCTW, GWLP_USERDATA,
        WM_NCCREATE,
    };

    if msg == WM_NCCREATE && lparam != 0 {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    } else if msg == WM_CLADE_WAKE {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WakeState;
        if !ptr.is_null() {
            ((*ptr).on_wake)();
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Spawns a lightweight Win32 message-only window listener (`"clade-wake"`) for `root`. Parked in
/// the kernel inside `GetMessageW` at 0% CPU until `WM_CLADE_WAKE` arrives.
#[cfg(windows)]
pub fn spawn_wake_listener(root: &Path, on_wake: impl Fn() + Send + Sync + 'static) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassExW, TranslateMessage,
        HWND_MESSAGE, MSG, WNDCLASSEXW,
    };

    let class = wake_window_class(root);
    let spawned = std::thread::Builder::new()
        .name("clade-wake".into())
        .spawn(move || {
            let wide_class: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
            let state_ptr = Box::into_raw(Box::new(WakeState {
                on_wake: Box::new(on_wake),
            }));
            // SAFETY: `WNDCLASSEXW` and `MSG` are plain zero-initialisable structs; `wide_class`
            // outlives class registration and the message loop; `state_ptr` is reclaimed if window
            // creation fails and otherwise lives for the lifetime of the listener thread.
            unsafe {
                let mut wc: WNDCLASSEXW = std::mem::zeroed();
                wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
                wc.lpfnWndProc = Some(wake_wnd_proc);
                wc.lpszClassName = wide_class.as_ptr();
                RegisterClassExW(&wc);

                let hwnd = CreateWindowExW(
                    0,
                    wide_class.as_ptr(),
                    wide_class.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    state_ptr.cast(),
                );
                if hwnd.is_null() {
                    drop(Box::from_raw(state_ptr));
                    tracing::warn!("could not create message-only wake listener window");
                    return;
                }

                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not spawn wake listener thread");
    }
}

#[cfg(unix)]
pub fn spawn_wake_listener<F>(root: &Path, on_wake: F)
where
    F: Fn() + Send + Sync + 'static,
{
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    let Some(sock) = wake_socket_path(root) else {
        tracing::warn!("wake socket path exceeds Unix sun_path limit; skipping wake listener");
        return;
    };
    if let Some(parent) = sock.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::remove_file(&sock);
    let Ok(listener) = UnixListener::bind(&sock) else {
        tracing::warn!("could not bind Unix wake socket");
        return;
    };
    let _ = fs::set_permissions(&sock, fs::Permissions::from_mode(0o600));
    let spawned = std::thread::Builder::new()
        .name("clade-wake".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut buf = [0u8; 1];
                let _ = stream.take(1).read(&mut buf);
                on_wake();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not spawn wake listener thread");
    }
}

#[cfg(not(any(windows, unix)))]
pub fn spawn_wake_listener(_root: &Path, _on_wake: impl Fn() + Send + Sync + 'static) {}

/// Logs go to `logs/clade.log` inside the folder. One rotation keeps the stick tidy.
/// Secrets must never reach this log: only log ids and states, never tokens or passphrases.
pub fn init_logging(root: &PortableRoot) {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    let path = root.logs_dir().join("clade.log");
    if fs::metadata(&path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(&path, root.logs_dir().join("clade.old.log"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = tracing_subscriber::fmt()
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .try_init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_detail_strips_launcher_root_drive_paths_and_unc_shares() {
        let root = Path::new(r"D:\Portable\Clade");
        assert_eq!(
            redact_detail(
                r"could not read D:\Portable\Clade\data\settings.json: invalid json",
                Some(root),
            ),
            r"could not read <clade>\data\settings.json: invalid json"
        );
        assert_eq!(
            redact_detail(
                "failed at d:/portable/clade/data/clade.lock (os error 32)",
                Some(root),
            ),
            "failed at <clade>/data/clade.lock (os error 32)"
        );
        assert_eq!(
            redact_detail(
                r"verbatim \\?\D:\Portable\Clade\data\vault.bin unreadable",
                Some(root),
            ),
            r"verbatim <clade>\data\vault.bin unreadable"
        );

        let user_err = redact_detail(
            r#"access denied at C:\Users\alice\Desktop\file.txt and "C:\Users\Alice Smith\Secrets\note.txt""#,
            Some(root),
        );
        assert!(!user_err.contains("alice"), "{user_err}");
        assert!(!user_err.contains("Alice"), "{user_err}");
        assert!(!user_err.contains("Smith"), "{user_err}");
        assert_eq!(user_err, r#"access denied at <path> and "<path>""#);

        let unc_err = redact_detail(
            r"share error on \\fileserver\team\alice\Clade and \\?\UNC\nas\backups\file.dat.",
            None,
        );
        assert!(!unc_err.contains("fileserver"), "{unc_err}");
        assert!(!unc_err.contains("nas"), "{unc_err}");
        assert!(!unc_err.contains("alice"), "{unc_err}");
        assert_eq!(unc_err, "share error on <unc-path> and <unc-path>.");

        let unix_root = Path::new("/home/alice/Clade");
        let unix_err = redact_detail(
            "failed at /home/alice/Clade/data/clade.lock and \"/run/media/alice/USB Stick/secret.txt\" or /home/bob/notes.txt",
            Some(unix_root),
        );
        assert!(!unix_err.contains("bob"), "{unix_err}");
        assert!(!unix_err.contains("USB"), "{unix_err}");
        assert_eq!(
            unix_err,
            "failed at <clade>/data/clade.lock and \"<path>\" or <path>"
        );
    }

    #[test]
    fn wake_window_class_is_case_and_slash_insensitive_and_distinct_per_folder() {
        let c1 = wake_window_class(Path::new(r"D:\Games\Clade"));
        let c2 = wake_window_class(Path::new(r"d:\games\clade\"));
        let c3 = wake_window_class(Path::new("D:/Games/Clade"));
        let c4 = wake_window_class(Path::new(r"\\?\D:\Games\Clade"));
        assert!(c1.starts_with("CladeWake."));
        assert_eq!(c1.len(), "CladeWake.".len() + 16);
        assert_eq!(c1, c2);
        assert_eq!(c1, c3);
        assert_eq!(c1, c4);

        let other = wake_window_class(Path::new(r"E:\USB\Clade"));
        assert_ne!(c1, other);
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn wake_listener_and_client_round_trip_per_folder_without_cross_talk() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        assert!(!wake_running_instance(dir_b.path()));

        let (tx, rx) = mpsc::channel();
        spawn_wake_listener(dir_a.path(), move || {
            let _ = tx.send(());
        });

        assert!(wake_running_instance(dir_a.path()));
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
        assert!(!wake_running_instance(dir_b.path()));
    }
}
