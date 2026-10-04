//! Launcher-managed Java runtimes under `runtimes/java/<id>/`.
//!
//! Rule: Minecraft is only ever started with a runtime from this folder. `PATH`,
//! `JAVA_HOME`, the registry and `Program Files` are never consulted, so a PC with no Java
//! (or the wrong Java) behaves exactly like the PC the stick was prepared on.
//!
//! Each runtime directory holds a `runtime.json` written by the installer *after* the
//! download was verified. Before use, the executable is re-hashed against it, so a swapped or
//! corrupted `javaw.exe` is refused rather than executed.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::portable::fsutil::{self, Expected};
use crate::portable::{PortableRoot, RelPath};

const MANIFEST_NAME: &str = "runtime.json";

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
    let manifest_bytes = match fs::read(dir.join(MANIFEST_NAME)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(RuntimeError::Missing(id.to_owned()))
        }
        Err(e) => return Err(e.into()),
    };
    let m: RuntimeManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| RuntimeError::Manifest {
            id: id.to_owned(),
            reason: e.to_string(),
        })?;
    if m.id != id || m.arch != "x64" {
        return Err(RuntimeError::Manifest {
            id: id.to_owned(),
            reason: "id or architecture mismatch".into(),
        });
    }
    let exe = m.java_exe.to_path(&dir);
    fsutil::reject_links_between(&dir, &exe).map_err(|_| RuntimeError::Integrity(id.to_owned()))?;
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

pub fn write_manifest(root: &PortableRoot, m: &RuntimeManifest) -> Result<(), RuntimeError> {
    let dir = runtime_dir(root, &m.id)?;
    fs::create_dir_all(&dir)?;
    Ok(fsutil::atomic_write(
        &dir.join(MANIFEST_NAME),
        &serde_json::to_vec_pretty(m).map_err(io::Error::other)?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
