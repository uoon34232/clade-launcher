//! Extracting native libraries into an instance's `natives/` folder.
//!
//! Native jars come from the shared, hash-verified library store and are still treated as
//! hostile archives: every jar is unpacked by the hardened extractor (`safe_archive`: no `..`,
//! absolute or drive paths, no links, no case collisions, size/entry/ratio limits enforced while
//! streaming) into a scratch folder under `cache/tmp/prepare/`, filtered, and only then swapped into
//! `instances/<id>/natives/` in one rename. The folder is disposable and rebuilt on every
//! preparation, so it never holds stale files from another version.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use crate::mc_meta::{Library, Role};
use crate::portable::fsutil::{
    is_link, reject_links_between, remove_dir_all_with_retry, rename_with_retry,
};
use crate::portable::{PortableRoot, RelPath};
use crate::safe_archive::{extract_zip_into, ExtractError, Limits};

const LIMITS: Limits = Limits {
    max_entries: 5_000,
    max_file_bytes: 64 * 1024 * 1024,
    max_total_bytes: 256 * 1024 * 1024,
    max_ratio: 1_000,
};

#[derive(Debug, thiserror::Error)]
pub enum NativesError {
    #[error("a native library archive is unsafe: {0}")]
    Unsafe(#[from] ExtractError),
    #[error("could not extract native libraries: {0}")]
    Io(#[from] io::Error),
}

/// Files that are never wanted in the natives folder (signatures, metadata, checksums).
fn skipped(rel: &str, exclude: &[String]) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.starts_with("meta-inf/")
        || lower.ends_with(".sha1")
        || lower.ends_with(".git")
        || exclude
            .iter()
            .any(|e| lower.starts_with(&e.to_ascii_lowercase()))
}

fn collect(dir: &Path, base: &Path, out: &mut Vec<(String, std::path::PathBuf)>) -> io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            collect(&p, base, out)?;
        } else {
            let rel = p
                .strip_prefix(base)
                .map_err(io::Error::other)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, p));
        }
    }
    Ok(())
}

fn refuse_if_link(path: &Path) -> io::Result<()> {
    match is_link(path) {
        Ok(true) => Err(io::Error::other(format!(
            "refusing to follow link at {}",
            path.display()
        ))),
        Ok(false) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Atomically swaps `out` into `natives_dir` by renaming any existing `natives_dir` to a sibling
/// `natives.old-<nonce>` backup first, restoring the backup if the replacement rename fails.
fn swap_natives_dir(
    root: &PortableRoot,
    out: &Path,
    natives_dir: &Path,
) -> Result<(), NativesError> {
    if natives_dir == root.path() {
        return Err(NativesError::Io(io::Error::other(
            "natives directory cannot be the launcher root",
        )));
    }
    reject_links_between(root.path(), natives_dir)?;
    refuse_if_link(natives_dir)?;

    let parent = natives_dir
        .parent()
        .ok_or_else(|| io::Error::other("natives directory has no parent"))?;
    reject_links_between(root.path(), parent)?;
    fs::create_dir_all(parent)?;
    reject_links_between(root.path(), natives_dir)?;
    refuse_if_link(natives_dir)?;

    let backup = if fs::symlink_metadata(natives_dir).is_ok() {
        let mut nonce = [0u8; 6];
        getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
        let b = parent.join(format!("natives.old-{}", hex::encode(nonce)));
        rename_with_retry(natives_dir, &b)?;
        Some(b)
    } else {
        None
    };

    if let Err(e) = rename_with_retry(out, natives_dir) {
        if let Some(b) = &backup {
            let _ = rename_with_retry(b, natives_dir);
        }
        return Err(NativesError::Io(e));
    }

    if let Some(b) = &backup {
        let _ = remove_dir_all_with_retry(b);
    }
    Ok(())
}

/// Rebuilds `natives_dir` from the given libraries (only those whose role extracts natives).
/// Returns the number of files placed.
pub fn extract_natives(
    root: &PortableRoot,
    natives_dir: &Path,
    libraries: &[&Library],
) -> Result<usize, NativesError> {
    reject_links_between(root.path(), natives_dir)?;
    refuse_if_link(natives_dir)?;

    let tmp = root.prepare_tmp_dir();
    reject_links_between(root.path(), &tmp)?;
    fs::create_dir_all(&tmp)?;
    reject_links_between(root.path(), &tmp)?;

    let scratch = tempfile::Builder::new()
        .prefix("natives-")
        .tempdir_in(&tmp)?;
    let scratch_path = scratch.keep();

    let result = (|| -> Result<usize, NativesError> {
        let out = scratch_path.join("out");
        fs::create_dir(&out)?;
        let mut placed = 0usize;

        for (i, lib) in libraries.iter().enumerate() {
            let exclude: &[String] = match &lib.role {
                Role::Native { exclude } | Role::ClasspathAndNative { exclude } => exclude,
                Role::Classpath => continue,
            };
            let jar = lib.artifact.path.to_path(&root.libraries_dir());
            let unpacked = scratch_path.join(format!("jar-{i}"));
            fs::create_dir(&unpacked)?;
            extract_zip_into(File::open(&jar)?, &unpacked, LIMITS)?;
            let mut files = Vec::new();
            collect(&unpacked, &unpacked, &mut files)?;
            for (rel, src) in files {
                if skipped(&rel, exclude) {
                    continue;
                }
                // Names were validated by the extractor; checked again where they are used.
                let rel = RelPath::new(&rel).map_err(|e| io::Error::other(e.to_string()))?;
                let dst = rel.to_path(&out);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Move extracted native file instead of copying bytes twice; fall back to copy if needed.
                if rename_with_retry(&src, &dst).is_err() {
                    fs::copy(&src, &dst)?;
                }
                placed += 1;
            }
        }

        swap_natives_dir(root, &out, natives_dir)?;
        Ok(placed)
    })();

    let _ = remove_dir_all_with_retry(&scratch_path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mc_meta::Artifact;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn jar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, data) in entries {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn lib(path: &str, role: Role) -> Library {
        Library {
            name: path.into(),
            artifact: Artifact {
                path: RelPath::new(path).unwrap(),
                url: None,
                expected: None,
                size: None,
            },
            role,
        }
    }

    fn setup(jars: &[(&str, Vec<u8>)]) -> (tempfile::TempDir, PortableRoot) {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        for (p, bytes) in jars {
            let dst = root.libraries_dir().join(p);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::write(dst, bytes).unwrap();
        }
        (d, root)
    }

    #[test]
    fn extracts_dlls_honours_excludes_and_replaces_stale_files() {
        let natives = jar(&[
            ("lwjgl.dll", b"dll1"),
            ("windows/x64/org/lwjgl/lwjgl.dll", b"dll2"),
            ("META-INF/MANIFEST.MF", b"m"),
            ("META-INF/x.SF", b"s"),
            ("lwjgl.dll.sha1", b"h"),
            ("skip/me.dll", b"no"),
        ]);
        let (d, root) = setup(&[("o/l/natives.jar", natives)]);
        let target = d.path().join("instances").join("x").join("natives");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("stale.dll"), b"old").unwrap();
        let l = lib(
            "o/l/natives.jar",
            Role::Native {
                exclude: vec!["skip/".into()],
            },
        );
        let n = extract_natives(&root, &target, &[&l]).unwrap();
        assert_eq!(n, 2);
        assert_eq!(fs::read(target.join("lwjgl.dll")).unwrap(), b"dll1");
        assert_eq!(
            fs::read(target.join("windows/x64/org/lwjgl/lwjgl.dll")).unwrap(),
            b"dll2"
        );
        assert!(
            !target.join("META-INF").exists()
                && !target.join("skip").exists()
                && !target.join("stale.dll").exists()
        );
        assert!(!target.join("lwjgl.dll.sha1").exists());
        // Scratch space is cleaned up.
        assert_eq!(fs::read_dir(root.prepare_tmp_dir()).unwrap().count(), 0);
    }

    #[test]
    fn classpath_only_libraries_are_not_extracted() {
        let (d, root) = setup(&[("a/b.jar", jar(&[("x.dll", b"1")]))]);
        let target = d.path().join("natives");
        let l = lib("a/b.jar", Role::Classpath);
        assert_eq!(extract_natives(&root, &target, &[&l]).unwrap(), 0);
        assert!(target.is_dir() && fs::read_dir(&target).unwrap().count() == 0);
    }

    #[test]
    fn unsafe_archives_are_refused_and_change_nothing() {
        let evil = jar(&[("../../escape.dll", b"x")]);
        let (d, root) = setup(&[("a/evil.jar", evil)]);
        let target = d.path().join("natives");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.dll"), b"keep").unwrap();
        let l = lib("a/evil.jar", Role::Native { exclude: vec![] });
        assert!(extract_natives(&root, &target, &[&l]).is_err());
        assert_eq!(
            fs::read(target.join("keep.dll")).unwrap(),
            b"keep",
            "the existing folder is untouched"
        );
        assert!(!d.path().join("escape.dll").exists());

        let abs = jar(&[("C:/Windows/evil.dll", b"x")]);
        let (_d2, root2) = setup(&[("a/abs.jar", abs)]);
        let l2 = lib("a/abs.jar", Role::Native { exclude: vec![] });
        assert!(extract_natives(&root2, &root2.path().join("n"), &[&l2]).is_err());
    }

    #[test]
    fn a_missing_library_jar_is_an_error() {
        let (d, root) = setup(&[]);
        let l = lib("a/missing.jar", Role::Native { exclude: vec![] });
        assert!(extract_natives(&root, &d.path().join("n"), &[&l]).is_err());
    }

    #[test]
    fn skips_meta_inf_checksums_and_excludes_case_insensitively() {
        let first = jar(&[
            ("lwjgl.dll", b"v1"),
            ("meta-inf/MANIFEST.MF", b"m"),
            ("lwjgl.dll.SHA1", b"h"),
            ("repo.GIT", b"g"),
            ("Skip/ignored.dll", b"no"),
        ]);
        let second = jar(&[("lwjgl.dll", b"v2"), ("Meta-Inf/CERT.RSA", b"c")]);
        let (d, root) = setup(&[("a/first.jar", first), ("a/second.jar", second)]);
        let target = d.path().join("instances").join("y").join("natives");
        let l1 = lib(
            "a/first.jar",
            Role::ClasspathAndNative {
                exclude: vec!["SKIP/".into()],
            },
        );
        let l2 = lib("a/second.jar", Role::Native { exclude: vec![] });

        let placed = extract_natives(&root, &target, &[&l1, &l2]).unwrap();
        assert_eq!(placed, 2);
        assert_eq!(
            fs::read(target.join("lwjgl.dll")).unwrap(),
            b"v2",
            "later native jar replaces earlier file"
        );
        let entries: Vec<String> = fs::read_dir(&target)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, ["lwjgl.dll"]);
    }

    #[test]
    fn atomic_swap_restores_existing_natives_if_replacement_fails() {
        let (d, root) = setup(&[]);
        let inst_dir = d.path().join("instances").join("p");
        let target = inst_dir.join("natives");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("openal.dll"), b"preserved").unwrap();

        // Missing `out` causes the replacement rename to fail after `target` was moved to
        // `natives.old-<nonce>`; `swap_natives_dir` must roll the backup back into `target`.
        let missing_out = root.prepare_tmp_dir().join("missing-out");
        assert!(swap_natives_dir(&root, &missing_out, &target).is_err());
        assert_eq!(fs::read(target.join("openal.dll")).unwrap(), b"preserved");

        let siblings: Vec<String> = fs::read_dir(&inst_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            siblings,
            ["natives"],
            "no orphaned backup directory left behind"
        );
    }
}
