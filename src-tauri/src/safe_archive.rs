//! Hardened zip extraction for Java runtimes, loaders and modpacks.
//!
//! Everything in an archive is hostile until proven otherwise:
//! - entry names pass `RelPath` validation (no `..`, absolute/UNC/drive paths, backslashes,
//!   `:` streams, device names, trailing dots/spaces, control characters);
//! - symlink entries are refused;
//! - names that collide on a case-insensitive filesystem are refused;
//! - entry count, per-file size, total size and compression ratio are capped, and sizes are
//!   enforced while streaming (a zip bomb can lie in its headers);
//! - files are created with `create_new`, never overwriting, and parents are checked for links;
//! - extraction happens in a scratch directory and is moved into place only on full success.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Seek, Write};
use std::path::Path;

use zip::ZipArchive;

use crate::portable::fsutil::reject_links_between;
use crate::portable::relpath::{RelPath, RelPathError};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_entries: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    /// Maximum uncompressed:compressed ratio per entry.
    pub max_ratio: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_file_bytes: 2 * 1024 * 1024 * 1024,
            max_total_bytes: 8 * 1024 * 1024 * 1024,
            max_ratio: 1000,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("unsafe entry name {name:?}: {source}")]
    BadName { name: String, source: RelPathError },
    #[error("archive contains a symbolic link ({0:?}), which is not allowed")]
    Symlink(String),
    #[error("archive contains names that collide on case-insensitive filesystems: {0:?}")]
    CaseCollision(String),
    #[error("archive exceeds a safety limit: {0}")]
    LimitExceeded(&'static str),
    #[error("destination already exists")]
    DestinationExists,
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub dirs: usize,
    pub bytes: u64,
}

/// Extracts into a fresh scratch directory under `scratch_parent` (must be on the same volume
/// as `dest`, e.g. `cache/`), then renames it to `dest` (which must not exist).
pub fn extract_zip_atomic<R: Read + Seek>(
    reader: R,
    dest: &Path,
    scratch_parent: &Path,
    limits: Limits,
) -> Result<Report, ExtractError> {
    if dest.exists() {
        return Err(ExtractError::DestinationExists);
    }
    fs::create_dir_all(scratch_parent)?;
    let scratch = tempfile::Builder::new()
        .prefix("extract-")
        .tempdir_in(scratch_parent)?;
    let report = extract_zip_into(reader, scratch.path(), limits)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(scratch.path(), dest)?;
    // The directory now lives at `dest`; stop the guard from deleting a path that no longer exists.
    let _ = scratch.keep();
    Ok(report)
}

/// Extracts into an existing, empty-ish directory.
pub fn extract_zip_into<R: Read + Seek>(
    reader: R,
    base: &Path,
    limits: Limits,
) -> Result<Report, ExtractError> {
    let mut zip = ZipArchive::new(reader)?;
    if zip.len() > limits.max_entries {
        return Err(ExtractError::LimitExceeded("too many entries"));
    }

    // Pass 1: validate every name before writing anything.
    let mut seen = HashSet::new();
    for i in 0..zip.len() {
        let e = zip.by_index_raw(i)?;
        let name = e.name().to_owned();
        let rel = entry_path(&name)?;
        if let Some(mode) = e.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(ExtractError::Symlink(name));
            }
        }
        if !seen.insert(rel.fold_case()) {
            return Err(ExtractError::CaseCollision(name));
        }
    }

    // Pass 2: write, enforcing limits on the bytes that actually arrive.
    let mut report = Report::default();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i)?;
        let name = e.name().to_owned();
        let rel = entry_path(&name)?;
        let out = rel.to_path(base);
        reject_links_between(base, &out)?;

        if e.is_dir() {
            fs::create_dir_all(&out)?;
            report.dirs += 1;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
            reject_links_between(base, parent)?;
        }
        let compressed = e.compressed_size().max(1);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out)?;
        let written = copy_limited(&mut e, &mut file, limits.max_file_bytes)?;
        if written / compressed > limits.max_ratio && written > 1024 * 1024 {
            return Err(ExtractError::LimitExceeded("compression ratio"));
        }
        report.bytes += written;
        if report.bytes > limits.max_total_bytes {
            return Err(ExtractError::LimitExceeded("total uncompressed size"));
        }
        report.files += 1;
    }
    Ok(report)
}

fn entry_path(name: &str) -> Result<RelPath, ExtractError> {
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    RelPath::new(trimmed).map_err(|source| ExtractError::BadName {
        name: name.to_owned(),
        source,
    })
}

fn copy_limited(r: &mut impl Read, w: &mut impl Write, max: u64) -> Result<u64, ExtractError> {
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
        if total > max {
            return Err(ExtractError::LimitExceeded("file size"));
        }
        w.write_all(&buf[..n])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    fn build(entries: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        let mut c = w.finish().unwrap();
        c.set_position(0);
        c
    }

    fn extract(
        entries: &[(&str, &[u8])],
        limits: Limits,
    ) -> (tempfile::TempDir, Result<Report, ExtractError>) {
        let d = tempfile::tempdir().unwrap();
        let r = extract_zip_into(build(entries), d.path(), limits);
        (d, r)
    }

    #[test]
    fn extracts_a_normal_archive() {
        let (d, r) = extract(
            &[
                ("bin/javaw.exe", b"MZ"),
                ("lib/modules", b"data"),
                ("release", b"x"),
            ],
            Limits::default(),
        );
        assert_eq!(r.unwrap().files, 3);
        assert_eq!(
            fs::read(d.path().join("bin").join("javaw.exe")).unwrap(),
            b"MZ"
        );
    }

    #[test]
    fn rejects_path_traversal_and_absolute_names() {
        for name in [
            "../evil.txt",
            "a/../../evil.txt",
            "/abs.txt",
            "C:/Windows/evil.dll",
            "C:\\evil.dll",
            "a\\..\\b",
            "file.txt:stream",
            "con.txt",
            "x/NUL",
        ] {
            let (d, r) = extract(&[("ok.txt", b"1"), (name, b"x")], Limits::default());
            assert!(
                matches!(r, Err(ExtractError::BadName { .. })),
                "{name} was not rejected"
            );
            // Validation happens before any write, so even the harmless first entry is absent.
            assert!(!d.path().join("ok.txt").exists());
            assert!(!d.path().parent().unwrap().join("evil.txt").exists());
        }
    }

    #[test]
    fn rejects_symlink_entries() {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        w.add_symlink("link", "C:/Windows", SimpleFileOptions::default())
            .unwrap();
        let mut c = w.finish().unwrap();
        c.set_position(0);
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(
            extract_zip_into(c, d.path(), Limits::default()),
            Err(ExtractError::Symlink(_))
        ));
    }

    #[test]
    fn rejects_case_collisions() {
        let (_d, r) = extract(
            &[("Mods/a.jar", b"1"), ("mods/A.jar", b"2")],
            Limits::default(),
        );
        assert!(matches!(r, Err(ExtractError::CaseCollision(_))));
    }

    #[test]
    fn enforces_streaming_size_and_entry_limits() {
        let big = vec![0u8; 5000];
        let (_d, r) = extract(
            &[("a", &big)],
            Limits {
                max_file_bytes: 1000,
                ..Limits::default()
            },
        );
        assert!(matches!(r, Err(ExtractError::LimitExceeded(_))));
        let (_d, r) = extract(
            &[("a", &big), ("b", &big)],
            Limits {
                max_total_bytes: 6000,
                ..Limits::default()
            },
        );
        assert!(matches!(r, Err(ExtractError::LimitExceeded(_))));
        let (_d, r) = extract(
            &[("a", b"1"), ("b", b"2")],
            Limits {
                max_entries: 1,
                ..Limits::default()
            },
        );
        assert!(matches!(r, Err(ExtractError::LimitExceeded(_))));
    }

    #[test]
    fn rejects_zip_bombs_by_ratio() {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file(
            "bomb.bin",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        w.write_all(&vec![0u8; 20 * 1024 * 1024]).unwrap();
        let mut c = w.finish().unwrap();
        c.set_position(0);
        let d = tempfile::tempdir().unwrap();
        let r = extract_zip_into(
            c,
            d.path(),
            Limits {
                max_ratio: 50,
                ..Limits::default()
            },
        );
        assert!(matches!(
            r,
            Err(ExtractError::LimitExceeded("compression ratio"))
        ));
    }

    #[test]
    fn atomic_extract_leaves_nothing_behind_on_failure_and_refuses_existing_dest() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("cache");
        let dest = d.path().join("runtimes").join("java").join("x");
        let bad = build(&[("fine.txt", b"1"), ("../escape.txt", b"2")]);
        assert!(extract_zip_atomic(bad, &dest, &scratch, Limits::default()).is_err());
        assert!(!dest.exists());
        assert_eq!(
            fs::read_dir(&scratch).unwrap().count(),
            0,
            "scratch dir was not cleaned up"
        );

        let good = build(&[("fine.txt", b"1")]);
        extract_zip_atomic(good, &dest, &scratch, Limits::default()).unwrap();
        assert_eq!(fs::read(dest.join("fine.txt")).unwrap(), b"1");
        let again = build(&[("fine.txt", b"1")]);
        assert!(matches!(
            extract_zip_atomic(again, &dest, &scratch, Limits::default()),
            Err(ExtractError::DestinationExists)
        ));
    }
}
