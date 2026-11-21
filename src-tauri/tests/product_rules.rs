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
        ["shader", "_browser"].concat(),
        ["market", "place"].concat(),
    ]
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
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

#[test]
fn no_content_platform_or_content_browser_anywhere_in_the_product() {
    let root = repo_root();
    let mut files = vec![
        root.join("package.json"),
        root.join("index.html"),
        root.join("src-tauri").join("Cargo.toml"),
        root.join("src-tauri").join("tauri.conf.json"),
    ];
    for dir in ["src", "src-tauri/src", "src-tauri/capabilities", "e2e"] {
        collect(&root.join(dir), &mut files);
    }
    let needles = forbidden();
    let mut hits = Vec::new();
    for f in files {
        let Ok(text) = fs::read_to_string(&f) else {
            continue;
        };
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
    let mut files = Vec::new();
    collect(&root.join("src-tauri").join("src"), &mut files);
    collect(&root.join("src"), &mut files);
    let needle = ["api", "_key"].concat();
    for f in files {
        if let Ok(t) = fs::read_to_string(&f) {
            assert!(
                !t.to_lowercase().contains(&needle),
                "{} mentions an API key setting",
                f.display()
            );
        }
    }
}
