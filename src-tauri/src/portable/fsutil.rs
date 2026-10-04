//! Filesystem primitives shared by every module that writes to the portable folder.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path};

/// Writes `bytes` to `path` so readers only ever see the old or the new content, never a
/// torn file. Matters on USB sticks that can be pulled mid-write.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(parent)?;
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
        fs::rename(&tmp, path) // replaces atomically on Windows (MoveFileEx) and POSIX
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
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

/// Fails if any existing component between `base` and `target` is a link. Defends against
/// a hostile archive (or a stray junction on the stick) redirecting writes outside the root.
pub fn reject_links_between(base: &Path, target: &Path) -> io::Result<()> {
    let rest = target
        .strip_prefix(base)
        .map_err(|_| io::Error::other("target is outside the base directory"))?;
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
        let _ = fs::remove_file(tmp);
        return Err(e);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(tmp, dest)?;
    Ok(())
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
