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
use std::path::{Component, Path, PathBuf};

use zip::ZipArchive;

use crate::portable::fsutil::{
    is_link, reject_links_between, remove_dir_all_with_retry, rename_with_retry,
};
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

fn common_base_dir(a: &Path, b: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    let mut normal_count = 0usize;
    for (ca, cb) in a.components().zip(b.components()) {
        let same = ca
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&cb.as_os_str().to_string_lossy());
        if !same {
            break;
        }
        if matches!(ca, Component::Normal(_)) {
            normal_count += 1;
        }
        out.push(ca.as_os_str());
    }
    (normal_count >= 1 && fs::symlink_metadata(&out).is_ok()).then_some(out)
}

fn ensure_dir_no_links(dir: &Path, common_base: Option<&Path>) -> io::Result<()> {
    let base = match common_base.filter(|b| !b.as_os_str().is_empty()) {
        Some(b) => b,
        None => {
            let mut anc = dir;
            while fs::symlink_metadata(anc).is_err() {
                match anc.parent() {
                    Some(p) if !p.as_os_str().is_empty() => anc = p,
                    _ => break,
                }
            }
            anc
        }
    };
    reject_links_between(base, dir)?;
    fs::create_dir_all(dir)?;
    reject_links_between(base, dir)?;
    if is_link(dir).unwrap_or(false) {
        return Err(io::Error::other(format!(
            "refusing to follow link at {}",
            dir.display()
        )));
    }
    Ok(())
}

fn ensure_dir_cached(
    base: &Path,
    dir: &Path,
    checked_dirs: &mut HashSet<PathBuf>,
) -> io::Result<()> {
    if !checked_dirs.contains(dir) {
        reject_links_between(base, dir)?;
        fs::create_dir_all(dir)?;
        reject_links_between(base, dir)?;
        checked_dirs.insert(dir.to_path_buf());
    }
    Ok(())
}

/// Extracts into a fresh scratch directory under `scratch_parent` (must be on the same volume
/// as `dest`, e.g. `cache/`), then renames it to `dest` (which must not exist).
pub fn extract_zip_atomic<R: Read + Seek>(
    reader: R,
    dest: &Path,
    scratch_parent: &Path,
    limits: Limits,
) -> Result<Report, ExtractError> {
    if fs::symlink_metadata(dest).is_ok() {
        return Err(ExtractError::DestinationExists);
    }
    let common = common_base_dir(dest, scratch_parent);
    ensure_dir_no_links(scratch_parent, common.as_deref())?;

    let scratch = tempfile::Builder::new()
        .prefix("extract-")
        .tempdir_in(scratch_parent)?;
    let scratch_path = scratch.keep();

    let result = (|| -> Result<Report, ExtractError> {
        let report = extract_zip_into(reader, &scratch_path, limits)?;
        if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
            ensure_dir_no_links(parent, common.as_deref())?;
        }
        if fs::symlink_metadata(dest).is_ok() {
            return Err(ExtractError::DestinationExists);
        }
        rename_with_retry(&scratch_path, dest)?;
        Ok(report)
    })();

    if result.is_err() {
        let _ = remove_dir_all_with_retry(&scratch_path);
    }
    result
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
        if e.is_symlink() || e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(ExtractError::Symlink(name));
        }
        if !seen.insert(rel.fold_case()) {
            return Err(ExtractError::CaseCollision(name));
        }
    }

    // Pass 2: write, enforcing limits on the bytes that actually arrive.
    reject_links_between(base, base)?;
    let mut checked_dirs: HashSet<PathBuf> = HashSet::new();
    checked_dirs.insert(base.to_path_buf());
    let mut report = Report::default();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i)?;
        let name = e.name().to_owned();
        if e.is_symlink() || e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(ExtractError::Symlink(name));
        }
        let rel = entry_path(&name)?;
        let out = rel.to_path(base);

        if e.is_dir() {
            ensure_dir_cached(base, &out, &mut checked_dirs)?;
            report.dirs += 1;
            continue;
        }
        if let Some(parent) = out.parent() {
            ensure_dir_cached(base, parent, &mut checked_dirs)?;
        }
        let compressed = e.compressed_size().max(1);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out)?;
        let written = copy_limited(&mut e, &mut file, compressed, report.bytes, &limits)?;
        report.bytes = report.bytes.saturating_add(written);
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

fn copy_limited(
    r: &mut impl Read,
    w: &mut impl Write,
    compressed: u64,
    total_so_far: u64,
    limits: &Limits,
) -> Result<u64, ExtractError> {
    let mut buf = [0u8; 64 * 1024];
    let mut entry_bytes = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(entry_bytes);
        }
        entry_bytes = entry_bytes.saturating_add(n as u64);
        if entry_bytes > limits.max_file_bytes {
            return Err(ExtractError::LimitExceeded("file size"));
        }
        if entry_bytes / compressed > limits.max_ratio {
            return Err(ExtractError::LimitExceeded("compression ratio"));
        }
        if total_so_far.saturating_add(entry_bytes) > limits.max_total_bytes {
            return Err(ExtractError::LimitExceeded("total uncompressed size"));
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

    #[test]
    fn enforces_ratio_on_sub_megabyte_entries_and_total_bytes_mid_stream() {
        // 200 KiB (<= 1 MiB) of zeros compresses to ~220 bytes (~930:1 > 50:1) and must be
        // stopped on the first chunk before writing to disk.
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file(
            "small_bomb.bin",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        w.write_all(&vec![0u8; 200 * 1024]).unwrap();
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
        assert_eq!(
            fs::metadata(d.path().join("small_bomb.bin")).unwrap().len(),
            0,
            "offending chunk must be rejected before writing to disk"
        );

        // Cumulative `max_total_bytes` must abort mid-stream on the second file before writing past limit.
        let non_compressible_a: Vec<u8> = (0..4_000).map(|i| (i * 31 % 251) as u8).collect();
        let non_compressible_b: Vec<u8> = (0..200_000).map(|i| (i * 17 % 253) as u8).collect();
        let (d2, r2) = extract(
            &[
                ("sub/a.bin", &non_compressible_a),
                ("sub/b.bin", &non_compressible_b),
            ],
            Limits {
                max_total_bytes: 70_000,
                ..Limits::default()
            },
        );
        assert!(matches!(
            r2,
            Err(ExtractError::LimitExceeded("total uncompressed size"))
        ));
        let written_a = fs::metadata(d2.path().join("sub/a.bin")).unwrap().len();
        let written_b = fs::metadata(d2.path().join("sub/b.bin")).unwrap().len();
        assert!(
            written_a + written_b <= 70_000,
            "must never write past max_total_bytes ({} + {})",
            written_a,
            written_b
        );
    }
}
