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
    let watches_icon = build.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("").trim();
        code.contains(r#"println!("cargo:rerun-if-changed=icons/icon.ico")"#)
    });
    assert!(
        watches_icon,
        "build.rs must emit rerun-if-changed for icons/icon.ico, otherwise a new icon never reaches Clade.exe"
    );
}

#[test]
fn every_configured_icon_exists() {
    let conf: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root().join("tauri.conf.json")).unwrap()).unwrap();
    let icons = conf["bundle"]["icon"].as_array().expect("bundle.icon");
    assert!(!icons.is_empty());
    assert!(
        icons.iter().any(|v| v.as_str() == Some("icons/icon.ico")),
        "bundle.icon must include icons/icon.ico for the Windows executable icon"
    );
    for i in icons {
        let rel = i.as_str().unwrap();
        assert!(root().join(rel).is_file(), "missing icon file {rel}");
    }
}

#[test]
fn the_windows_icon_has_the_small_sizes() {
    let ico = fs::read(root().join("icons/icon.ico")).unwrap();
    assert!(ico.len() >= 6, "icon.ico truncated before header");
    assert_eq!(&ico[0..4], &[0, 0, 1, 0], "not an .ico file");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let dir_end = 6 + 16 * count;
    assert!(
        ico.len() >= dir_end,
        "icon.ico truncated before directory end ({} < {dir_end})",
        ico.len()
    );
    let sizes: Vec<u32> = (0..count)
        .map(|i| {
            let entry = &ico[6 + 16 * i..6 + 16 * (i + 1)];
            let w = if entry[0] == 0 { 256 } else { entry[0] as u32 };
            let h = if entry[1] == 0 { 256 } else { entry[1] as u32 };
            assert_eq!(w, h, "icon.ico entry {i} must be square (got {w}x{h})");
            let bytes_in_res =
                u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
            let image_offset =
                u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
            assert!(bytes_in_res > 0, "icon.ico entry {i} ({w}x{h}) is empty");
            let image_end = image_offset
                .checked_add(bytes_in_res)
                .expect("icon.ico entry offset overflow");
            assert!(
                image_end <= ico.len(),
                "icon.ico entry {i} ({w}x{h}) out of bounds ({image_offset} + {bytes_in_res} > {})",
                ico.len()
            );
            w
        })
        .collect();
    for want in [16, 20, 24, 32, 40, 48, 64, 128, 256] {
        assert!(
            sizes.contains(&want),
            "icon.ico lacks the {want}px size (has {sizes:?})"
        );
    }
}
