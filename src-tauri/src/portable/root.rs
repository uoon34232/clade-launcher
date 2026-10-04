//! The portable root: the single directory everything lives in.
//!
//! In release builds it is the directory containing the launcher executable, resolved at
//! run time, so moving the folder to another drive letter, USB stick or PC changes nothing.
//! No environment variable, registry key, or user-profile folder is consulted.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use super::relpath::{RelPath, RelPathError};

/// Bump when the on-disk layout changes in a way older builds cannot read.
pub const LAYOUT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error("could not locate the launcher executable: {0}")]
    Exe(io::Error),
    #[error("the launcher folder is not writable ({0}). Is the USB stick write-protected?")]
    NotWritable(io::Error),
    #[error("path {0} is outside the launcher folder")]
    Outside(String),
    #[error("invalid stored path: {0}")]
    Rel(#[from] RelPathError),
    #[error(
        "another Clade Launcher instance is already using this folder (possibly on another PC)"
    )]
    AlreadyRunning,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Every directory below the root. One place so nothing invents its own location.
pub const DIRS: &[&str] = &[
    "data",
    "data/webview2", // WebView2 profile (cookies, cache); keeps it out of %LOCALAPPDATA%
    "data/playtime-active", // crash-recovery journal for running game sessions
    "instances",
    "runtimes",
    "runtimes/java",
    "minecraft",
    "libraries",
    "assets",
    "cache",
    "cache/tmp",
    "cache/downloads", // in-flight downloads, verified before being moved into place
    "cache/updates",   // staged launcher updates (never outside the folder)
    "logs",
];

#[derive(Debug, Clone)]
pub struct PortableRoot {
    root: PathBuf,
}

impl PortableRoot {
    /// Release: the folder holding the executable. Debug: `clade-root` next to the dev
    /// binary (inside `target/`, which is git-ignored) so development never pollutes the repo.
    pub fn discover() -> Result<Self, RootError> {
        let exe = std::env::current_exe().map_err(RootError::Exe)?;
        let dir = exe.parent().ok_or_else(|| {
            RootError::Exe(io::Error::other("executable has no parent directory"))
        })?;
        let root = if cfg!(debug_assertions) {
            dir.join("clade-root")
        } else {
            dir.to_path_buf()
        };
        Ok(Self {
            root: std::path::absolute(root).map_err(RootError::Exe)?,
        })
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Creates the standard layout and proves the folder is writable.
    pub fn ensure_layout(&self) -> Result<(), RootError> {
        for d in DIRS {
            fs::create_dir_all(RelPath::new(d)?.to_path(&self.root))
                .map_err(RootError::NotWritable)?;
        }
        let probe = self.data_dir().join(".write-test");
        fs::write(&probe, b"ok").map_err(RootError::NotWritable)?;
        let _ = fs::remove_file(probe);
        Ok(())
    }

    /// Resolves a stored relative path to an absolute one under this root.
    pub fn resolve(&self, rel: &RelPath) -> PathBuf {
        rel.to_path(&self.root)
    }

    /// Converts an absolute path back to its stored (relative) form. Fails for anything
    /// outside the root, so absolute paths can never leak into saved files.
    pub fn relativize(&self, abs: &Path) -> Result<RelPath, RootError> {
        let outside = || RootError::Outside(abs.display().to_string());
        let rest = abs.strip_prefix(&self.root).map_err(|_| outside())?;
        let parts: Vec<&str> = rest
            .components()
            .map(|c| c.as_os_str().to_str().ok_or_else(outside))
            .collect::<Result<_, _>>()?;
        Ok(RelPath::new(&parts.join("/"))?)
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }
    pub fn webview_data_dir(&self) -> PathBuf {
        self.root.join("data").join("webview2")
    }
    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }
    pub fn runtimes_dir(&self) -> PathBuf {
        self.root.join("runtimes")
    }
    pub fn java_runtimes_dir(&self) -> PathBuf {
        self.root.join("runtimes").join("java")
    }
    pub fn fixed_webview2_dir(&self) -> PathBuf {
        self.root.join("runtimes").join("webview2")
    }
    pub fn minecraft_dir(&self) -> PathBuf {
        self.root.join("minecraft")
    }
    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }
    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
    /// Java and the game are pointed here via `-Djava.io.tmpdir` instead of `%TEMP%`.
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("cache").join("tmp")
    }
    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("cache").join("downloads")
    }
    pub fn updates_dir(&self) -> PathBuf {
        self.root.join("cache").join("updates")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn settings_file(&self) -> PathBuf {
        self.data_dir().join("settings.json")
    }
    pub fn accounts_file(&self) -> PathBuf {
        self.data_dir().join("accounts.json")
    }
    pub fn vault_file(&self) -> PathBuf {
        self.data_dir().join("vault.bin")
    }
    pub fn lock_file(&self) -> PathBuf {
        self.data_dir().join("clade.lock")
    }
}

/// Exclusive lock on the folder. Prevents two launchers (even on two different PCs sharing
/// the stick over a network share) from corrupting the same files. The OS drops the lock if
/// the process dies or the stick is pulled, so there are no stale lock files to clean up.
#[derive(Debug)]
pub struct RootLock {
    _file: File,
}

impl RootLock {
    pub fn acquire(root: &PortableRoot) -> Result<Self, RootError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.lock_file())?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(RootError::AlreadyRunning),
            Err(std::fs::TryLockError::Error(e)) => Err(RootError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_created_and_relative_round_trip_works() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        for dir in DIRS {
            assert!(d.path().join(dir).is_dir(), "{dir}");
        }
        let rel = RelPath::new("instances/foo/mods").unwrap();
        let abs = root.resolve(&rel);
        assert!(abs.starts_with(d.path()));
        assert_eq!(root.relativize(&abs).unwrap(), rel);
    }

    #[test]
    fn moving_the_folder_keeps_stored_paths_valid() {
        let a = tempfile::tempdir().unwrap();
        let root_a = PortableRoot::at(a.path().join("Launcher"));
        root_a.ensure_layout().unwrap();
        let rel = RelPath::new("instances/x").unwrap();
        fs::create_dir_all(root_a.resolve(&rel)).unwrap();
        fs::write(root_a.resolve(&rel).join("options.txt"), b"fov:70").unwrap();

        // "Another drive letter / another PC": same content under a different absolute root.
        let b = tempfile::tempdir().unwrap();
        let root_b = PortableRoot::at(b.path().join("E_drive").join("Launcher"));
        fs::create_dir_all(root_b.path().parent().unwrap()).unwrap();
        fs::rename(root_a.path(), root_b.path()).unwrap();
        assert_eq!(
            fs::read(root_b.resolve(&rel).join("options.txt")).unwrap(),
            b"fov:70"
        );
    }

    #[test]
    fn relativize_rejects_outside_paths() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("Launcher"));
        assert!(root
            .relativize(Path::new("C:\\Users\\someone\\.minecraft"))
            .is_err());
        assert!(root.relativize(&d.path().join("elsewhere")).is_err());
    }

    #[test]
    fn second_lock_is_refused_until_first_dropped() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        let first = RootLock::acquire(&root).unwrap();
        assert!(matches!(
            RootLock::acquire(&root),
            Err(RootError::AlreadyRunning)
        ));
        drop(first);
        assert!(RootLock::acquire(&root).is_ok());
    }
}
