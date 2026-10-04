//! Guards for the application icon: the build must notice icon changes, and the Windows icon must keep
//! the small sizes the shell asks for (a missing size makes Windows scale a bigger one and blur it).

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_build_script_watches_the_windows_icon() {
    let build = fs::read_to_string(root().join("build.rs")).unwrap();
    assert!(
        build.contains("cargo:rerun-if-changed=icons/icon.ico"),
        "build.rs must emit rerun-if-changed for icons/icon.ico, otherwise a new icon never reaches Clade.exe"
    );
}

#[test]
fn every_configured_icon_exists() {
    let conf: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root().join("tauri.conf.json")).unwrap()).unwrap();
    let icons = conf["bundle"]["icon"].as_array().expect("bundle.icon");
    assert!(!icons.is_empty());
    for i in icons {
        let rel = i.as_str().unwrap();
        assert!(root().join(rel).is_file(), "missing icon file {rel}");
    }
}

#[test]
fn the_windows_icon_has_the_small_sizes() {
    let ico = fs::read(root().join("icons/icon.ico")).unwrap();
    assert_eq!(&ico[0..4], &[0, 0, 1, 0], "not an .ico file");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let sizes: Vec<u32> = (0..count)
        .map(|i| {
            let w = ico[6 + 16 * i];
            if w == 0 {
                256
            } else {
                w as u32
            }
        })
        .collect();
    for want in [16, 24, 32, 48, 64, 256] {
        assert!(
            sizes.contains(&want),
            "icon.ico lacks the {want}px size (has {sizes:?})"
        );
    }
}
