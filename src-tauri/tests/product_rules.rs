//! Executable product rules. These tests exist so the launcher cannot drift into a content
//! marketplace by accident. If one fails, do not weaken it to make a change pass: the product
//! philosophy in docs/TECHNICAL.md and README.md must be changed first, on purpose.

use std::fs;
use std::path::{Path, PathBuf};

use clade_launcher_lib::download_policy::{
    DEFAULT_ALLOWED_HOSTS, UPDATE_CHECK_HOSTS, UPDATE_DOWNLOAD_HOSTS,
};

/// Third-party content platforms and content-browsing features that must never appear. Built
/// from parts so this file never matches itself.
fn forbidden() -> Vec<String> {
    vec![
        ["modr", "inth"].concat(),
        ["curse", "forge"].concat(),
        ["forge", "cdn"].concat(),
        ["mod", "_browser"].concat(),
        ["mod", "browser"].concat(),
        ["mod", "pack_browser"].concat(),
        ["mod", "packbrowser"].concat(),
        ["shader", "_browser"].concat(),
        ["shader", "browser"].concat(),
        ["market", "place"].concat(),
    ]
}

fn forbidden_api_key_forms() -> Vec<String> {
    vec![
        ["api", "_key"].concat(),
        ["api", "key"].concat(),
        ["api", "-key"].concat(),
    ]
}

fn is_allowed_diagnostics_redaction_line(line: &str, needle: &str) -> bool {
    // Only the explicit log-redaction patterns/docs in diagnostics.rs are exempt, never `api_key`.
    if needle == ["api", "_key"].concat() {
        return false;
    }
    let trimmed = line.trim();
    trimmed == ["\"api", "key\","].concat()
        || trimmed == ["\"x-api", "-key\","].concat()
        || trimmed.starts_with(&["/// Whether a name (`api", "-key`"].concat())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    assert!(
        dir.is_dir(),
        "expected scanned directory to exist: {}",
        dir.display()
    );
    let rd = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read directory {}: {e}", dir.display()));
    for entry in rd {
        let entry = entry
            .unwrap_or_else(|e| panic!("failed to read directory entry in {}: {e}", dir.display()));
        let p = entry.path();
        if p.is_dir() {
            collect(&p, out);
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("rs" | "ts" | "tsx" | "json" | "toml" | "css" | "html")
        ) {
            out.push(p);
        }
    }
}

fn product_files(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![
        root.join("package.json"),
        root.join("index.html"),
        root.join("src-tauri").join("Cargo.toml"),
        root.join("src-tauri").join("tauri.conf.json"),
    ];
    for f in &files {
        assert!(
            f.is_file(),
            "expected scanned config file to exist: {}",
            f.display()
        );
    }
    for (dir, min_files) in [
        ("src", 5usize),
        ("src-tauri/src", 10usize),
        ("src-tauri/capabilities", 1usize),
        ("e2e", 1usize),
    ] {
        let dir_path = root.join(dir);
        let before = files.len();
        collect(&dir_path, &mut files);
        let scanned = files.len() - before;
        assert!(
            scanned >= min_files,
            "scanner found too few files in {}: {scanned} (expected at least {min_files})",
            dir_path.display()
        );
    }
    files
}

fn read_source(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn no_content_platform_or_content_browser_anywhere_in_the_product() {
    let root = repo_root();
    let files = product_files(&root);
    let needles = forbidden();
    let mut hits = Vec::new();
    for f in files {
        let text = read_source(&f);
        let lower = text.to_lowercase();
        for n in &needles {
            if lower.contains(n.as_str()) {
                hits.push(format!(
                    "{} contains '{n}'",
                    f.strip_prefix(&root).unwrap_or(&f).display()
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "product rule violated (vanilla-first, no content platforms):\n{}",
        hits.join("\n")
    );
}

#[test]
fn network_allow_list_is_exactly_minecraft_microsoft_sign_in_and_loader_runtimes() {
    // Changing this list changes what the launcher can contact. Update it only together with
    // the product rules in docs/TECHNICAL.md, never to add a mod/modpack/shader/resource-pack source.
    let approved = [
        "mojang.com",
        "minecraft.net",
        "minecraftservices.com",
        "microsoftonline.com",
        "live.com",
        "xboxlive.com",
        "fabricmc.net",
        "quiltmc.org",
        "minecraftforge.net",
        "neoforged.net",
    ];
    let mut actual: Vec<&str> = DEFAULT_ALLOWED_HOSTS.to_vec();
    let mut expected: Vec<&str> = approved.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

#[test]
fn update_check_allow_list_is_isolated_from_game_downloads() {
    assert_eq!(UPDATE_CHECK_HOSTS, ["api.github.com"]);
    assert_eq!(
        UPDATE_DOWNLOAD_HOSTS,
        [
            "github.com",
            "release-assets.githubusercontent.com",
            "objects.githubusercontent.com"
        ]
    );
    for host in UPDATE_CHECK_HOSTS
        .iter()
        .chain(UPDATE_DOWNLOAD_HOSTS.iter())
    {
        assert!(!DEFAULT_ALLOWED_HOSTS.contains(host), "{host}");
    }
    assert!(!DEFAULT_ALLOWED_HOSTS.contains(&"githubusercontent.com"));
}

#[test]
fn no_api_key_settings_exist() {
    // The launcher must never require third-party API keys.
    let root = repo_root();
    let files = product_files(&root);
    let diagnostics_rs = root.join("src-tauri").join("src").join("diagnostics.rs");
    let needles = forbidden_api_key_forms();
    let mut hits = Vec::new();
    for f in files {
        let text = read_source(&f);
        let is_diagnostics = f == diagnostics_rs;
        for (line_idx, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            for n in &needles {
                if lower.contains(n.as_str())
                    && !(is_diagnostics && is_allowed_diagnostics_redaction_line(line, n))
                {
                    hits.push(format!(
                        "{}:{} mentions '{n}'",
                        f.strip_prefix(&root).unwrap_or(&f).display(),
                        line_idx + 1
                    ));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "product rule violated (no API key settings):\n{}",
        hits.join("\n")
    );
}

#[test]
fn product_rule_scanners_catch_camel_case_and_kebab_case_variants() {
    let content_needles = forbidden();
    for sample in [
        ["mod", "Browser"].concat(),
        ["modpack", "Browser"].concat(),
        ["shader", "Browser"].concat(),
    ] {
        let lower = sample.to_lowercase();
        assert!(
            content_needles.iter().any(|n| lower.contains(n.as_str())),
            "content scanner missed {sample}"
        );
    }

    let api_needles = forbidden_api_key_forms();
    for sample in [
        ["api", "_key"].concat(),
        ["api", "Key"].concat(),
        ["api", "-key"].concat(),
    ] {
        let lower = sample.to_lowercase();
        assert!(
            api_needles.iter().any(|n| lower.contains(n.as_str())),
            "API key scanner missed {sample}"
        );
    }

    assert!(is_allowed_diagnostics_redaction_line(
        &["    \"api", "key\","].concat(),
        &["api", "key"].concat()
    ));
    assert!(is_allowed_diagnostics_redaction_line(
        &["    \"x-api", "-key\","].concat(),
        &["api", "-key"].concat()
    ));
    assert!(!is_allowed_diagnostics_redaction_line(
        &["let api", "Key = 1;"].concat(),
        &["api", "key"].concat()
    ));
    assert!(!is_allowed_diagnostics_redaction_line(
        &["    \"api", "_key\","].concat(),
        &["api", "_key"].concat()
    ));
}
