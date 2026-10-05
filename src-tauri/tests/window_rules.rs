//! Clade is a windowed launcher: the window must not be maximizable or fullscreen, and the
//! frontend must not be able to ask for either (double-clicking the drag strip would otherwise
//! call the toggle-maximize command).

use serde_json::Value;

fn read(rel: &str) -> Value {
    let path = format!("{}/{rel}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn the_main_window_is_configured_as_not_maximizable_and_not_fullscreen() {
    let conf = read("tauri.conf.json");
    let window = &conf["app"]["windows"][0];
    assert_eq!(window["label"], "main");
    assert_eq!(window["maximizable"], false);
    assert_eq!(window["fullscreen"], false);
    assert!(window.get("maximized").is_none_or(|v| v == false));
}

#[test]
fn the_capability_denies_maximize_and_fullscreen_commands() {
    let cap = read("capabilities/default.json");
    let permissions: Vec<&str> = cap["permissions"]
        .as_array()
        .expect("permissions")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for denied in [
        "core:window:deny-internal-toggle-maximize",
        "core:window:deny-maximize",
        "core:window:deny-toggle-maximize",
        "core:window:deny-set-fullscreen",
    ] {
        assert!(permissions.contains(&denied), "{denied} missing");
    }
    for granted in [
        "core:window:allow-minimize",
        "core:window:allow-close",
        "core:window:allow-start-dragging",
    ] {
        assert!(permissions.contains(&granted), "{granted} missing");
    }
}

#[test]
fn the_webview_has_a_strict_content_security_policy() {
    let conf = read("tauri.conf.json");
    let csp = conf["app"]["security"]["csp"]
        .as_str()
        .expect("a CSP string is configured");
    let directive = |name: &str| -> Vec<&str> {
        csp.split(';')
            .map(str::trim)
            .find_map(|d| d.strip_prefix(name).filter(|r| r.starts_with(' ')))
            .unwrap_or_else(|| panic!("CSP has no {name} directive: {csp}"))
            .split_whitespace()
            .collect()
    };
    assert_eq!(directive("default-src"), ["'self'"]);
    assert_eq!(directive("script-src"), ["'self'"]);
    assert_eq!(directive("img-src"), ["'self'", "data:"]);
    assert_eq!(directive("font-src"), ["'self'"]);
    assert_eq!(directive("connect-src"), ["ipc:", "http://ipc.localhost"]);
    // inline styles are needed by the interface; inline or eval-ed *scripts* never are
    assert!(directive("style-src").contains(&"'unsafe-inline'"));
    for forbidden in [
        "unsafe-eval",
        "wasm-unsafe-eval",
        "*",
        "https:",
        "http:",
        "ws:",
        "blob:",
    ] {
        assert!(
            !csp.split_whitespace()
                .any(|t| t.trim_matches('\'') == forbidden),
            "the CSP must not contain {forbidden}: {csp}"
        );
    }
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
}

#[test]
fn the_frontend_has_no_generic_url_or_path_opening_capability() {
    // Opening folders, logs and the sign-in page is done by Rust itself with fixed locations; the
    // page must not be able to ask the opener plugin to open anything.
    let cap = read("capabilities/default.json");
    let permissions = cap["permissions"].as_array().expect("permissions");
    assert!(
        permissions
            .iter()
            .filter_map(Value::as_str)
            .all(|p| !p.starts_with("opener:")),
        "no opener permission may be granted to the frontend"
    );
    let package = read("../package.json");
    for section in ["dependencies", "devDependencies"] {
        assert!(
            package[section].get("@tauri-apps/plugin-opener").is_none(),
            "the opener's JavaScript API is not used by the frontend"
        );
    }
}
