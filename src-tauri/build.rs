fn main() {
    // tauri-build compiles icons/icon.ico into the Windows resources: the Clade.exe icon, which is also
    // the window and taskbar icon. It does not tell Cargo to watch that file, so a changed icon silently
    // stayed old in the executable. (The other files in icons/ are bundle-only and never consumed here.)
    println!("cargo:rerun-if-changed=icons/icon.ico");
    let attrs = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows-app.manifest")),
    );
    tauri_build::try_build(attrs).expect("failed to run tauri-build");
}
