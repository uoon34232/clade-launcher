//! Launcher-managed Java runtimes under `runtimes/java/<id>/`.
//!
//! Rule: Minecraft is only ever started with a runtime from this folder. `PATH`,
//! `JAVA_HOME`, the registry and `Program Files` are never consulted, so a PC with no Java
//! (or the wrong Java) behaves exactly like the PC the stick was prepared on.
//!
//! Each runtime directory holds a `runtime.json` written by the installer *after* the
//! download was verified. Before use, the executable is re-hashed against it, so a swapped or
//! corrupted `javaw.exe` (or `java.exe` for headless CLI tasks) is refused rather than executed.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::portable::fsutil::{self, Expected};
use crate::portable::{PortableRoot, RelPath};

const MANIFEST_NAME: &str = "runtime.json";
const MAX_RUNTIME_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeManifest {
    pub schema: u32,
    /// Directory name, e.g. `temurin-21.0.5+11-windows-x64`.
    pub id: String,
    pub vendor: String,
    pub java_major: u32,
    /// Only 64-bit Windows is targeted first.
    pub arch: String,
    /// Path of the Java executable relative to this runtime directory.
    pub java_exe: RelPath,
    /// SHA-256 of the executable, recorded after the archive itself was verified.
    pub java_exe_sha256: String,
    /// Optional console executable (`bin/java.exe`) when `java_exe` is `bin/javaw.exe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_cli_exe: Option<RelPath>,
    /// SHA-256 of `java_cli_exe`, recorded at install time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_cli_exe_sha256: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("Java runtime '{0}' is not installed in the launcher folder")]
    Missing(String),
    #[error("runtime manifest for '{id}' is invalid: {reason}")]
    Manifest { id: String, reason: String },
    #[error("Java executable of runtime '{0}' failed its integrity check; reinstall the runtime")]
    Integrity(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// A runtime that exists, is inside the root, and passed its integrity check.
#[derive(Debug, Clone)]
pub struct ManagedJava {
    pub id: String,
    pub java_major: u32,
    pub exe: PathBuf,
    pub home: PathBuf,
}

pub fn runtime_dir(root: &PortableRoot, id: &str) -> Result<PathBuf, RuntimeError> {
    let rel = RelPath::new(id).map_err(|e| RuntimeError::Manifest {
        id: id.to_owned(),
        reason: e.to_string(),
    })?;
    if rel.components().count() != 1 {
        return Err(RuntimeError::Manifest {
            id: id.to_owned(),
            reason: "runtime id must be a single directory name".into(),
        });
    }
    Ok(rel.to_path(&root.java_runtimes_dir()))
}

/// The one and only way to obtain a Java executable for launching the game.
pub fn resolve_java(root: &PortableRoot, id: &str) -> Result<ManagedJava, RuntimeError> {
    let dir = runtime_dir(root, id)?;
    let manifest_path = dir.join(MANIFEST_NAME);
    fsutil::reject_links_between(root.path(), &manifest_path)
        .map_err(|_| RuntimeError::Integrity(id.to_owned()))?;
    let manifest_bytes = match fsutil::read_bounded(&manifest_path, MAX_RUNTIME_MANIFEST_BYTES) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(RuntimeError::Missing(id.to_owned()))
        }
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            return Err(RuntimeError::Manifest {
                id: id.to_owned(),
                reason: e.to_string(),
            })
        }
        Err(e) => return Err(e.into()),
    };
    let m: RuntimeManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| RuntimeError::Manifest {
            id: id.to_owned(),
            reason: e.to_string(),
        })?;
    if m.schema != 1 {
        return Err(RuntimeError::Manifest {
            id: id.to_owned(),
            reason: format!("unsupported runtime manifest schema {}", m.schema),
        });
    }
    if m.id != id || m.arch != "x64" {
        return Err(RuntimeError::Manifest {
            id: id.to_owned(),
            reason: "id or architecture mismatch".into(),
        });
    }
    let exe = m.java_exe.to_path(&dir);
    fsutil::reject_links_between(root.path(), &exe)
        .map_err(|_| RuntimeError::Integrity(id.to_owned()))?;
    if !exe.is_file() {
        return Err(RuntimeError::Missing(id.to_owned()));
    }
    fsutil::verify_file(&exe, &Expected::Sha256(m.java_exe_sha256.clone()), None)
        .map_err(|_| RuntimeError::Integrity(id.to_owned()))?;
    Ok(ManagedJava {
        id: m.id,
        java_major: m.java_major,
        exe,
        home: dir,
    })
}

/// Resolves a verified console `java.exe` for headless CLI tasks (such as loader installers)
/// when the runtime's primary launch binary is `javaw.exe`.
pub fn resolve_cli_java_exe(
    root: &PortableRoot,
    java: &ManagedJava,
) -> Result<PathBuf, RuntimeError> {
    fsutil::reject_links_between(root.path(), &java.exe)
        .map_err(|_| RuntimeError::Integrity(java.id.clone()))?;
    if java
        .exe
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("java.exe"))
    {
        return Ok(java.exe.clone());
    }

    let manifest_path = java.home.join(MANIFEST_NAME);
    fsutil::reject_links_between(root.path(), &manifest_path)
        .map_err(|_| RuntimeError::Integrity(java.id.clone()))?;
    match fsutil::read_bounded(&manifest_path, MAX_RUNTIME_MANIFEST_BYTES) {
        Ok(manifest_bytes) => {
            let m: RuntimeManifest =
                serde_json::from_slice(&manifest_bytes).map_err(|e| RuntimeError::Manifest {
                    id: java.id.clone(),
                    reason: e.to_string(),
                })?;
            if m.schema != 1 {
                return Err(RuntimeError::Manifest {
                    id: java.id.clone(),
                    reason: format!("unsupported runtime manifest schema {}", m.schema),
                });
            }
            match (&m.java_cli_exe, &m.java_cli_exe_sha256) {
                (Some(rel), Some(sha)) => {
                    let cli = rel.to_path(&java.home);
                    fsutil::reject_links_between(root.path(), &cli)
                        .map_err(|_| RuntimeError::Integrity(java.id.clone()))?;
                    fsutil::verify_file(&cli, &Expected::Sha256(sha.clone()), None)
                        .map_err(|_| RuntimeError::Integrity(java.id.clone()))?;
                    return Ok(cli);
                }
                (None, None) => {}
                _ => {
                    return Err(RuntimeError::Manifest {
                        id: java.id.clone(),
                        reason: "incomplete java_cli_exe metadata".into(),
                    });
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            return Err(RuntimeError::Manifest {
                id: java.id.clone(),
                reason: e.to_string(),
            });
        }
        Err(e) => return Err(e.into()),
    }

    if let Some(rf) = crate::java_install::recorded_files(root, &java.id).and_then(|files| {
        files
            .into_iter()
            .find(|f| f.path.as_str() == "bin/java.exe")
    }) {
        let cli = rf.path.to_path(&java.home);
        fsutil::reject_links_between(root.path(), &cli)
            .map_err(|_| RuntimeError::Integrity(java.id.clone()))?;
        fsutil::verify_file(
            &cli,
            &Expected::Sha1(rf.download.sha1),
            Some(rf.download.size),
        )
        .map_err(|_| RuntimeError::Integrity(java.id.clone()))?;
        return Ok(cli);
    }

    fsutil::reject_links_between(root.path(), &java.exe)
        .map_err(|_| RuntimeError::Integrity(java.id.clone()))?;
    Ok(java.exe.clone())
}

pub fn write_manifest(root: &PortableRoot, m: &RuntimeManifest) -> Result<(), RuntimeError> {
    let dir = runtime_dir(root, &m.id)?;
    fsutil::reject_links_between(root.path(), &dir)
        .map_err(|_| RuntimeError::Integrity(m.id.clone()))?;
    fs::create_dir_all(&dir)?;
    Ok(fsutil::atomic_write(
        &dir.join(MANIFEST_NAME),
        &serde_json::to_vec_pretty(m).map_err(io::Error::other)?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::Sha1;
    use sha2::{Digest, Sha256};

    fn setup(tamper: bool) -> (tempfile::TempDir, PortableRoot, RuntimeManifest) {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        let id = "temurin-21-windows-x64";
        let exe_bytes = b"MZ fake javaw";
        let dir = runtime_dir(&root, id).unwrap();
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(
            dir.join("bin").join("javaw.exe"),
            if tamper {
                &b"MZ evil javaw"[..]
            } else {
                &exe_bytes[..]
            },
        )
        .unwrap();
        let m = RuntimeManifest {
            schema: 1,
            id: id.into(),
            vendor: "Eclipse Temurin".into(),
            java_major: 21,
            arch: "x64".into(),
            java_exe: RelPath::new("bin/javaw.exe").unwrap(),
            java_exe_sha256: hex::encode(Sha256::digest(exe_bytes)),
            java_cli_exe: None,
            java_cli_exe_sha256: None,
        };
        write_manifest(&root, &m).unwrap();
        (d, root, m)
    }

    #[test]
    fn resolves_a_verified_runtime_inside_the_root() {
        let (d, root, m) = setup(false);
        let j = resolve_java(&root, &m.id).unwrap();
        assert!(j.exe.starts_with(d.path()));
        assert_eq!(j.java_major, 21);
    }

    #[test]
    fn missing_runtime_is_an_error_never_a_system_fallback() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        assert!(matches!(
            resolve_java(&root, "temurin-17-windows-x64"),
            Err(RuntimeError::Missing(_))
        ));
    }

    #[test]
    fn modified_executable_is_refused() {
        let (_d, root, m) = setup(true);
        assert!(matches!(
            resolve_java(&root, &m.id),
            Err(RuntimeError::Integrity(_))
        ));
    }

    #[test]
    fn runtime_ids_cannot_escape() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        for bad in ["../x", "a/b", "C:/Java", "..", "con"] {
            assert!(runtime_dir(&root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unknown_schema_and_oversized_manifests_are_rejected() {
        let (_d, root, mut m) = setup(false);
        for bad_schema in [0, 2, 99] {
            m.schema = bad_schema;
            write_manifest(&root, &m).unwrap();
            assert!(matches!(
                resolve_java(&root, &m.id),
                Err(RuntimeError::Manifest { .. })
            ));
        }

        let dir = runtime_dir(&root, &m.id).unwrap();
        fs::write(dir.join(MANIFEST_NAME), vec![b' '; 70 * 1024]).unwrap();
        assert!(matches!(
            resolve_java(&root, &m.id),
            Err(RuntimeError::Manifest { .. })
        ));
    }

    #[test]
    fn resolve_cli_java_exe_verifies_console_binary_and_handles_fallbacks() {
        let (_d, root, mut m) = setup(false);
        let dir = runtime_dir(&root, &m.id).unwrap();
        let java = resolve_java(&root, &m.id).unwrap();

        // 1. No CLI binary recorded at all: falls back to verified javaw.exe.
        assert_eq!(resolve_cli_java_exe(&root, &java).unwrap(), java.exe);

        // 2. Legacy runtime-files.json fallback: verifies bin/java.exe by SHA-1 and size.
        let cli_bytes = b"MZ fake console java";
        let cli_path = dir.join("bin").join("java.exe");
        fs::write(&cli_path, cli_bytes).unwrap();
        let cli_sha1 = hex::encode(Sha1::digest(cli_bytes));
        fs::write(
            dir.join("runtime-files.json"),
            serde_json::json!({
                "schema": 1,
                "files": [{
                    "path": "bin/java.exe",
                    "url": "https://piston-data.mojang.com/v1/objects/x/java.exe",
                    "sha1": cli_sha1,
                    "size": cli_bytes.len()
                }]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(resolve_cli_java_exe(&root, &java).unwrap(), cli_path);

        // Tampering bin/java.exe fails closed under the recorded_files fallback.
        fs::write(&cli_path, b"MZ tampered").unwrap();
        assert!(matches!(
            resolve_cli_java_exe(&root, &java),
            Err(RuntimeError::Integrity(_))
        ));
        fs::write(&cli_path, cli_bytes).unwrap();

        // 3. Explicit java_cli_exe + java_cli_exe_sha256 in runtime.json takes precedence.
        m.java_cli_exe = Some(RelPath::new("bin/java.exe").unwrap());
        m.java_cli_exe_sha256 = Some(hex::encode(Sha256::digest(cli_bytes)));
        write_manifest(&root, &m).unwrap();
        assert_eq!(resolve_cli_java_exe(&root, &java).unwrap(), cli_path);

        fs::write(&cli_path, b"MZ tampered sha256").unwrap();
        assert!(matches!(
            resolve_cli_java_exe(&root, &java),
            Err(RuntimeError::Integrity(_))
        ));

        // 4. Synthetic unit-test ManagedJava without runtime.json returns java.exe inside root.
        let synth_home = root.java_runtimes_dir().join("synth-jre");
        let synth = ManagedJava {
            id: "synth-jre".into(),
            java_major: 21,
            exe: synth_home.join("bin").join("java.exe"),
            home: synth_home,
        };
        assert_eq!(resolve_cli_java_exe(&root, &synth).unwrap(), synth.exe);
    }

    #[cfg(windows)]
    #[test]
    fn symlinked_manifest_is_rejected_by_resolve_java_and_cli_resolver() {
        let (d, root, m) = setup(false);
        let java = resolve_java(&root, &m.id).unwrap();
        let dir = runtime_dir(&root, &m.id).unwrap();
        let manifest_path = dir.join(MANIFEST_NAME);
        let outside = d.path().join("outside-runtime.json");
        fs::copy(&manifest_path, &outside).unwrap();
        fs::remove_file(&manifest_path).unwrap();

        match std::os::windows::fs::symlink_file(&outside, &manifest_path) {
            Ok(()) => {}
            Err(e)
                if e.raw_os_error() == Some(1314)
                    || e.kind() == io::ErrorKind::PermissionDenied =>
            {
                return;
            }
            Err(e) => panic!("unexpected symlink_file error: {e}"),
        }

        assert!(matches!(
            resolve_java(&root, &m.id),
            Err(RuntimeError::Integrity(_))
        ));
        assert!(matches!(
            resolve_cli_java_exe(&root, &java),
            Err(RuntimeError::Integrity(_))
        ));
    }
}
