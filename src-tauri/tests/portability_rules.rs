//! Executable portability rules (docs/TECHNICAL.md, "Portable filesystem architecture"). They scan the launcher's own
//! production Rust code for constructs that would tie data or behaviour to one machine: the
//! system TEMP directory, user-profile/AppData locations, ambient environment lookups, bare
//! process spawning (which would inherit the user's environment) and hard-coded absolute paths.
//!
//! Only items and modules annotated with `#[cfg(test)]` (and comments) are skipped — mid-file
//! `#[cfg(test)]` helpers never mask production code below them. A genuine exception needs a
//! `portability-allow: <reason>` comment on the line or within the three lines above it, which
//! makes every exception visible in review.

use std::fs;
use std::path::{Path, PathBuf};

/// (token, why it is forbidden)
const FORBIDDEN: &[(&str, &str)] = &[
    (
        "env::var",
        "ambient environment lookups make behaviour machine-specific",
    ),
    (
        "env::vars",
        "ambient environment lookups make behaviour machine-specific",
    ),
    (
        "use std::env",
        "ambient std::env imports risk machine-specific lookups",
    ),
    (
        "env::temp_dir",
        "the system TEMP directory is machine-specific; use PortableRoot::tmp_dir()",
    ),
    ("env::home_dir", "the user profile is machine-specific"),
    (
        "env::current_dir",
        "the process working directory is machine-specific",
    ),
    (
        "set_var(",
        "process-wide environment changes must be deliberate and visible",
    ),
    (
        "remove_var(",
        "process-wide environment changes must be deliberate and visible",
    ),
    (
        "app_data_dir",
        "Tauri's AppData locations are machine-specific",
    ),
    (
        "app_config_dir",
        "Tauri's AppData locations are machine-specific",
    ),
    (
        "app_cache_dir",
        "Tauri's AppData locations are machine-specific",
    ),
    (
        "app_local_data_dir",
        "Tauri's AppData locations are machine-specific",
    ),
    (
        "app_log_dir",
        "Tauri's AppData locations are machine-specific",
    ),
    ("dirs::", "OS known-folder lookups are machine-specific"),
    (
        "directories::",
        "OS known-folder lookups are machine-specific",
    ),
    (
        "tempfile::tempdir(",
        "uses the system TEMP directory; use tempdir_in(<launcher cache>)",
    ),
    (
        "TempDir::new(",
        "uses the system TEMP directory; use tempdir_in(<launcher cache>)",
    ),
    (
        "tempdir()",
        "uses the system TEMP directory; use tempdir_in(<launcher cache>)",
    ),
    ("NamedTempFile::new(", "uses the system TEMP directory"),
    ("tempfile::tempfile(", "uses the system TEMP directory"),
    (
        "Command::new(",
        "child processes must get a scrubbed environment (see loader_install)",
    ),
    // Quoted or path-separated forms only, so `inst.minecraft` and `minecraft_dir()` are not hit.
    (
        "\".minecraft\"",
        "the global .minecraft directory must never be used",
    ),
    (
        "\\.minecraft",
        "the global .minecraft directory must never be used",
    ),
    (
        "/.minecraft",
        "the global .minecraft directory must never be used",
    ),
];

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

#[derive(Default)]
struct LexState {
    block_comment_depth: usize,
    in_string: bool,
    raw_hashes: Option<usize>,
}

impl LexState {
    fn step_line(&mut self, line: &str) -> i32 {
        let b = line.as_bytes();
        let mut i = 0;
        let mut delta = 0i32;

        while i < b.len() {
            if self.block_comment_depth > 0 {
                if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
                    self.block_comment_depth += 1;
                    i += 2;
                    continue;
                }
                if i + 1 < b.len() && b[i] == b'*' && b[i + 1] == b'/' {
                    self.block_comment_depth -= 1;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if let Some(hashes) = self.raw_hashes {
                if b[i] == b'"'
                    && b.get(i + 1..i + 1 + hashes)
                        .is_some_and(|s| s.iter().all(|&c| c == b'#'))
                {
                    self.raw_hashes = None;
                    i += 1 + hashes;
                    continue;
                }
                i += 1;
                continue;
            }

            if self.in_string {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    self.in_string = false;
                }
                i += 1;
                continue;
            }

            // Outside comments and strings: check line/block comments first.
            if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
                break;
            }
            if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
                self.block_comment_depth += 1;
                i += 2;
                continue;
            }
            // Check raw strings: r"...", r#"..."#, b"...", br#"..."#
            if (b[i] == b'r' || (b[i] == b'b' && b.get(i + 1) == Some(&b'r')))
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
            {
                let start_hashes = if b[i] == b'b' { i + 2 } else { i + 1 };
                let mut j = start_hashes;
                while j < b.len() && b[j] == b'#' {
                    j += 1;
                }
                if j < b.len() && b[j] == b'"' {
                    self.raw_hashes = Some(j - start_hashes);
                    i = j + 1;
                    continue;
                }
            }
            // Char / byte literals `'{'`, `'}'`, `'\''`
            if b[i] == b'\'' {
                if b.get(i + 1) == Some(&b'\\') {
                    // Advance past escaped char literal if closing `'` is nearby; avoid swallowing lifetimes.
                    if let Some(end) = b
                        .get(i + 2..i + 6)
                        .and_then(|s| s.iter().position(|&c| c == b'\''))
                    {
                        i += 2 + end + 1;
                        continue;
                    }
                } else if b.get(i + 2) == Some(&b'\'') {
                    i += 3;
                    continue;
                }
            }
            if b[i] == b'"' {
                self.in_string = true;
                i += 1;
                continue;
            }
            if b[i] == b'{' {
                delta += 1;
            } else if b[i] == b'}' {
                delta -= 1;
            }
            i += 1;
        }
        delta
    }
}

/// Extracts `(1-based line number, line text)` belonging to production code, skipping ONLY the
/// module/item annotated by each `#[cfg(test)]` attribute (never skipping later production items).
fn production_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut lex = LexState::default();
    let mut pending_test_attr = false;
    let mut test_brace_depth: i32 = 0;
    let mut inside_test_item = false;

    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        let was_plain_context =
            lex.block_comment_depth == 0 && !lex.in_string && lex.raw_hashes.is_none();
        let delta = lex.step_line(line);

        if inside_test_item {
            test_brace_depth += delta;
            if test_brace_depth <= 0 {
                inside_test_item = false;
                test_brace_depth = 0;
            }
            continue;
        }
        if pending_test_attr {
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[") {
                continue;
            }
            if delta > 0 {
                inside_test_item = true;
                test_brace_depth = delta;
                pending_test_attr = false;
            } else if (!trimmed.contains('{') && trimmed.ends_with(';')) || line.contains('{') {
                pending_test_attr = false;
            }
            continue;
        }
        if was_plain_context && trimmed == "#[cfg(test)]" {
            pending_test_attr = true;
            continue;
        }
        out.push((idx + 1, line));
    }
    out
}

/// Detects hard-coded absolute drive (`"C:\..."`), UNC (`"\\server\share"`, `"//server/share"`),
/// or extended/device (`"\\?\C:\..."`, `"\\?\UNC\server\..."`, `"\\.\pipe\..."`) path literals,
/// while ignoring bare prefix-strip constants such as `"\\\\?\\"` or `"https://..."` URLs.
fn has_absolute_literal(line: &str) -> bool {
    let b = line.as_bytes();
    // 1. Drive-letter paths: `"C:\..."`, `"C:\\..."`, `r"D:/..."`
    if (1..b.len().saturating_sub(2)).any(|i| {
        b[i - 1] == b'"'
            && b[i].is_ascii_alphabetic()
            && b[i + 1] == b':'
            && (b[i + 2] == b'\\' || b[i + 2] == b'/')
    }) {
        return true;
    }
    // 2. Quoted UNC / verbatim / device paths at the start of string literals.
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == b'"' && !(i > 0 && b[i - 1] == b'\\') {
            let rest = &line[i + 1..];
            // Raw-looking forward UNC `"//server/share"` (opening `"` not `://`).
            if let Some(after_slash) = rest.strip_prefix("//") {
                if let Some(end_q) = after_slash.find('"') {
                    let inner = &after_slash[..end_q];
                    if inner
                        .bytes()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
                        && inner.contains('/')
                    {
                        return true;
                    }
                }
            }
            // Verbatim / device literals: `\\?\<target>`, `\\\\?\\<target>`, `\\.\<dev>`, `\\\\.\\<dev>`
            for prefix in [r"\\\\?\\", r"\\?\", r"\\\\.\\", r"\\.\"] {
                if let Some(after) = rest.strip_prefix(prefix) {
                    if let Some(end_q) = find_string_end(after, !rest_is_raw(line, i)) {
                        let tail = &after[..end_q];
                        if !tail.is_empty()
                            && !tail.eq_ignore_ascii_case("UNC\\")
                            && tail != "UNC\\\\"
                        {
                            return true;
                        }
                    }
                }
            }
            // UNC literals: `\\server\share` or `\\\\server\\share`
            for prefix in [r"\\\\", r"\\"] {
                if let Some(after) = rest.strip_prefix(prefix) {
                    if after
                        .bytes()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
                    {
                        if let Some(end_q) = find_string_end(after, !rest_is_raw(line, i)) {
                            let inner = &after[..end_q];
                            if inner.contains('\\') {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        i += 1;
    }
    false
}

fn rest_is_raw(line: &str, quote_idx: usize) -> bool {
    quote_idx > 0 && line.as_bytes()[quote_idx - 1] == b'r'
}

fn find_string_end(s: &str, escaped: bool) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if escaped && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == b'"' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Returns `(line number, reason)` for every violation in the production part of `text`.
fn scan_source(text: &str) -> Vec<(usize, String)> {
    let prod = production_lines(text);
    let mut hits = Vec::new();
    for (idx, &(line_no, line)) in prod.iter().enumerate() {
        if is_comment(line) {
            continue;
        }
        let allowed = (idx.saturating_sub(3)..=idx).any(|j| {
            line_no.saturating_sub(prod[j].0) <= 3 && prod[j].1.contains("portability-allow:")
        });
        if allowed {
            continue;
        }
        for (token, why) in FORBIDDEN {
            if line.contains(token) {
                hits.push((line_no, format!("`{token}`: {why}")));
            }
        }
        if has_absolute_literal(line) {
            hits.push((line_no, "hard-coded absolute path literal".to_owned()));
        }
    }
    hits
}

fn is_test_submodule(file: &Path) -> bool {
    let Some(name) = file.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if name != "tests.rs" && !name.ends_with("_tests.rs") {
        return false;
    }
    let Some(mod_stem) = file.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(parent_dir) = file.parent() else {
        return false;
    };
    let parent_rs = parent_dir.with_extension("rs");
    let mod_rs = parent_dir.join("mod.rs");
    let decl = format!("mod {mod_stem};");
    for candidate in [parent_rs, mod_rs] {
        if let Ok(text) = fs::read_to_string(&candidate) {
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            for (i, &l) in lines.iter().enumerate() {
                if l.contains(&decl) && i > 0 && lines[i - 1] == "#[cfg(test)]" {
                    return true;
                }
            }
        }
    }
    false
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") && !is_test_submodule(&p) {
            out.push(p);
        }
    }
}

#[test]
fn production_code_is_free_of_machine_specific_constructs() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        files.len() > 10,
        "scanner found too few files: {}",
        files.len()
    );
    let mut report = Vec::new();
    for f in &files {
        let text = fs::read_to_string(f).unwrap();
        for (line, why) in scan_source(&text) {
            report.push(format!(
                "{}:{line}: {why}",
                f.strip_prefix(&src).unwrap().display()
            ));
        }
    }
    assert!(
        report.is_empty(),
        "portability rule violated. Use PortableRoot/RelPath, or justify an exception with `portability-allow:`:\n{}",
        report.join("\n")
    );
}

#[test]
fn the_scanner_actually_detects_violations() {
    // A guard that silently matches nothing is worse than none: prove it bites.
    let bad = [
        "let t = std::env::temp_dir();",
        "let d = tempfile::tempdir().unwrap();",
        "let p = app.path().app_data_dir();",
        "let h = std::env::var(\"APPDATA\");",
        "use std::env;",
        "let t2 = TempDir::new().unwrap();",
        "let c = std::process::Command::new(\"java\");",
        "let g = home.join(\".minecraft\");",
        "let r = PathBuf::from(r\"C:\\Users\\x\");",
        "let u1 = PathBuf::from(r\"\\\\server\\share\\file\");",
        "let u2 = PathBuf::from(\"//server/share/file\");",
        "let e1 = PathBuf::from(r\"\\\\?\\C:\\Users\\x\");",
        "let e2 = PathBuf::from(\"\\\\\\\\.\\\\pipe\\\\test\");",
        "std::env::set_var(\"X\", \"1\");",
        "let d = dirs::home_dir();",
    ];
    for line in bad {
        assert!(!scan_source(line).is_empty(), "not detected: {line}");
    }
    // Allowed forms (including prefix-strip literals and URLs).
    assert!(scan_source("let d = tempfile::Builder::new().tempdir_in(&cache);").is_empty());
    assert!(scan_source("// std::env::temp_dir() is forbidden").is_empty());
    assert!(scan_source("// portability-allow: reason\nlet h = std::env::var(\"X\");").is_empty());
    assert!(scan_source("let p = text.replace(\"\\\\\\\\?\\\\\", \"\");").is_empty());
    assert!(scan_source("let p = raw.strip_prefix(r\"\\\\?\\UNC\\\");").is_empty());
    assert!(scan_source("const URL: &str = \"https://example.com/a/b\";").is_empty());
    assert!(
        scan_source("let x = 1;\n#[cfg(test)]\nmod t { fn f() { std::env::temp_dir(); } }")
            .is_empty()
    );
    // Mid-file #[cfg(test)] helpers do NOT silence subsequent production code.
    let mid_file_leak = scan_source(
        "#[cfg(test)]\nfn test_helper() -> bool { true }\nfn prod_fn() { let _ = std::env::temp_dir(); }",
    );
    assert_eq!(mid_file_leak.len(), 1);
    assert_eq!(mid_file_leak[0].0, 3);
    // An exception comment does not leak to code far below it.
    assert!(!scan_source("// portability-allow: reason\nlet a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\nlet h = std::env::var(\"X\");").is_empty());
}

#[test]
fn every_exception_is_listed_so_new_ones_get_reviewed() {
    // Adding a `portability-allow:` exception must be a conscious, reviewed change: update this
    // list together with the justification in docs/TECHNICAL.md.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut found: Vec<String> = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        let n: usize = production_lines(&text)
            .into_iter()
            .map(|(_, line)| line.matches("portability-allow:").count())
            .sum();
        if n > 0 {
            found.push(format!("{}={n}", f.file_name().unwrap().to_string_lossy()));
        }
    }
    found.sort();
    assert_eq!(
        found,
        [
            "launch_args.rs=1", // the one place the game process is created (scrubbed environment)
            "loader_install.rs=5", // stage-local .minecraft stand-in, SystemRoot lookup (+fallback), the one scrubbed spawn site, the read-only tripwire
            "update_apply.rs=1", // spawns only the verified staged update helper or relaunches the updated Clade executable
            "webview.rs=2", // WEBVIEW2_BROWSER_EXECUTABLE_FOLDER pointing at the bundled runtime
        ],
        "the set of portability exceptions changed"
    );
}
