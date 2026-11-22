//! The portable root: the single directory everything lives in.
//!
//! In release builds it is the directory containing the launcher executable, resolved at
//! run time, so moving the folder to another drive letter, USB stick or PC changes nothing.
//! No environment variable, registry key, or user-profile folder is consulted.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::fsutil::{is_link, reject_links_between, remove_file_with_retry, strip_prefix_portable};
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
    "cache/tmp/prepare", // scratch space for preparation/extraction only (safe to sweep at start)
    "cache/tmp/game",    // -Djava.io.tmpdir for launched games (isolated from prepare sweeps)
    "cache/downloads",   // in-flight downloads, verified before being moved into place
    "cache/updates",     // staged launcher updates (never outside the folder)
    "logs",
];

/// Strips Windows verbatim prefixes (`\\?\UNC\server\share` -> `\\server\share`, and `<248`-char
/// `\\?\X:\...` -> `X:\...`) so child tools (Java 8, WebView2 Fixed Version, diagnostics redaction)
/// receive standard Win32 paths.
pub fn normalize_verbatim_path(p: PathBuf) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p;
    };
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    if let Some(drive) = s.strip_prefix(r"\\?\") {
        let b = drive.as_bytes();
        let is_drive_path =
            b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
        if is_drive_path && drive.encode_utf16().count() < 248 {
            return PathBuf::from(drive);
        }
    }
    p
}

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
        let abs = std::path::absolute(root).map_err(RootError::Exe)?;
        Ok(Self {
            root: normalize_verbatim_path(abs),
        })
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: normalize_verbatim_path(root.into()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Creates only `data/` (checking for reparse points) so `RootLock::acquire` can lock the
    /// folder before touching any shared layout files.
    pub fn ensure_data_dir(&self) -> Result<(), RootError> {
        if is_link(&self.root).unwrap_or(false) {
            return Err(RootError::NotWritable(io::Error::other(
                "launcher root cannot be a link or junction",
            )));
        }
        let data = self.data_dir();
        reject_links_between(&self.root, &data).map_err(RootError::NotWritable)?;
        fs::create_dir_all(&data).map_err(RootError::NotWritable)?;
        reject_links_between(&self.root, &data).map_err(RootError::NotWritable)?;
        Ok(())
    }

    /// Creates the standard layout, rejects junctions/symlinks, and proves the folder is writable
    /// using a unique temp probe so concurrent second-instance launches never collide.
    pub fn ensure_layout(&self) -> Result<(), RootError> {
        self.ensure_data_dir()?;
        for d in DIRS {
            let target = RelPath::new(d)?.to_path(&self.root);
            reject_links_between(&self.root, &target).map_err(RootError::NotWritable)?;
            fs::create_dir_all(&target).map_err(RootError::NotWritable)?;
            reject_links_between(&self.root, &target).map_err(RootError::NotWritable)?;
        }
        let mut nonce = [0u8; 6];
        getrandom::fill(&mut nonce)
            .map_err(|e| RootError::NotWritable(io::Error::other(e.to_string())))?;
        let probe = self
            .data_dir()
            .join(format!(".write-test-{}.tmp", hex::encode(nonce)));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(RootError::NotWritable)?;
        f.write_all(b"ok").map_err(RootError::NotWritable)?;
        drop(f);
        let _ = remove_file_with_retry(&probe);
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
        let rest = strip_prefix_portable(abs, &self.root).ok_or_else(outside)?;
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
    /// Legacy/general scratch root inside `cache/tmp`.
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("cache").join("tmp")
    }
    /// Scratch space exclusively for preparation/extraction (safe to sweep at startup).
    pub fn prepare_tmp_dir(&self) -> PathBuf {
        self.root.join("cache").join("tmp").join("prepare")
    }
    /// Isolated `-Djava.io.tmpdir` for launched Minecraft processes.
    pub fn game_tmp_dir(&self) -> PathBuf {
        self.root.join("cache").join("tmp").join("game")
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

fn is_not_writable_io_error(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 19))
        || matches!(
            e.kind(),
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
        )
}

fn map_lock_open_error(e: io::Error) -> RootError {
    if matches!(e.raw_os_error(), Some(32 | 33)) {
        RootError::AlreadyRunning
    } else if is_not_writable_io_error(&e) {
        RootError::NotWritable(e)
    } else {
        RootError::Io(e)
    }
}

impl RootLock {
    pub fn acquire(root: &PortableRoot) -> Result<Self, RootError> {
        root.ensure_data_dir()?;
        let lock_path = root.lock_file();
        match is_link(&lock_path) {
            Ok(true) => {
                return Err(RootError::NotWritable(io::Error::other(
                    "lock file cannot be a link or junction",
                )));
            }
            Ok(false) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) if is_not_writable_io_error(&e) => return Err(RootError::NotWritable(e)),
            Err(e) => return Err(RootError::Io(e)),
        }
        let mut opts = OpenOptions::new();
        opts.create(true).truncate(false).read(true).write(true);
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ | FILE_SHARE_WRITE (0x3), deliberately omitting FILE_SHARE_DELETE
            // so `clade.lock` cannot be unlinked or renamed away while held.
            opts.share_mode(0x0000_0001 | 0x0000_0002);
        }
        let file = opts.open(&lock_path).map_err(map_lock_open_error)?;
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
        assert!(abs.starts_with(root.path()));
        assert_eq!(root.relativize(&abs).unwrap(), rel);
    }

    #[test]
    fn verbatim_prefixes_are_normalized() {
        let p = normalize_verbatim_path(PathBuf::from(r"\\?\UNC\srv\share\Clade"));
        assert_eq!(p, PathBuf::from(r"\\srv\share\Clade"));
        assert_eq!(
            normalize_verbatim_path(PathBuf::from(r"\\?\C:\Clade")),
            PathBuf::from(r"C:\Clade")
        );
        assert_eq!(
            normalize_verbatim_path(PathBuf::from(r"\\?\d:\Clade")),
            PathBuf::from(r"d:\Clade")
        );

        let volume = PathBuf::from(r"\\?\Volume{12345678-1234-1234-1234-123456789012}\Clade");
        assert_eq!(normalize_verbatim_path(volume.clone()), volume);

        let non_drive = PathBuf::from(r"\\?\C:relative");
        assert_eq!(normalize_verbatim_path(non_drive.clone()), non_drive);

        let digit_drive = PathBuf::from(r"\\?\1:\Clade");
        assert_eq!(normalize_verbatim_path(digit_drive.clone()), digit_drive);

        let long = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(250)));
        assert_eq!(normalize_verbatim_path(long.clone()), long);
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

    #[test]
    fn map_lock_open_error_classifies_sharing_permission_and_other_errors() {
        assert!(matches!(
            map_lock_open_error(io::Error::from_raw_os_error(32)),
            RootError::AlreadyRunning
        ));
        assert!(matches!(
            map_lock_open_error(io::Error::from_raw_os_error(33)),
            RootError::AlreadyRunning
        ));
        assert!(matches!(
            map_lock_open_error(io::Error::from_raw_os_error(5)),
            RootError::NotWritable(_)
        ));
        assert!(matches!(
            map_lock_open_error(io::Error::from_raw_os_error(19)),
            RootError::NotWritable(_)
        ));
        assert!(matches!(
            map_lock_open_error(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
            RootError::NotWritable(_)
        ));
        assert!(matches!(
            map_lock_open_error(io::Error::new(io::ErrorKind::ReadOnlyFilesystem, "ro")),
            RootError::NotWritable(_)
        ));
        assert!(matches!(
            map_lock_open_error(io::Error::other("other")),
            RootError::Io(_)
        ));
    }

    #[test]
    fn acquire_maps_readonly_lock_file_to_not_writable() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_data_dir().unwrap();
        let lock_path = root.lock_file();
        fs::write(&lock_path, b"").unwrap();
        let mut perms = fs::metadata(&lock_path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&lock_path, perms).unwrap();

        let res = RootLock::acquire(&root);

        let mut reset = fs::metadata(&lock_path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        reset.set_readonly(false);
        let _ = fs::set_permissions(&lock_path, reset);

        assert!(matches!(res, Err(RootError::NotWritable(_))));
    }

    #[test]
    fn acquire_rejects_symlink_lock_file() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("portable"));
        root.ensure_data_dir().unwrap();
        let outside = d.path().join("outside.lock");

        if let Err(e) = std::os::windows::fs::symlink_file(&outside, root.lock_file()) {
            // Creating file symlinks requires Developer Mode or elevation on Windows.
            if e.raw_os_error() == Some(1314) {
                return;
            }
            panic!("unexpected symlink_file error: {e}");
        }

        let res = RootLock::acquire(&root);
        assert!(matches!(res, Err(RootError::NotWritable(_))));
        assert!(!outside.exists());
    }
}
