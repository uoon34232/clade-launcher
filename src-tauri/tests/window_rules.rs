//! Clade is a windowed launcher: the window must not be maximizable or fullscreen, and the
//! frontend must not be able to ask for either (double-clicking the drag strip would otherwise
//! call the toggle-maximize command).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

const FORBIDDEN_WINDOW_PERMS: &[&str] = &[
    "core:window:allow-internal-toggle-maximize",
    "core:window:allow-maximize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-set-fullscreen",
    "core:window:allow-set-maximizable",
    "core:window:allow-set-fullscreen-on-monitor",
    "core:window:allow-set-simple-fullscreen",
    "core:window:*",
    "core:*",
    "*",
];

#[derive(Clone)]
struct CapabilityDoc {
    source: String,
    value: Value,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> Value {
    let path = manifest_dir().join(rel);
    read_json(&path).unwrap_or_else(|e| panic!("{e}"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn collect_capability_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|entry| {
            entry
                .map(|e| e.path())
                .map_err(|e| format!("{}: {e}", dir.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for path in entries {
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "unexpected symlink in capabilities: {}",
                path.display()
            ));
        }
        if meta.is_dir() {
            collect_capability_files(&path, out)?;
        } else if meta.is_file() {
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                return Err(format!(
                    "unexpected non-JSON capability file: {}",
                    path.display()
                ));
            }
            out.push(path);
        } else {
            return Err(format!("unexpected capability entry: {}", path.display()));
        }
    }
    Ok(())
}

fn extract_capabilities_from_value(
    source: &str,
    value: Value,
) -> Result<Vec<CapabilityDoc>, String> {
    match value {
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (idx, item) in items.into_iter().enumerate() {
                if !item.is_object() {
                    return Err(format!(
                        "{source}[{idx}]: capability entry must be an object"
                    ));
                }
                out.push(CapabilityDoc {
                    source: format!("{source}[{idx}]"),
                    value: item,
                });
            }
            Ok(out)
        }
        Value::Object(mut map) => {
            if let Some(caps) = map.remove("capabilities") {
                if map.contains_key("permissions") {
                    return Err(format!(
                        "{source}: ambiguous capability object with both capabilities and permissions"
                    ));
                }
                let Value::Array(items) = caps else {
                    return Err(format!("{source}: capabilities field must be an array"));
                };
                let mut out = Vec::with_capacity(items.len());
                for (idx, item) in items.into_iter().enumerate() {
                    if !item.is_object() {
                        return Err(format!(
                            "{source}.capabilities[{idx}]: capability entry must be an object"
                        ));
                    }
                    out.push(CapabilityDoc {
                        source: format!("{source}.capabilities[{idx}]"),
                        value: item,
                    });
                }
                Ok(out)
            } else {
                Ok(vec![CapabilityDoc {
                    source: source.to_owned(),
                    value: Value::Object(map),
                }])
            }
        }
        _ => Err(format!(
            "{source}: capability file must be an object or array"
        )),
    }
}

fn collect_config_capabilities(
    conf: &Value,
    file_identifiers: &BTreeSet<String>,
) -> Result<Vec<CapabilityDoc>, String> {
    let Some(sec_caps) = conf["app"]["security"].get("capabilities") else {
        return Ok(Vec::new());
    };
    if sec_caps.is_null() {
        return Ok(Vec::new());
    }
    let arr = sec_caps
        .as_array()
        .ok_or("tauri.conf.json app.security.capabilities must be an array")?;
    let mut inline = Vec::new();
    let mut includes_default = false;
    for (idx, entry) in arr.iter().enumerate() {
        match entry {
            Value::String(id) => {
                if id.trim().is_empty() {
                    return Err(format!(
                        "tauri.conf.json app.security.capabilities[{idx}] is empty"
                    ));
                }
                if !file_identifiers.contains(id.as_str()) {
                    return Err(format!(
                        "tauri.conf.json app.security.capabilities[{idx}] references unknown capability '{id}'"
                    ));
                }
                if id == "default" {
                    includes_default = true;
                }
            }
            Value::Object(_) => {
                let id = entry
                    .get("identifier")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        format!(
                            "tauri.conf.json app.security.capabilities[{idx}] missing string identifier"
                        )
                    })?;
                if id == "default" {
                    includes_default = true;
                }
                inline.push(CapabilityDoc {
                    source: format!("tauri.conf.json#app.security.capabilities[{idx}]"),
                    value: entry.clone(),
                });
            }
            _ => {
                return Err(format!(
                    "tauri.conf.json app.security.capabilities[{idx}] must be a string or capability object"
                ));
            }
        }
    }
    if !includes_default {
        return Err("tauri.conf.json app.security.capabilities must include 'default'".to_owned());
    }
    Ok(inline)
}

fn load_all_capabilities(conf: &Value) -> Result<(Vec<CapabilityDoc>, CapabilityDoc), String> {
    let root = manifest_dir();
    let cap_dir = root.join("capabilities");
    let default_path = cap_dir.join("default.json");
    let mut files = Vec::new();
    collect_capability_files(&cap_dir, &mut files)?;

    let mut docs = Vec::new();
    let mut file_identifiers = BTreeSet::new();
    let mut default_doc: Option<CapabilityDoc> = None;

    for path in files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let parsed = read_json(&path)?;
        for doc in extract_capabilities_from_value(&rel, parsed)? {
            let id = doc
                .value
                .get("identifier")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| format!("{}: capability missing string identifier", doc.source))?;
            file_identifiers.insert(id.to_owned());
            if path == default_path && id == "default" {
                if default_doc.is_some() {
                    return Err(format!("{rel}: duplicate 'default' capability"));
                }
                default_doc = Some(doc.clone());
            }
            docs.push(doc);
        }
    }

    let default_doc =
        default_doc.ok_or("capabilities/default.json must define the 'default' capability")?;
    docs.extend(collect_config_capabilities(conf, &file_identifiers)?);
    Ok((docs, default_doc))
}

fn parse_permissions<'a>(cap: &'a Value, source: &str) -> Result<Vec<&'a str>, String> {
    let arr = cap
        .get("permissions")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{source}: capability missing permissions array"))?;
    let mut ids = Vec::with_capacity(arr.len());
    for (idx, entry) in arr.iter().enumerate() {
        match entry {
            Value::String(s) => {
                if s.trim().is_empty() {
                    return Err(format!("{source}: permissions[{idx}] is empty"));
                }
                ids.push(s.as_str());
            }
            Value::Object(obj) => {
                let id = obj
                    .get("identifier")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        format!("{source}: permissions[{idx}] object missing string identifier")
                    })?;
                ids.push(id);
            }
            _ => {
                return Err(format!(
                    "{source}: permissions[{idx}] must be a string or an object with identifier"
                ));
            }
        }
    }
    Ok(ids)
}

fn validate_windows_config(conf: &Value) -> Result<(), String> {
    let windows = conf["app"]["windows"]
        .as_array()
        .ok_or("tauri.conf.json app.windows must be an array")?;
    if windows.len() != 1 {
        return Err(format!(
            "expected exactly 1 window in tauri.conf.json app.windows, found {}",
            windows.len()
        ));
    }
    for (idx, window) in windows.iter().enumerate() {
        if idx == 0 && window.get("label").and_then(Value::as_str) != Some("main") {
            return Err("app.windows[0] label must be 'main'".to_owned());
        }
        if window.get("maximizable") != Some(&Value::Bool(false)) {
            return Err(format!("app.windows[{idx}] must set maximizable: false"));
        }
        if window.get("fullscreen") != Some(&Value::Bool(false)) {
            return Err(format!("app.windows[{idx}] must set fullscreen: false"));
        }
        if !window.get("maximized").is_none_or(|v| v == false) {
            return Err(format!("app.windows[{idx}] must not set maximized: true"));
        }
        if window.get("create") != Some(&Value::Bool(false)) {
            return Err(format!(
                "app.windows[{idx}] must set create: false so .setup() assigns the portable WebView2 data directory"
            ));
        }
    }
    Ok(())
}

fn validate_window_capabilities(
    docs: &[CapabilityDoc],
    default_doc: &CapabilityDoc,
) -> Result<(), String> {
    let default_windows = default_doc
        .value
        .get("windows")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: missing windows array", default_doc.source))?;
    if default_windows.len() != 1 || default_windows[0].as_str() != Some("main") {
        return Err(format!(
            "{}: default capability must target [\"main\"], got {default_windows:?}",
            default_doc.source
        ));
    }

    let default_perms = parse_permissions(&default_doc.value, &default_doc.source)?;
    for denied in [
        "core:window:deny-internal-toggle-maximize",
        "core:window:deny-maximize",
        "core:window:deny-toggle-maximize",
        "core:window:deny-set-fullscreen",
    ] {
        if !default_perms.contains(&denied) {
            return Err(format!("{}: {denied} missing", default_doc.source));
        }
    }
    for granted in [
        "core:window:allow-minimize",
        "core:window:allow-close",
        "core:window:allow-start-dragging",
    ] {
        if !default_perms.contains(&granted) {
            return Err(format!("{}: {granted} missing", default_doc.source));
        }
    }

    for doc in docs {
        let perms = parse_permissions(&doc.value, &doc.source)?;
        let normalized: Vec<String> = perms
            .iter()
            .map(|p| p.trim().to_ascii_lowercase())
            .collect();
        for perm in &normalized {
            if FORBIDDEN_WINDOW_PERMS.contains(&perm.as_str()) {
                return Err(format!(
                    "{}: forbidden window permission '{perm}'",
                    doc.source
                ));
            }
        }
        let uses_default_window_set = normalized
            .iter()
            .any(|p| p == "core:default" || p == "core:window:default");
        if uses_default_window_set
            && !normalized
                .iter()
                .any(|p| p == "core:window:deny-internal-toggle-maximize")
        {
            return Err(format!(
                "{}: core:default/core:window:default requires core:window:deny-internal-toggle-maximize",
                doc.source
            ));
        }
    }
    Ok(())
}

fn validate_opener_absence(docs: &[CapabilityDoc], package: &Value) -> Result<(), String> {
    for doc in docs {
        let perms = parse_permissions(&doc.value, &doc.source)?;
        for perm in perms {
            let lower = perm.trim().to_ascii_lowercase();
            if lower.starts_with("opener:") || lower == "opener" || lower == "*" {
                return Err(format!(
                    "{}: no opener permission may be granted to the frontend (found '{perm}')",
                    doc.source
                ));
            }
        }
    }
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if package
            .get(section)
            .and_then(|s| s.get("@tauri-apps/plugin-opener"))
            .is_some()
        {
            return Err(format!(
                "package.json {section} must not include @tauri-apps/plugin-opener"
            ));
        }
    }
    Ok(())
}

fn validate_csp(csp: &str) -> Result<(), String> {
    let expected: [(&str, &[&str]); 10] = [
        ("default-src", &["'self'"]),
        ("script-src", &["'self'"]),
        ("style-src", &["'self'", "'unsafe-inline'"]),
        ("img-src", &["'self'", "data:"]),
        ("font-src", &["'self'"]),
        ("connect-src", &["ipc:", "http://ipc.localhost"]),
        ("object-src", &["'none'"]),
        ("base-uri", &["'none'"]),
        ("form-action", &["'none'"]),
        ("frame-ancestors", &["'none'"]),
    ];

    let mut directives: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut seen_lower: BTreeSet<String> = BTreeSet::new();

    for segment in csp.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let mut parts = segment.split_whitespace();
        let name = parts
            .next()
            .ok_or_else(|| format!("empty CSP directive in {csp}"))?;
        let name_lower = name.to_ascii_lowercase();
        if !seen_lower.insert(name_lower) {
            return Err(format!("duplicate CSP directive '{name}' in {csp}"));
        }
        if !expected.iter().any(|(approved, _)| *approved == name) {
            return Err(format!("unapproved CSP directive '{name}' in {csp}"));
        }
        let sources: Vec<&str> = parts.collect();
        if sources.is_empty() {
            return Err(format!("CSP directive '{name}' has no sources in {csp}"));
        }

        for &token in &sources {
            if name == "connect-src" && token == "http://ipc.localhost" {
                continue;
            }
            let unquoted = token.trim_matches('\'').to_ascii_lowercase();
            if matches!(
                unquoted.as_str(),
                "unsafe-eval" | "wasm-unsafe-eval" | "unsafe-hashes"
            ) {
                return Err(format!("the CSP must not contain {token}: {csp}"));
            }
            if unquoted == "unsafe-inline" && name != "style-src" {
                return Err(format!(
                    "only style-src may use 'unsafe-inline' (found in {name}): {csp}"
                ));
            }
            if unquoted.contains("://") || unquoted.contains('*') {
                return Err(format!(
                    "the CSP must not contain URLs or wildcards ({token}): {csp}"
                ));
            }
            for scheme in ["wss:", "https:", "http:", "ws:", "blob:", "filesystem:"] {
                if unquoted.contains(scheme) {
                    return Err(format!(
                        "the CSP must not contain scheme {scheme} ({token}): {csp}"
                    ));
                }
            }
            if unquoted.contains("data:") && name != "img-src" {
                return Err(format!(
                    "only img-src may use data: (found in {name}): {csp}"
                ));
            }
        }

        directives.insert(name, sources);
    }

    for (name, pinned) in expected {
        let actual = directives
            .get(name)
            .ok_or_else(|| format!("CSP has no {name} directive: {csp}"))?;
        if actual.as_slice() != pinned {
            return Err(format!(
                "CSP directive {name} must be {pinned:?}, got {actual:?} in {csp}"
            ));
        }
    }
    Ok(())
}

fn validate_browser_args(args: &str) -> Result<(), String> {
    let tokens: Vec<&str> = args.split_whitespace().collect();

    for flag_name in [
        "disable-features",
        "disk-cache-size",
        "media-cache-size",
        "js-flags",
    ] {
        let eq_prefix = format!("{flag_name}=");
        let count = tokens
            .iter()
            .filter(|t| {
                let norm = t
                    .strip_prefix("--")
                    .or_else(|| t.strip_prefix('-'))
                    .unwrap_or("");
                norm == flag_name || norm.starts_with(&eq_prefix)
            })
            .count();
        if count != 1 {
            return Err(format!(
                "expected --{flag_name} flag exactly once in additionalBrowserArgs (found {count}): {args}"
            ));
        }
    }

    let disabled = tokens
        .iter()
        .find_map(|t| t.strip_prefix("--disable-features="))
        .ok_or_else(|| format!("retains --disable-features= in {args}"))?;
    let features: Vec<&str> = disabled.split(',').collect();
    for required in ["msWebOOUI", "msPdfOOUI", "msSmartScreenProtection"] {
        if !features.contains(&required) {
            return Err(format!(
                "missing wry default disabled feature {required} in {args}"
            ));
        }
    }

    for flag in [
        "--disk-cache-size=16777216",
        "--media-cache-size=4194304",
        "--js-flags=--max-old-space-size=128",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-domain-reliability",
        "--disable-sync",
        "--disable-speech-api",
        "--no-pings",
    ] {
        let count = tokens.iter().filter(|t| **t == flag).count();
        if count != 1 {
            return Err(format!(
                "missing or duplicated browser memory/background cap {flag} in {args}"
            ));
        }
        if !flag.contains('=') {
            let base = flag.trim_start_matches('-');
            let eq_prefix = format!("{base}=");
            if tokens.iter().any(|t| {
                let norm = t
                    .strip_prefix("--")
                    .or_else(|| t.strip_prefix('-'))
                    .unwrap_or("");
                norm.starts_with(&eq_prefix)
            }) {
                return Err(format!("unexpected overriding flag for {flag} in {args}"));
            }
        }
    }

    Ok(())
}

#[test]
fn the_main_window_is_configured_as_not_maximizable_and_not_fullscreen() {
    let conf = read("tauri.conf.json");
    validate_windows_config(&conf).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_capability_denies_maximize_and_fullscreen_commands() {
    let conf = read("tauri.conf.json");
    let (docs, default_doc) = load_all_capabilities(&conf).unwrap_or_else(|e| panic!("{e}"));
    validate_window_capabilities(&docs, &default_doc).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_webview_has_a_strict_content_security_policy() {
    let conf = read("tauri.conf.json");
    assert_eq!(conf["app"]["security"]["freezePrototype"], true);
    let csp = conf["app"]["security"]["csp"]
        .as_str()
        .expect("a CSP string is configured");
    validate_csp(csp).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_webview_caps_caches_and_v8_heap_while_retaining_wry_default_disabled_features() {
    let conf = read("tauri.conf.json");
    let windows = conf["app"]["windows"]
        .as_array()
        .expect("app.windows is an array");
    assert_eq!(windows.len(), 1, "expected exactly 1 configured window");
    for window in windows {
        let args = window["additionalBrowserArgs"]
            .as_str()
            .expect("additionalBrowserArgs is configured");
        validate_browser_args(args).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn the_frontend_has_no_generic_url_or_path_opening_capability() {
    // Opening folders, logs and the sign-in page is done by Rust itself with fixed locations; the
    // page must not be able to ask the opener plugin to open anything.
    let conf = read("tauri.conf.json");
    let (docs, _) = load_all_capabilities(&conf).unwrap_or_else(|e| panic!("{e}"));
    let package = read("../package.json");
    validate_opener_absence(&docs, &package).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn csp_guard_rejects_semicolon_origin_and_subdirective_bypasses() {
    let base = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
    assert!(validate_csp(base).is_ok());

    let bad_cases = [
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline' https:; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https://evil.com; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: *.evil.com; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost wss://evil.com; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; worker-src *;",
        "default-src 'self'; script-src 'self'; script-src-elem 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; script-src-attr 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; style-src-elem https://evil.com; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline' blob:; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline' filesystem:; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; default-src *;",
    ];
    for bad in bad_cases {
        assert!(validate_csp(bad).is_err(), "CSP should be rejected: {bad}");
    }
}

#[test]
fn capability_guards_reject_object_permissions_inline_capabilities_and_extra_window_grants() {
    let default_doc = CapabilityDoc {
        source: "capabilities/default.json".to_owned(),
        value: serde_json::json!({
            "identifier": "default",
            "windows": ["main"],
            "permissions": [
                "core:default",
                "core:window:allow-minimize",
                "core:window:allow-close",
                "core:window:allow-start-dragging",
                "core:window:deny-internal-toggle-maximize",
                "core:window:deny-maximize",
                "core:window:deny-toggle-maximize",
                "core:window:deny-set-fullscreen"
            ]
        }),
    };
    let empty_pkg = serde_json::json!({});

    // Object-form opener permission must be caught.
    let object_opener = CapabilityDoc {
        source: "capabilities/extra.json".to_owned(),
        value: serde_json::json!({
            "identifier": "extra",
            "windows": ["main"],
            "permissions": [
                { "identifier": "opener:allow-open-url", "allow": [{ "url": "https://*" }] }
            ]
        }),
    };
    assert!(validate_opener_absence(&[default_doc.clone(), object_opener], &empty_pkg).is_err());

    // Malformed permission entry must not be silently ignored.
    let malformed_perm = CapabilityDoc {
        source: "capabilities/extra.json".to_owned(),
        value: serde_json::json!({
            "identifier": "extra",
            "windows": ["main"],
            "permissions": [{ "allow": ["https://*"] }]
        }),
    };
    assert!(validate_opener_absence(&[malformed_perm], &empty_pkg).is_err());

    // Object-form and string-form forbidden window permissions must be rejected across all capabilities.
    for perm in [
        serde_json::json!({ "identifier": "core:window:allow-maximize" }),
        serde_json::json!("core:window:allow-set-maximizable"),
        serde_json::json!("core:window:allow-set-fullscreen-on-monitor"),
        serde_json::json!("core:window:allow-set-simple-fullscreen"),
    ] {
        let extra = CapabilityDoc {
            source: "capabilities/extra.json".to_owned(),
            value: serde_json::json!({
                "identifier": "extra",
                "windows": ["main"],
                "permissions": [perm]
            }),
        };
        assert!(validate_window_capabilities(&[default_doc.clone(), extra], &default_doc).is_err());
    }

    // Inline capability in tauri.conf.json must be extracted and validated.
    let inline_conf = serde_json::json!({
        "app": {
            "security": {
                "capabilities": [
                    "default",
                    {
                        "identifier": "inline",
                        "windows": ["main"],
                        "permissions": [
                            { "identifier": "opener:allow-open-path", "allow": [{ "path": "$HOME/*" }] }
                        ]
                    }
                ]
            }
        }
    });
    let file_ids = BTreeSet::from(["default".to_owned()]);
    let inline = collect_config_capabilities(&inline_conf, &file_ids).unwrap();
    assert!(validate_opener_absence(&inline, &empty_pkg).is_err());

    // Default capability must target "main".
    let wrong_target = CapabilityDoc {
        source: "capabilities/default.json".to_owned(),
        value: serde_json::json!({
            "identifier": "default",
            "windows": ["*"],
            "permissions": default_doc.value["permissions"].clone()
        }),
    };
    assert!(
        validate_window_capabilities(std::slice::from_ref(&wrong_target), &wrong_target).is_err()
    );

    // Array and `{ "capabilities": [...] }` file forms must also be parsed and validated.
    let wrapped = serde_json::json!({
        "capabilities": [{
            "identifier": "wrapped",
            "windows": ["main"],
            "permissions": ["core:window:default"]
        }]
    });
    let wrapped_docs =
        extract_capabilities_from_value("capabilities/wrapped.json", wrapped).unwrap();
    assert!(validate_window_capabilities(&wrapped_docs, &default_doc).is_err());

    let array_docs = extract_capabilities_from_value(
        "capabilities/array.json",
        serde_json::json!([{
            "identifier": "array-item",
            "windows": ["main"],
            "permissions": [{ "identifier": "opener:allow-reveal-item-in-dir" }]
        }]),
    )
    .unwrap();
    assert!(validate_opener_absence(&array_docs, &empty_pkg).is_err());
}

#[test]
fn window_and_browser_args_guards_reject_create_true_extra_windows_and_duplicate_flags() {
    let missing_create = serde_json::json!({
        "app": {
            "windows": [{
                "label": "main",
                "maximizable": false,
                "fullscreen": false
            }]
        }
    });
    assert!(validate_windows_config(&missing_create).is_err());

    let create_true = serde_json::json!({
        "app": {
            "windows": [{
                "label": "main",
                "maximizable": false,
                "fullscreen": false,
                "create": true
            }]
        }
    });
    assert!(validate_windows_config(&create_true).is_err());

    let extra_window = serde_json::json!({
        "app": {
            "windows": [
                { "label": "main", "maximizable": false, "fullscreen": false, "create": false },
                { "label": "aux", "maximizable": true, "fullscreen": false, "create": true }
            ]
        }
    });
    assert!(validate_windows_config(&extra_window).is_err());

    let base_args = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disk-cache-size=16777216 --media-cache-size=4194304 --js-flags=--max-old-space-size=128 --disable-background-networking --disable-component-update --disable-domain-reliability --disable-sync --disable-speech-api --no-pings";
    assert!(validate_browser_args(base_args).is_ok());

    for suffix in [
        " --disable-features=None",
        " --disk-cache-size=999999999",
        " --media-cache-size=999999999",
        " --js-flags=--max-old-space-size=4096",
    ] {
        let tampered = format!("{base_args}{suffix}");
        assert!(
            validate_browser_args(&tampered).is_err(),
            "duplicate flag should be rejected: {tampered}"
        );
    }
}
