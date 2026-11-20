//! Filesystem primitives shared by every module that writes to the portable folder.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const RETRY_DELAYS_MS: &[u64] = &[10, 25, 50, 100, 200, 400, 750];

/// True when `err` is a transient Windows sharing or antivirus scan lock (`ERROR_ACCESS_DENIED` = 5
/// or `ERROR_SHARING_VIOLATION` = 32) commonly triggered by Windows Defender or Search Indexer
/// right after a file handle closes.
pub fn is_transient_win_lock(err: &io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(err.raw_os_error(), Some(5 | 32))
    }
    #[cfg(not(windows))]
    {
        let _ = err;
        false
    }
}

fn clear_readonly(path: &Path) {
    if let Ok(md) = fs::symlink_metadata(path) {
        let mut perms = md.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

#[cfg(windows)]
fn wide_null(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Atomically renames `src` over `dst` (flushing directory metadata with `MOVEFILE_WRITE_THROUGH` on
/// Windows) and retries transient Windows Defender / Indexer locks (`os error 5` and `32`).
pub fn rename_with_retry(src: &Path, dst: &Path) -> io::Result<()> {
    let mut attempt = 0usize;
    loop {
        let res = {
            #[cfg(windows)]
            {
                use windows_sys::Win32::Storage::FileSystem::{
                    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
                };
                let src_w = wide_null(src);
                let dst_w = wide_null(dst);
                // SAFETY: Both buffers are NUL-terminated UTF-16 and outlive the call.
                let ok = unsafe {
                    MoveFileExW(
                        src_w.as_ptr(),
                        dst_w.as_ptr(),
                        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                    )
                };
                if ok != 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            }
            #[cfg(not(windows))]
            {
                fs::rename(src, dst)
            }
        };
        match res {
            Ok(()) => return Ok(()),
            Err(e) if is_transient_win_lock(&e) && attempt < RETRY_DELAYS_MS.len() => {
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Moves `src` to `dst` ONLY if `dst` does not already exist (no `MOVEFILE_REPLACE_EXISTING`),
/// retrying transient scan locks on `src`. Never overwrites an existing backup.
pub fn move_new_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    let mut attempt = 0usize;
    loop {
        let res = {
            #[cfg(windows)]
            {
                use windows_sys::Win32::Storage::FileSystem::{
                    MoveFileExW, MOVEFILE_WRITE_THROUGH,
                };
                let src_w = wide_null(src);
                let dst_w = wide_null(dst);
                // SAFETY: Both buffers are NUL-terminated UTF-16 and outlive the call.
                let ok =
                    unsafe { MoveFileExW(src_w.as_ptr(), dst_w.as_ptr(), MOVEFILE_WRITE_THROUGH) };
                if ok != 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            }
            #[cfg(not(windows))]
            {
                if fs::symlink_metadata(dst).is_ok() {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "destination exists",
                    ))
                } else {
                    fs::rename(src, dst)
                }
            }
        };
        match res {
            Ok(()) => return Ok(()),
            Err(e)
                if e.kind() == io::ErrorKind::AlreadyExists
                    || matches!(e.raw_os_error(), Some(80 | 183)) =>
            {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, e));
            }
            Err(e) if is_transient_win_lock(&e) && attempt < RETRY_DELAYS_MS.len() => {
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Removes a file, retrying transient Windows Defender / Indexer locks. Missing files succeed.
pub fn remove_file_with_retry(path: &Path) -> io::Result<()> {
    let mut attempt = 0usize;
    loop {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if is_transient_win_lock(&e) && attempt < RETRY_DELAYS_MS.len() => {
                clear_readonly(path);
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Removes a directory tree (refusing top-level reparse points), retrying transient Windows locks.
pub fn remove_dir_all_with_retry(path: &Path) -> io::Result<()> {
    match is_link(path) {
        Ok(true) => {
            return Err(io::Error::other(format!(
                "refusing to remove link directory {}",
                path.display()
            )))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        _ => {}
    }
    let mut attempt = 0usize;
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if is_transient_win_lock(&e) && attempt < RETRY_DELAYS_MS.len() => {
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Reads a file up to `max_bytes`, failing before allocating multi-gigabyte buffers on corrupt or
/// hostile inputs and retrying transient sharing violations on Windows.
pub fn read_bounded(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let mut attempt = 0usize;
    let file = loop {
        match File::open(path) {
            Ok(f) => break f,
            Err(e) if is_transient_win_lock(&e) && attempt < RETRY_DELAYS_MS.len() => {
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    };
    let len = file.metadata()?.len();
    if len > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file exceeds maximum size ({len} > {max_bytes} bytes)"),
        ));
    }
    let cap = usize::try_from(len).unwrap_or(0);
    let mut out = Vec::with_capacity(cap);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut out)?;
    if out.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file grew past maximum size while reading",
        ));
    }
    Ok(out)
}

/// Allocation unit (cluster size) in bytes for the volume holding `path`, clamped to a sane
/// `4 KiB..=1 MiB` range. Matters on exFAT USB drives with 128–256 KiB clusters.
pub fn cluster_size_bytes(path: &Path) -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceW;
        let mut dir = path;
        while !dir.exists() {
            let Some(p) = dir.parent() else {
                return 4096;
            };
            dir = p;
        }
        let root_anc = dir.ancestors().last().unwrap_or(dir);
        let wide = wide_null(root_anc);
        let mut spc = 0u32;
        let mut bps = 0u32;
        let mut free_c = 0u32;
        let mut total_c = 0u32;
        // SAFETY: `wide` is NUL-terminated and all out pointers are valid scalars.
        let ok = unsafe {
            GetDiskFreeSpaceW(wide.as_ptr(), &mut spc, &mut bps, &mut free_c, &mut total_c)
        };
        if ok != 0 && spc > 0 && bps > 0 {
            return (u64::from(spc) * u64::from(bps)).clamp(4096, 1024 * 1024);
        }
        4096
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let mut dir = path;
        while !dir.exists() {
            let Some(p) = dir.parent() else {
                return 4096;
            };
            dir = p;
        }
        let Ok(c_path) = CString::new(dir.as_os_str().as_bytes()) else {
            return 4096;
        };
        // SAFETY: `libc::statvfs` expects an initialized/zeroed `statvfs` buffer and a valid
        // NUL-terminated path pointer.
        unsafe {
            let mut vfs: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c_path.as_ptr(), &mut vfs) == 0 {
                #[allow(clippy::unnecessary_cast)]
                let unit = if vfs.f_frsize > 0 {
                    vfs.f_frsize as u64
                } else {
                    vfs.f_bsize as u64
                };
                if unit > 0 {
                    return unit.clamp(4096, 1024 * 1024);
                }
            }
        }
        4096
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        4096
    }
}

/// Writes `bytes` to `path` so readers only ever see the old or the new content, never a
/// torn file. Matters on USB sticks that can be pulled mid-write.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(parent)?;
    if is_link(parent).unwrap_or(false) || is_link(path).unwrap_or(false) {
        return Err(io::Error::other("refusing to atomic_write through a link"));
    }
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    let mut nonce = [0u8; 6];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    tmp_name.push(format!(".{}.tmp", hex::encode(nonce)));
    let tmp = parent.join(tmp_name);

    let result = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?; // flush to the device, not just the OS cache
        drop(f);
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = remove_file_with_retry(&tmp);
    }
    result
}

/// True if `p` is a symlink, junction or any other reparse point.
pub fn is_link(p: &Path) -> io::Result<bool> {
    let md = fs::symlink_metadata(p)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        Ok(md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(md.file_type().is_symlink())
    }
}

pub(crate) fn strip_prefix_portable(target: &Path, base: &Path) -> Option<PathBuf> {
    if let Ok(rest) = target.strip_prefix(base) {
        return Some(rest.to_path_buf());
    }
    #[cfg(windows)]
    {
        let mut t_comps = target.components();
        for b_comp in base.components() {
            let t_comp = t_comps.next()?;
            let eq = b_comp
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&t_comp.as_os_str().to_string_lossy());
            if !eq {
                return None;
            }
        }
        let mut out = PathBuf::new();
        for rem in t_comps {
            out.push(rem.as_os_str());
        }
        Some(out)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Fails if `base` itself or any existing component between `base` and `target` is a link.
/// Defends against a hostile archive (or a stray NTFS junction on the stick) redirecting writes
/// outside the root.
pub fn reject_links_between(base: &Path, target: &Path) -> io::Result<()> {
    match is_link(base) {
        Ok(true) => {
            return Err(io::Error::other(format!(
                "refusing to follow link at {}",
                base.display()
            )))
        }
        Ok(false) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let rest = strip_prefix_portable(target, base)
        .ok_or_else(|| io::Error::other("target is outside the base directory"))?;
    let mut cur = base.to_path_buf();
    for comp in rest.components() {
        match comp {
            Component::Normal(c) => cur.push(c),
            _ => return Err(io::Error::other("unexpected path component")),
        }
        match is_link(&cur) {
            Ok(true) => {
                return Err(io::Error::other(format!(
                    "refusing to follow link at {}",
                    cur.display()
                )))
            }
            Ok(false) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => break, // nothing deeper exists yet
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Sha1(String),
    Sha256(String),
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("size mismatch: expected {expected} bytes, got {actual}")]
    Size { expected: u64, actual: u64 },
    #[error("hash mismatch")]
    Hash,
}

fn digest_reader<D: sha2::digest::Digest>(mut reader: impl Read) -> io::Result<(String, u64)> {
    let mut d = D::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        d.update(&buf[..n]);
    }
    Ok((hex::encode(d.finalize()), total))
}

/// Streams `reader` through the expected digest. Hex comparison is case-insensitive.
pub fn verify_reader(
    reader: impl Read,
    expected: &Expected,
    size: Option<u64>,
) -> Result<(), VerifyError> {
    let ((actual, total), want) = match expected {
        Expected::Sha1(h) => (digest_reader::<sha1::Sha1>(reader)?, h),
        Expected::Sha256(h) => (digest_reader::<sha2::Sha256>(reader)?, h),
    };
    if let Some(expected_size) = size {
        if expected_size != total {
            return Err(VerifyError::Size {
                expected: expected_size,
                actual: total,
            });
        }
    }
    if !actual.eq_ignore_ascii_case(want) {
        return Err(VerifyError::Hash);
    }
    Ok(())
}

/// SHA-256 of a file as lowercase hex, streamed.
pub fn sha256_hex(path: &Path) -> io::Result<String> {
    Ok(digest_reader::<sha2::Sha256>(File::open(path)?)?.0)
}

pub fn verify_file(path: &Path, expected: &Expected, size: Option<u64>) -> Result<(), VerifyError> {
    verify_reader(File::open(path)?, expected, size)
}

/// Moves a fully downloaded temp file into place, but only if it matches the expected
/// size/hash. On mismatch the temp file is deleted and nothing at `dest` changes.
pub fn install_verified(
    tmp: &Path,
    dest: &Path,
    expected: &Expected,
    size: Option<u64>,
) -> Result<(), VerifyError> {
    if let Err(e) = verify_file(tmp, expected, size) {
        let _ = remove_file_with_retry(tmp);
        return Err(e);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    rename_with_retry(tmp, dest)?;
    Ok(())
}

/// Moves a damaged file out of the way without ever overwriting an earlier backup: the first
/// choice is `<name>.corrupt`, then `<name>.corrupt-<unix time>`, then the same with a counter.
/// Returns where the file went. Nothing is deleted.
pub fn quarantine_corrupt(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("path has no file name"))?
        .to_string_lossy()
        .into_owned();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut candidates = vec![format!("{name}.corrupt"), format!("{name}.corrupt-{stamp}")];
    candidates.extend((1..1000).map(|n| format!("{name}.corrupt-{stamp}-{n}")));
    for candidate in candidates {
        let target = dir.join(candidate);
        match move_new_no_replace(path, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("too many corrupt backups"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    const HELLO_SHA1: &str = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("f.json");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn quarantine_keeps_every_damaged_copy() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let mut seen = Vec::new();
        for n in 0..3 {
            fs::write(&p, format!("bad {n}")).unwrap();
            let moved = quarantine_corrupt(&p).unwrap();
            assert!(!p.exists());
            assert_eq!(fs::read_to_string(&moved).unwrap(), format!("bad {n}"));
            seen.push(moved);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 3, "no backup overwrote another");
        assert!(d.path().join("settings.json.corrupt").is_file());
    }

    #[test]
    fn read_bounded_enforces_ceiling() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.bin");
        fs::write(&p, b"1234567890").unwrap();
        assert_eq!(read_bounded(&p, 10).unwrap(), b"1234567890");
        assert!(read_bounded(&p, 9).is_err());
    }

    #[test]
    fn verifies_hashes_and_sizes() {
        let hello = b"hello";
        assert!(verify_reader(&hello[..], &Expected::Sha256(HELLO_SHA256.into()), Some(5)).is_ok());
        assert!(
            verify_reader(&hello[..], &Expected::Sha1(HELLO_SHA1.to_uppercase()), None).is_ok()
        );
        assert!(matches!(
            verify_reader(&hello[..], &Expected::Sha256(HELLO_SHA256.into()), Some(6)),
            Err(VerifyError::Size { .. })
        ));
        assert!(matches!(
            verify_reader(&b"hellp"[..], &Expected::Sha256(HELLO_SHA256.into()), None),
            Err(VerifyError::Hash)
        ));
    }

    #[test]
    fn install_verified_rejects_and_cleans_up() {
        let d = tempfile::tempdir().unwrap();
        let tmp = d.path().join("dl.tmp");
        let dest = d.path().join("out").join("file.bin");
        fs::write(&tmp, b"tampered").unwrap();
        assert!(
            install_verified(&tmp, &dest, &Expected::Sha256(HELLO_SHA256.into()), None).is_err()
        );
        assert!(!tmp.exists() && !dest.exists());
        fs::write(&tmp, b"hello").unwrap();
        install_verified(&tmp, &dest, &Expected::Sha256(HELLO_SHA256.into()), Some(5)).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
    }
}
