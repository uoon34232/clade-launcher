//! Executable portability rules (docs/TECHNICAL.md, "Portable filesystem architecture"). They scan the launcher's own
//! production Rust code for constructs that would tie data or behaviour to one machine: the
//! system TEMP directory, user-profile/AppData locations, ambient environment lookups, bare
//! process spawning (which would inherit the user's environment) and hard-coded absolute paths.
//!
//! Test code (everything from the first `#[cfg(test)]` line of a file) and comments are not
//! scanned. A genuine exception needs a `portability-allow: <reason>` comment on the line or
//! within the three lines above it, which makes every exception visible in review.

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

/// Detects a quoted absolute Windows path literal such as `"C:\..."` or `r"D:/..."`.
fn has_absolute_literal(line: &str) -> bool {
    let b = line.as_bytes();
    (1..b.len().saturating_sub(2)).any(|i| {
        b[i - 1] == b'"'
            && b[i].is_ascii_alphabetic()
            && b[i + 1] == b':'
            && (b[i + 2] == b'\\' || b[i + 2] == b'/')
    })
}

/// Returns `(line number, reason)` for every violation in the production part of `text`.
fn scan_source(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "#[cfg(test)]" {
            break; // by convention tests sit at the bottom of the file
        }
        if is_comment(line) {
            continue;
        }
        let allowed = (i.saturating_sub(3)..=i).any(|j| lines[j].contains("portability-allow:"));
        if allowed {
            continue;
        }
        for (token, why) in FORBIDDEN {
            if line.contains(token) {
                hits.push((i + 1, format!("`{token}`: {why}")));
            }
        }
        if has_absolute_literal(line) {
            hits.push((i + 1, "hard-coded absolute path literal".to_owned()));
        }
    }
    hits
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
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
        "let c = std::process::Command::new(\"java\");",
        "let g = home.join(\".minecraft\");",
        "let r = PathBuf::from(r\"C:\\Users\\x\");",
        "std::env::set_var(\"X\", \"1\");",
        "let d = dirs::home_dir();",
    ];
    for line in bad {
        assert!(!scan_source(line).is_empty(), "not detected: {line}");
    }
    // Allowed forms.
    assert!(scan_source("let d = tempfile::Builder::new().tempdir_in(&cache);").is_empty());
    assert!(scan_source("// std::env::temp_dir() is forbidden").is_empty());
    assert!(scan_source("// portability-allow: reason\nlet h = std::env::var(\"X\");").is_empty());
    assert!(
        scan_source("let x = 1;\n#[cfg(test)]\nmod t { fn f() { std::env::temp_dir(); } }")
            .is_empty()
    );
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
        let prod = text.split("#[cfg(test)]").next().unwrap_or("");
        let n = prod.matches("portability-allow:").count();
        if n > 0 {
            found.push(format!("{}={n}", f.file_name().unwrap().to_string_lossy()));
        }
    }
    found.sort();
    assert_eq!(
        found,
        [
            "launch_args.rs=1", // the one place the game process is created (scrubbed environment)
            "loader_install.rs=4", // SystemRoot lookup (+fallback), the one scrubbed spawn site, the read-only tripwire
            "webview.rs=2", // WEBVIEW2_BROWSER_EXECUTABLE_FOLDER pointing at the bundled runtime
        ],
        "the set of portability exceptions changed"
    );
}
