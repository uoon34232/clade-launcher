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
