//! Installing Mojang's own Java runtimes under `runtimes/java/<id>/`.
//!
//! Source: Mojang's runtime index (`piston-meta.mojang.com/v1/products/java-runtime/.../all.json`,
//! the same data the official launcher uses). For the version's required component (for example
//! `java-runtime-delta`) the index names a manifest listing every file of that runtime with its
//! SHA-1 and size. Nothing else is consulted: no `JAVA_HOME`, no `PATH`, no registry, no
//! third-party JDK vendor, and the user never installs Java.
//!
//! Installation is crash-safe: files download (verified, resumable) into
//! `cache/java-staging/<id>/`, `runtime.json` is written inside that folder last, and only then is
//! the whole folder renamed to `runtimes/java/<id>/`. A runtime folder therefore either is
//! complete and verified or does not exist; an interrupted run keeps its verified files and the
//! next run continues from them. An installed runtime is reused (no network) while its
//! launch executable (`javaw.exe`, falling back to `java.exe`) still matches the hash recorded at
//! install time ([`resolve_java`]).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::download::{DownloadError, Downloader, FileJob};
use crate::mc_meta::Download;
use crate::net::{FetchError, Fetcher, Streamer};
use crate::portable::fsutil::{self, atomic_write, sha256_hex, Expected};
use crate::portable::{PortableRoot, RelPath};
use crate::runtimes::{resolve_java, runtime_dir, ManagedJava, RuntimeManifest};

pub const RUNTIME_INDEX_URL: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
#[cfg(windows)]
pub const INDEX_PLATFORM: &str = "windows-x64";
#[cfg(not(windows))]
pub const INDEX_PLATFORM: &str = "linux";

#[cfg(windows)]
pub const RUNTIME_SUFFIX: &str = "windows-x64";
#[cfg(not(windows))]
pub const RUNTIME_SUFFIX: &str = "linux-x64";

const MAX_INDEX_BYTES: usize = 2 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[cfg(windows)]
pub const JAVAW_EXE: Option<&str> = Some("bin/javaw.exe");
#[cfg(not(windows))]
pub const JAVAW_EXE: Option<&str> = None;

#[cfg(windows)]
pub const JAVA_EXE: &str = "bin/java.exe";
#[cfg(not(windows))]
pub const JAVA_EXE: &str = "bin/java";

const STAGING: &str = "java-staging";
/// Written beside `runtime.json` at install time: every file of the runtime with the SHA-1 and size
/// Mojang's manifest gave for it. It is what lets "Verify files" check the whole runtime, offline.
const FILES_RECORD: &str = "runtime-files.json";

#[derive(Debug, thiserror::Error)]
pub enum JavaError {
    #[error("Mojang's Java runtime list could not be read: {0}")]
    Index(&'static str),
    #[error("Mojang offers no Java runtime '{0}' for this platform")]
    NoSuchRuntime(String),
    #[error("the Java runtime description is unsafe or malformed: {0}")]
    Manifest(&'static str),
    #[error("the Java runtime description failed its integrity check")]
    ManifestHash,
    #[error("Java could not be downloaded: {0}")]
    Fetch(#[from] FetchError),
    #[error("Java could not be downloaded: {0}")]
    Download(#[from] DownloadError),
    #[error("Java could not be installed: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRelease {
    pub component: String,
    pub version_name: String,
    pub manifest: Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFile {
    pub path: RelPath,
    pub download: Download,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLink {
    pub path: RelPath,
    pub target: RelPath,
}

/// Everything needed to install one runtime, known before any file is downloaded.
#[derive(Debug, Clone)]
pub struct RuntimePlan {
    pub release: RuntimeRelease,
    pub dirs: Vec<RelPath>,
    pub files: Vec<RuntimeFile>,
    pub links: Vec<RuntimeLink>,
}

impl RuntimePlan {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.download.size).sum()
    }
}

fn hex40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn download_of(v: &Value) -> Option<Download> {
    let sha1 = v.get("sha1")?.as_str().filter(|h| hex40(h))?;
    Some(Download {
        url: url::Url::parse(v.get("url")?.as_str()?).ok()?,
        sha1: sha1.to_ascii_lowercase(),
        size: v.get("size")?.as_u64()?,
    })
}

/// Picks `component` for the host OS out of Mojang's runtime index.
pub fn parse_index(body: &[u8], component: &str) -> Result<RuntimeRelease, JavaError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| JavaError::Index("not JSON"))?;
    let release = v
        .get(INDEX_PLATFORM)
        .and_then(|p| p.get(component))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| JavaError::NoSuchRuntime(component.to_owned()))?;
    let manifest = release
        .get("manifest")
        .and_then(download_of)
        .ok_or(JavaError::Index("no usable manifest"))?;
    let version_name = release
        .pointer("/version/name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty() && n.len() <= 40)
        .ok_or(JavaError::Index("no version name"))?;
    Ok(RuntimeRelease {
        component: component.to_owned(),
        version_name: version_name.to_owned(),
        manifest,
    })
}

fn resolve_link_target(link_path: &RelPath, target: &str) -> Result<RelPath, JavaError> {
    if target.is_empty() || target.contains(['\0', '\\', ':']) || target.starts_with('/') {
        return Err(JavaError::Manifest("unsafe link target"));
    }
    let mut stack: Vec<&str> = link_path.as_str().split('/').collect();
    stack.pop();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Err(JavaError::Manifest("unsafe link target"));
                }
            }
            s => stack.push(s),
        }
    }
    let joined = stack.join("/");
    RelPath::new(&joined).map_err(|_| JavaError::Manifest("unsafe link target"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFiles {
    pub dirs: Vec<RelPath>,
    pub files: Vec<RuntimeFile>,
    pub links: Vec<RuntimeLink>,
}

/// Parses a component manifest. Intra-runtime `link` entries (used in Mojang's Linux runtimes)
/// are strictly resolved inside the runtime root and materialized as regular copies later so no
/// OS symlinks are ever written.
pub fn parse_files(body: &[u8]) -> Result<ParsedFiles, JavaError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| JavaError::Manifest("not JSON"))?;
    let files = v
        .get("files")
        .and_then(Value::as_object)
        .ok_or(JavaError::Manifest("no file list"))?;
    if files.len() > MAX_FILES {
        return Err(JavaError::Manifest("too many files"));
    }
    let (mut dirs, mut out, mut links, mut total) = (Vec::new(), Vec::new(), Vec::new(), 0u64);
    for (name, entry) in files {
        let path = RelPath::new(name).map_err(|_| JavaError::Manifest("unsafe path"))?;
        match entry.get("type").and_then(Value::as_str) {
            Some("directory") => dirs.push(path),
            Some("file") => {
                let raw = entry
                    .pointer("/downloads/raw")
                    .and_then(download_of)
                    .ok_or(JavaError::Manifest("file without a verifiable download"))?;
                let executable = entry
                    .get("executable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                total = total.saturating_add(raw.size);
                out.push(RuntimeFile {
                    path,
                    download: raw,
                    executable,
                });
            }
            Some("link") => {
                let target_raw = entry
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or(JavaError::Manifest("link without target"))?;
                let target = resolve_link_target(&path, target_raw)?;
                links.push(RuntimeLink { path, target });
            }
            _ => return Err(JavaError::Manifest("unknown manifest entry type")),
        }
    }
    if total > MAX_TOTAL_BYTES {
        return Err(JavaError::Manifest("too large"));
    }
    let concrete_paths: HashSet<&str> = dirs
        .iter()
        .map(RelPath::as_str)
        .chain(out.iter().map(|f| f.path.as_str()))
        .collect();
    for l in &links {
        if l.path == l.target
            || concrete_paths.contains(l.path.as_str())
            || !concrete_paths.contains(l.target.as_str())
        {
            return Err(JavaError::Manifest("dangling or chained link target"));
        }
    }
    if !out
        .iter()
        .any(|f| f.path.as_str() == JAVA_EXE || JAVAW_EXE == Some(f.path.as_str()))
    {
        return Err(JavaError::Manifest("no java launch executable"));
    }
    Ok(ParsedFiles {
        dirs,
        files: out,
        links,
    })
}

/// Directory name of a runtime: `<component>--<version>--<platform>`.
pub fn runtime_id(component: &str, version_name: &str) -> String {
    let v: String = version_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{component}--{v}--{RUNTIME_SUFFIX}")
}

/// A verified runtime of this component that is already installed (newest version first). No network.
pub fn find_installed(root: &PortableRoot, component: &str) -> Option<ManagedJava> {
    fn extract_ver<'a>(id: &'a str, prefix: &str, suffix: &str) -> &'a str {
        let rest = id.strip_prefix(prefix).unwrap_or(id);
        rest.strip_suffix(suffix).unwrap_or(rest)
    }
    let prefix = format!("{component}--");
    let suffix = format!("--{RUNTIME_SUFFIX}");
    let mut ids: Vec<String> = fs::read_dir(root.java_runtimes_dir())
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(&suffix))
        .collect();
    ids.sort_by(|a, b| {
        crate::loaders::cmp_versions(
            extract_ver(a, &prefix, &suffix),
            extract_ver(b, &prefix, &suffix),
        )
        .then_with(|| a.cmp(b))
    });
    ids.into_iter()
        .rev()
        .find_map(|id| resolve_java(root, &id).ok())
}

/// Fetches and checks the runtime description for `component` (needs the network).
pub async fn plan_runtime<F: Fetcher>(
    fetcher: &F,
    component: &str,
) -> Result<RuntimePlan, JavaError> {
    let index_url = url::Url::parse(RUNTIME_INDEX_URL).map_err(|_| JavaError::Index("bad url"))?;
    let index = fetcher.get(&index_url, MAX_INDEX_BYTES).await?;
    let release = parse_index(&index, component)?;
    let body = fetcher
        .get(&release.manifest.url, MAX_MANIFEST_BYTES)
        .await?;
    if hex::encode(Sha1::digest(&body)) != release.manifest.sha1 {
        return Err(JavaError::ManifestHash);
    }
    let parsed = parse_files(&body)?;
    Ok(RuntimePlan {
        release,
        dirs: parsed.dirs,
        files: parsed.files,
        links: parsed.links,
    })
}

fn copy_dir_no_links(root: &Path, src: &Path, dst: &Path) -> Result<(), JavaError> {
    fsutil::reject_links_between(root, src)?;
    fsutil::reject_links_between(root, dst)?;
    if fsutil::is_link(src)? {
        return Err(JavaError::Manifest("refusing to follow symlink"));
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let sp = entry.path();
        if fsutil::is_link(&sp)? {
            return Err(JavaError::Manifest("refusing to follow symlink"));
        }
        let dp = dst.join(entry.file_name());
        if fsutil::is_link(&dp).unwrap_or(false) {
            return Err(JavaError::Manifest("refusing to overwrite symlink"));
        }
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir_no_links(root, &sp, &dp)?;
        } else if ft.is_file() {
            fs::copy(&sp, &dp)?;
        }
    }
    Ok(())
}

/// Applies Unix executable permissions from `files` and materializes verified intra-runtime
/// `links` as regular copies without ever creating OS symlinks.
pub fn finalize_runtime_tree(
    root: &PortableRoot,
    home: &Path,
    files: &[RuntimeFile],
    links: &[RuntimeLink],
) -> Result<(), JavaError> {
    fsutil::reject_links_between(root.path(), home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in files {
            if f.executable || f.path.as_str() == JAVA_EXE || f.path.as_str().starts_with("bin/") {
                let p = f.path.to_path(home);
                fsutil::reject_links_between(root.path(), &p)?;
                if p.is_file() {
                    fs::set_permissions(&p, fs::Permissions::from_mode(0o755))?;
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = files;

    for link in links {
        let src = link.target.to_path(home);
        let dst = link.path.to_path(home);
        fsutil::reject_links_between(root.path(), &src)?;
        fsutil::reject_links_between(root.path(), &dst)?;
        if fsutil::is_link(&src).unwrap_or(false) || fsutil::is_link(&dst).unwrap_or(false) {
            return Err(JavaError::Manifest("refusing to follow symlink"));
        }
        if src.is_dir() {
            copy_dir_no_links(root.path(), &src, &dst)?;
        } else if src.is_file() {
            if let Some(parent) = dst.parent() {
                fsutil::reject_links_between(root.path(), parent)?;
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dst)?;
            if let Ok(md) = fs::metadata(&src) {
                let _ = fs::set_permissions(&dst, md.permissions());
            }
        } else {
            return Err(JavaError::Manifest("missing link target after download"));
        }
    }
    Ok(())
}

/// Re-applies executable permissions and recorded intra-runtime copies after full verification.
pub fn reapply_runtime_metadata(
    root: &PortableRoot,
    java: &ManagedJava,
    files: &[RuntimeFile],
) -> Result<(), JavaError> {
    let links = recorded_links(root, &java.id);
    finalize_runtime_tree(root, &java.home, files, &links)
}

/// Installs the planned runtime (verified, resumable) and returns it ready to use.
pub async fn install_runtime<S: Streamer>(
    root: &PortableRoot,
    plan: &RuntimePlan,
    java_major: u32,
    downloader: &Downloader<'_, S>,
    concurrency: usize,
) -> Result<ManagedJava, JavaError> {
    let id = runtime_id(&plan.release.component, &plan.release.version_name);
    let target = runtime_dir(root, &id).map_err(|_| JavaError::Manifest("bad runtime id"))?;
    let staging = root.cache_dir().join(STAGING).join(&id);

    let already_installed = {
        let root = root.clone();
        let id = id.clone();
        let target = target.clone();
        let staging = staging.clone();
        let dirs = plan.dirs.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<ManagedJava>, JavaError> {
            if let Ok(java) = resolve_java(&root, &id) {
                return Ok(Some(java));
            }
            if target.exists() {
                fsutil::reject_links_between(root.path(), &target)?;
                fsutil::remove_dir_all_with_retry(&target)?; // present but failing its check: reinstall
            }
            fsutil::reject_links_between(root.path(), &staging)?;
            fs::create_dir_all(&staging)?;
            fsutil::reject_links_between(root.path(), &staging)?;
            for d in &dirs {
                let p = d.to_path(&staging);
                fsutil::reject_links_between(root.path(), &p)?;
                fs::create_dir_all(&p)?;
            }
            Ok(None)
        })
        .await
        .map_err(io::Error::other)??
    };
    if let Some(java) = already_installed {
        return Ok(java);
    }

    let jobs = plan
        .files
        .iter()
        .map(|f| FileJob {
            url: f.download.url.clone(),
            dest: f.path.to_path(&staging),
            expected: Expected::Sha1(f.download.sha1.clone()),
            size: Some(f.download.size),
        })
        .collect();
    downloader.fetch_all(jobs, concurrency).await?;

    let has_javaw = JAVAW_EXE.is_some_and(|j| plan.files.iter().any(|f| f.path.as_str() == j));
    let has_java = plan.files.iter().any(|f| f.path.as_str() == JAVA_EXE);
    let chosen_exe = if has_javaw {
        JAVAW_EXE.unwrap_or(JAVA_EXE)
    } else {
        JAVA_EXE
    };

    let root = root.clone();
    let files = plan.files.clone();
    let links = plan.links.clone();
    tokio::task::spawn_blocking(move || -> Result<ManagedJava, JavaError> {
        finalize_runtime_tree(&root, &staging, &files, &links)?;
        let java_exe = RelPath::new(chosen_exe).map_err(|_| JavaError::Manifest("bad exe path"))?;
        let java_exe_sha256 = sha256_hex(&java_exe.to_path(&staging))?;
        let (java_cli_exe, java_cli_exe_sha256) = if has_javaw && has_java {
            let cli = RelPath::new(JAVA_EXE).map_err(|_| JavaError::Manifest("bad exe path"))?;
            let sha = sha256_hex(&staging.join(JAVA_EXE))?;
            (Some(cli), Some(sha))
        } else {
            (None, None)
        };
        let manifest = RuntimeManifest {
            schema: 1,
            id: id.clone(),
            vendor: "mojang".into(),
            java_major,
            arch: "x64".into(),
            java_exe,
            java_exe_sha256,
            java_cli_exe,
            java_cli_exe_sha256,
        };
        atomic_write(&staging.join(FILES_RECORD), &record_bytes(&files, &links)?)?;
        atomic_write(
            &staging.join("runtime.json"),
            &serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?,
        )?;
        if let Some(parent) = target.parent() {
            fsutil::reject_links_between(root.path(), parent)?;
            fs::create_dir_all(parent)?;
        }
        fsutil::reject_links_between(root.path(), &target)?;
        if target.exists() {
            fsutil::remove_dir_all_with_retry(&target)?;
        }
        fsutil::rename_with_retry(&staging, &target)?;
        resolve_java(&root, &id)
            .map_err(|_| JavaError::Manifest("installed runtime failed its check"))
    })
    .await
    .map_err(io::Error::other)?
}

// ------------------------------------------------------------------------------ full verification

#[derive(Serialize, Deserialize)]
struct RecordedFile {
    path: String,
    url: String,
    sha1: String,
    size: u64,
    #[serde(default)]
    executable: bool,
}

#[derive(Serialize, Deserialize)]
struct RecordedLink {
    path: String,
    target: String,
}

#[derive(Serialize, Deserialize)]
struct FilesRecord {
    schema: u32,
    files: Vec<RecordedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<RecordedLink>,
}

fn record_bytes(files: &[RuntimeFile], links: &[RuntimeLink]) -> io::Result<Vec<u8>> {
    let record = FilesRecord {
        schema: 1,
        files: files
            .iter()
            .map(|f| RecordedFile {
                path: f.path.as_str().to_owned(),
                url: f.download.url.to_string(),
                sha1: f.download.sha1.clone(),
                size: f.download.size,
                executable: f.executable,
            })
            .collect(),
        links: links
            .iter()
            .map(|l| RecordedLink {
                path: l.path.as_str().to_owned(),
                target: l.target.as_str().to_owned(),
            })
            .collect(),
    };
    serde_json::to_vec_pretty(&record).map_err(io::Error::other)
}

fn read_record(root: &PortableRoot, id: &str) -> Option<FilesRecord> {
    let dir = runtime_dir(root, id).ok()?;
    fsutil::reject_links_between(root.path(), &dir).ok()?;
    let bytes = fsutil::read_bounded(&dir.join(FILES_RECORD), MAX_MANIFEST_BYTES as u64).ok()?;
    let record: FilesRecord = serde_json::from_slice(&bytes).ok()?;
    if record.schema != 1 || record.files.len() > MAX_FILES || record.links.len() > MAX_FILES {
        return None;
    }
    Some(record)
}

/// The files recorded when `id` was installed. `None` for a runtime installed before the record
/// existed, or when the record is unreadable or fails the same checks as a fresh manifest (a bad
/// entry means the whole record is not trusted).
pub fn recorded_files(root: &PortableRoot, id: &str) -> Option<Vec<RuntimeFile>> {
    let record = read_record(root, id)?;
    record
        .files
        .into_iter()
        .map(|f| {
            Some(RuntimeFile {
                path: RelPath::new(&f.path).ok()?,
                download: download_of(&serde_json::json!({
                    "sha1": f.sha1,
                    "size": f.size,
                    "url": f.url,
                }))?,
                executable: f.executable,
            })
        })
        .collect()
}

pub fn recorded_links(root: &PortableRoot, id: &str) -> Vec<RuntimeLink> {
    let Some(record) = read_record(root, id) else {
        return Vec::new();
    };
    record
        .links
        .into_iter()
        .filter_map(|l| {
            Some(RuntimeLink {
                path: RelPath::new(&l.path).ok()?,
                target: RelPath::new(&l.target).ok()?,
            })
        })
        .collect()
}

/// The authoritative file list for fully verifying an installed runtime, or `None` when there is
/// none to be had (and then verification stays limited to `java_exe`, which `resolve_java` always
/// checks):
/// - the list recorded when the runtime was installed, which needs no network; or
/// - for a runtime installed before that record existed, Mojang's current manifest for `component`,
///   but only if it describes exactly this installed version. The list is then recorded for next time.
///
/// Files that Mojang's manifest does not cover (anything the runtime folder gained later) are not
/// judged: no hash is invented for them.
pub async fn verification_files<F: Fetcher>(
    root: &PortableRoot,
    java: &ManagedJava,
    component: Option<&str>,
    fetcher: &F,
) -> Option<Vec<RuntimeFile>> {
    let recorded = {
        let root = root.clone();
        let id = java.id.clone();
        tokio::task::spawn_blocking(move || recorded_files(&root, &id))
            .await
            .ok()
            .flatten()
    };
    if let Some(files) = recorded {
        return Some(files);
    }
    let plan = plan_runtime(fetcher, component?).await.ok()?;
    if runtime_id(&plan.release.component, &plan.release.version_name) != java.id {
        return None; // a newer runtime exists; this older one has no authoritative list any more
    }
    let files = plan.files;
    let files_for_record = files.clone();
    let links_for_record = plan.links;
    let home = java.home.clone();
    let root = root.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if fsutil::reject_links_between(root.path(), &home).is_ok() {
            if let Ok(bytes) = record_bytes(&files_for_record, &links_for_record) {
                let _ = atomic_write(&home.join(FILES_RECORD), &bytes); // best effort
            }
        }
    })
    .await;
    Some(files)
}

/// Download jobs that check every listed file of an installed runtime and replace the missing or
/// damaged ones (the downloader keeps files that verify and fetches only what does not).
pub fn verification_jobs(java: &ManagedJava, files: &[RuntimeFile]) -> Vec<FileJob> {
    files
        .iter()
        .map(|f| FileJob {
            url: f.download.url.clone(),
            dest: f.path.to_path(&java.home),
            expected: Expected::Sha1(f.download.sha1.clone()),
            size: Some(f.download.size),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::{Cancel, Progress};
    use crate::net::testing::FakeFetcher;
    use crate::net::testing_stream::FakeStreamer;
    use crate::runtimes::{resolve_cli_java_exe, write_manifest};
    use serde_json::json;
    use sha2::Sha256;
    use std::time::Duration;

    const BASE: &str = "https://piston-data.mojang.com/v1/objects";

    fn sha1_of(b: &[u8]) -> String {
        hex::encode(Sha1::digest(b))
    }

    struct World {
        root: PortableRoot,
        _dir: tempfile::TempDir,
        fetcher: FakeFetcher,
        streamer: FakeStreamer,
        files: Vec<(&'static str, Vec<u8>)>,
    }

    /// A tiny runtime with the same shape as Mojang's: index -> manifest -> raw files.
    fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(dir.path());
        root.ensure_layout().unwrap();
        let files: Vec<(&'static str, Vec<u8>)> = if cfg!(windows) {
            vec![
                ("bin/java.exe", vec![1u8; 300]),
                ("bin/javaw.exe", vec![2u8; 200]),
                ("lib/modules", vec![3u8; 5000]),
            ]
        } else {
            vec![
                ("bin/java", vec![1u8; 300]),
                ("lib/jspawnhelper", vec![2u8; 200]),
                ("lib/modules", vec![3u8; 5000]),
            ]
        };
        let fetcher = FakeFetcher::new();
        let streamer = FakeStreamer::new();
        let mut entries = serde_json::Map::new();
        entries.insert("bin".into(), json!({"type": "directory"}));
        entries.insert("lib".into(), json!({"type": "directory"}));
        for (path, data) in &files {
            let url = format!("{BASE}/{}/{path}", sha1_of(data));
            streamer.add(&url, data);
            let exe = *path != "lib/modules";
            entries.insert(
                (*path).into(),
                json!({"type": "file", "executable": exe, "downloads": {"raw": {"sha1": sha1_of(data), "size": data.len(), "url": url}}}),
            );
        }
        let manifest = json!({"files": entries}).to_string();
        let manifest_url = "https://piston-meta.mojang.com/v1/packages/m/manifest.json";
        fetcher.ok(manifest_url, &manifest);
        let index = json!({INDEX_PLATFORM: {"java-runtime-delta": [{
            "availability": {"group": 1, "progress": 100},
            "manifest": {"sha1": sha1_of(manifest.as_bytes()), "size": manifest.len(), "url": manifest_url},
            "version": {"name": "21.0.7", "released": "2025-05-19T08:34:42+00:00"}}]}})
        .to_string();
        fetcher.ok(RUNTIME_INDEX_URL, &index);
        World {
            root,
            _dir: dir,
            fetcher,
            streamer,
            files,
        }
    }

    fn downloader<'a>(w: &'a World) -> Downloader<'a, FakeStreamer> {
        Downloader::new(&w.streamer, Cancel::new(), Progress::new()).with_backoff(Duration::ZERO)
    }

    #[tokio::test]
    async fn installs_a_runtime_verifies_it_and_reuses_it_without_the_network() {
        let w = world();
        let plan = plan_runtime(&w.fetcher, "java-runtime-delta")
            .await
            .unwrap();
        assert_eq!(plan.total_bytes(), 300 + 200 + 5000);
        let java = install_runtime(&w.root, &plan, 21, &downloader(&w), 4)
            .await
            .unwrap();
        assert_eq!(
            java.id,
            format!("java-runtime-delta--21.0.7--{RUNTIME_SUFFIX}")
        );
        assert_eq!(java.java_major, 21);
        let want_primary = JAVAW_EXE.unwrap_or(JAVA_EXE);
        assert!(
            java.exe.ends_with(Path::new(want_primary)),
            "{}",
            java.exe.display()
        );
        let cli = resolve_cli_java_exe(&w.root, &java).unwrap();
        assert!(cli.ends_with(Path::new(JAVA_EXE)), "{}", cli.display());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin_mode = fs::metadata(&cli).unwrap().permissions().mode() & 0o777;
            let helper_mode = fs::metadata(java.home.join("lib").join("jspawnhelper"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(bin_mode & 0o111, 0o111, "java binary must be executable");
            assert_eq!(
                helper_mode & 0o111,
                0o111,
                "jspawnhelper must be executable"
            );
        }
        assert_eq!(
            fs::read(java.home.join("lib").join("modules")).unwrap(),
            w.files[2].1
        );
        assert!(
            !w.root.cache_dir().join(STAGING).join(&java.id).exists(),
            "staging was moved into place"
        );

        // Offline from here on: found without any request.
        let calls_before = w.streamer.total_calls();
        let again = find_installed(&w.root, "java-runtime-delta").unwrap();
        assert_eq!(again.id, java.id);
        assert!(find_installed(&w.root, "java-runtime-gamma").is_none());
        assert_eq!(w.streamer.total_calls(), calls_before);
    }

    // ---- full verification

    async fn installed(w: &World) -> (RuntimePlan, ManagedJava) {
        let plan = plan_runtime(&w.fetcher, "java-runtime-delta")
            .await
            .unwrap();
        let java = install_runtime(&w.root, &plan, 21, &downloader(w), 4)
            .await
            .unwrap();
        (plan, java)
    }

    async fn repair(w: &World, java: &ManagedJava, files: &[RuntimeFile]) -> usize {
        downloader(w)
            .fetch_all(verification_jobs(java, files), 4)
            .await
            .unwrap()
            .downloaded
    }

    #[tokio::test]
    async fn the_install_records_every_file_and_a_damaged_runtime_is_found_and_repaired() {
        let w = world();
        let (plan, java) = installed(&w).await;
        let recorded = recorded_files(&w.root, &java.id).expect("the file list was recorded");
        assert_eq!(recorded, plan.files);

        // Primary launch exe is intact, so the everyday check still accepts the runtime...
        fs::write(java.home.join("lib").join("modules"), vec![9u8; 5000]).unwrap(); // same size
        let secondary_idx = usize::from(!cfg!(windows));
        let secondary_rel = w.files[secondary_idx].0;
        fs::remove_file(java.home.join(secondary_rel)).unwrap();
        assert!(
            find_installed(&w.root, "java-runtime-delta").is_some(),
            "the cheap launch-time check only looks at the primary launch exe"
        );
        if cfg!(windows) {
            assert!(
                resolve_cli_java_exe(&w.root, &java).is_err(),
                "the headless CLI binary check notices the missing java.exe"
            );
        }
        // ...but a full verification covers every file, offline (the record is local)
        let offline = FakeFetcher::new();
        let files = verification_files(&w.root, &java, Some("java-runtime-delta"), &offline)
            .await
            .unwrap();
        let before = w.streamer.total_calls();
        assert_eq!(
            repair(&w, &java, &files).await,
            2,
            "exactly the damaged files"
        );
        reapply_runtime_metadata(&w.root, &java, &files).unwrap();
        assert_eq!(w.streamer.total_calls(), before + 2);
        assert_eq!(
            fs::read(java.home.join("lib").join("modules")).unwrap(),
            w.files[2].1
        );
        assert_eq!(
            fs::read(java.home.join(secondary_rel)).unwrap(),
            w.files[secondary_idx].1
        );
        // a second run finds nothing to do
        assert_eq!(repair(&w, &java, &files).await, 0);
        // and both launch-time and CLI checks succeed again
        assert!(resolve_java(&w.root, &java.id).is_ok());
        assert!(resolve_cli_java_exe(&w.root, &java).is_ok());
    }

    #[tokio::test]
    async fn an_older_install_without_a_record_is_verified_from_mojangs_manifest_when_it_matches() {
        let w = world();
        let (plan, java) = installed(&w).await;
        fs::remove_file(java.home.join(FILES_RECORD)).unwrap(); // as installed by an earlier Clade
        assert!(recorded_files(&w.root, &java.id).is_none());

        // offline: nothing authoritative to check against, so only java_exe is covered
        let offline = FakeFetcher::new();
        assert!(
            verification_files(&w.root, &java, Some("java-runtime-delta"), &offline)
                .await
                .is_none()
        );
        // the user-chosen runtime case has no component to look up either
        assert!(verification_files(&w.root, &java, None, &w.fetcher)
            .await
            .is_none());

        // online and the same version: the manifest is used, and recorded for next time
        let files = verification_files(&w.root, &java, Some("java-runtime-delta"), &w.fetcher)
            .await
            .unwrap();
        assert_eq!(files, plan.files);
        assert!(recorded_files(&w.root, &java.id).is_some());

        // online but Mojang now offers a newer version: this install has no list, nothing is invented
        fs::remove_file(java.home.join(FILES_RECORD)).unwrap();
        let newer = FakeFetcher::new();
        let manifest = serde_json::json!({
            "files": {
                JAVA_EXE: {
                    "type": "file",
                    "downloads": {"raw": {"sha1": "0123456789abcdef0123456789abcdef01234567", "size": 1, "url": "https://piston-data.mojang.com/x"}}
                }
            }
        })
        .to_string();
        let manifest_url = "https://piston-meta.mojang.com/v1/packages/m/manifest.json";
        let index = serde_json::json!({
            INDEX_PLATFORM: {
                "java-runtime-delta": [{
                    "manifest": {"sha1": sha1_of(manifest.as_bytes()), "size": manifest.len(), "url": manifest_url},
                    "version": {"name": "21.0.9"}
                }]
            }
        })
        .to_string();
        newer.ok(RUNTIME_INDEX_URL, index);
        newer.ok(manifest_url, manifest);
        assert!(
            plan_runtime(&newer, "java-runtime-delta").await.is_ok(),
            "the newer plan itself is valid"
        );
        assert!(
            verification_files(&w.root, &java, Some("java-runtime-delta"), &newer)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_tampered_record_is_not_trusted() {
        let w = world();
        let (_plan, java) = installed(&w).await;
        let path = java.home.join(FILES_RECORD);
        for bad in [
            "not json".to_owned(),
            r#"{"schema":1,"files":[{"path":"../x.dll","url":"https://piston-data.mojang.com/x","sha1":"0123456789abcdef0123456789abcdef01234567","size":1}]}"#.to_owned(),
            r#"{"schema":1,"files":[{"path":"a.dll","url":"https://piston-data.mojang.com/x","sha1":"zz","size":1}]}"#.to_owned(),
        ] {
            fs::write(&path, bad).unwrap();
            assert!(recorded_files(&w.root, &java.id).is_none());
        }
    }

    #[tokio::test]
    async fn a_tampered_java_executable_is_not_reused_and_is_reinstalled() {
        let w = world();
        let plan = plan_runtime(&w.fetcher, "java-runtime-delta")
            .await
            .unwrap();
        let java = install_runtime(&w.root, &plan, 21, &downloader(&w), 4)
            .await
            .unwrap();
        fs::write(&java.exe, b"swapped").unwrap();
        assert!(find_installed(&w.root, "java-runtime-delta").is_none());
        let fixed = install_runtime(&w.root, &plan, 21, &downloader(&w), 4)
            .await
            .unwrap();
        let primary_idx = usize::from(cfg!(windows));
        assert_eq!(fs::read(&fixed.exe).unwrap(), w.files[primary_idx].1);
    }

    #[tokio::test]
    async fn an_interrupted_install_keeps_verified_files_and_finishes_later() {
        let w = world();
        let plan = plan_runtime(&w.fetcher, "java-runtime-delta")
            .await
            .unwrap();
        let modules_url = format!("{BASE}/{}/lib/modules", sha1_of(&w.files[2].1));
        w.streamer.answer_status(&modules_url, 404); // permanent failure part-way
        assert!(install_runtime(&w.root, &plan, 21, &downloader(&w), 1)
            .await
            .is_err());
        let id = runtime_id("java-runtime-delta", "21.0.7");
        assert!(
            !runtime_dir(&w.root, &id).unwrap().exists(),
            "no half-installed runtime"
        );
        assert!(find_installed(&w.root, "java-runtime-delta").is_none());
        let staged = w.root.cache_dir().join(STAGING).join(&id).join(JAVA_EXE);
        assert!(staged.is_file(), "verified files survive for the retry");
        w.streamer.clear_status(&modules_url);
        let before = w
            .streamer
            .count(&format!("{BASE}/{}/{JAVA_EXE}", sha1_of(&w.files[0].1)));
        install_runtime(&w.root, &plan, 21, &downloader(&w), 1)
            .await
            .unwrap();
        let after = w
            .streamer
            .count(&format!("{BASE}/{}/{JAVA_EXE}", sha1_of(&w.files[0].1)));
        assert_eq!(
            before, after,
            "already verified files were not downloaded again"
        );
    }

    #[tokio::test]
    async fn a_manifest_that_does_not_match_its_hash_is_refused() {
        let w = world();
        w.fetcher.ok(
            "https://piston-meta.mojang.com/v1/packages/m/manifest.json",
            r#"{"files":{}}"#,
        );
        assert!(matches!(
            plan_runtime(&w.fetcher, "java-runtime-delta").await,
            Err(JavaError::ManifestHash)
        ));
        assert!(matches!(
            plan_runtime(&w.fetcher, "java-runtime-nope").await,
            Err(JavaError::NoSuchRuntime(_))
        ));
    }

    #[test]
    fn hostile_manifests_are_refused() {
        let ok = json!({"type":"file","downloads":{"raw":{"sha1":"0123456789abcdef0123456789abcdef01234567","size":1,"url":"https://piston-data.mojang.com/x"}}});
        let make =
            |name: &str, e: Value| json!({"files": {JAVA_EXE: ok.clone(), name: e}}).to_string();
        assert!(parse_files(make("fine.txt", ok.clone()).as_bytes()).is_ok());
        if let Some(javaw) = JAVAW_EXE {
            assert!(
                parse_files(json!({"files": {javaw: ok.clone()}}).to_string().as_bytes()).is_ok(),
                "javaw.exe alone is accepted"
            );
        }
        assert!(parse_files(make("../escape.dll", ok.clone()).as_bytes()).is_err());
        assert!(parse_files(make("C:/x.dll", ok.clone()).as_bytes()).is_err());
        for bad_target in ["/etc", "../../etc/passwd", "C:\\x", "missing/file"] {
            assert!(
                parse_files(
                    make("bad/link", json!({"type":"link","target":bad_target})).as_bytes()
                )
                .is_err(),
                "{bad_target}"
            );
        }
        assert!(parse_files(
            make(
                "nohash",
                json!({"type":"file","downloads":{"raw":{"size":1,"url":"https://x"}}})
            )
            .as_bytes()
        )
        .is_err());
        assert!(
            parse_files(br#"{"files":{"a":{"type":"directory"}}}"#).is_err(),
            "must contain launch executable"
        );
    }

    #[tokio::test]
    async fn intra_runtime_links_are_materialized_without_os_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(dir.path());
        root.ensure_layout().unwrap();
        let exe_bytes = vec![1u8; 256];
        let license_bytes = b"Apache License".to_vec();
        let man_bytes = b"manual page".to_vec();
        let exe_url = format!("{BASE}/{}/{JAVA_EXE}", sha1_of(&exe_bytes));
        let lic_url = format!("{BASE}/{}/legal/base/LICENSE", sha1_of(&license_bytes));
        let man_url = format!("{BASE}/{}/man/ja_JP/java.1", sha1_of(&man_bytes));
        let streamer = FakeStreamer::new();
        streamer.add(&exe_url, &exe_bytes);
        streamer.add(&lic_url, &license_bytes);
        streamer.add(&man_url, &man_bytes);
        let fetcher = FakeFetcher::new();
        let manifest = json!({
            "files": {
                "bin": {"type": "directory"},
                "legal": {"type": "directory"},
                "legal/base": {"type": "directory"},
                "legal/mod": {"type": "directory"},
                "man": {"type": "directory"},
                "man/ja_JP": {"type": "directory"},
                JAVA_EXE: {"type": "file", "executable": true, "downloads": {"raw": {"sha1": sha1_of(&exe_bytes), "size": exe_bytes.len(), "url": exe_url}}},
                "legal/base/LICENSE": {"type": "file", "executable": false, "downloads": {"raw": {"sha1": sha1_of(&license_bytes), "size": license_bytes.len(), "url": lic_url}}},
                "legal/mod/LICENSE": {"type": "link", "target": "../base/LICENSE"},
                "man/ja_JP/java.1": {"type": "file", "executable": false, "downloads": {"raw": {"sha1": sha1_of(&man_bytes), "size": man_bytes.len(), "url": man_url}}},
                "man/ja": {"type": "link", "target": "ja_JP"},
            }
        })
        .to_string();
        let manifest_url = "https://piston-meta.mojang.com/v1/packages/l/manifest.json";
        fetcher.ok(manifest_url, &manifest);
        fetcher.ok(
            RUNTIME_INDEX_URL,
            json!({INDEX_PLATFORM: {"java-runtime-delta": [{
                "manifest": {"sha1": sha1_of(manifest.as_bytes()), "size": manifest.len(), "url": manifest_url},
                "version": {"name": "21.0.8"}
            }]}})
            .to_string(),
        );
        let plan = plan_runtime(&fetcher, "java-runtime-delta").await.unwrap();
        assert_eq!(plan.links.len(), 2);
        let dl =
            Downloader::new(&streamer, Cancel::new(), Progress::new()).with_backoff(Duration::ZERO);
        let java = install_runtime(&root, &plan, 21, &dl, 2).await.unwrap();
        let copied_file = java.home.join("legal").join("mod").join("LICENSE");
        let copied_dir_file = java.home.join("man").join("ja").join("java.1");
        assert!(!fsutil::is_link(&copied_file).unwrap());
        assert!(!fsutil::is_link(java.home.join("man").join("ja").as_path()).unwrap());
        assert_eq!(fs::read(&copied_file).unwrap(), license_bytes);
        assert_eq!(fs::read(&copied_dir_file).unwrap(), man_bytes);
        assert_eq!(recorded_links(&root, &java.id).len(), 2);
    }

    #[test]
    fn runtime_ids_are_safe_single_directory_names() {
        let id = runtime_id("jre-legacy", "8u51-cacert462b08");
        assert_eq!(
            id,
            format!("jre-legacy--8u51-cacert462b08--{RUNTIME_SUFFIX}")
        );
        assert!(runtime_id("x", "a/b\\c d")
            .chars()
            .all(|c| c != '/' && c != '\\' && c != ' '));
        let d = tempfile::tempdir().unwrap();
        assert!(runtime_dir(&PortableRoot::at(d.path()), &id).is_ok());
    }

    #[test]
    fn find_installed_sorts_versions_numerically_so_21_0_10_beats_21_0_9() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        let exe_bytes = b"MZ javaw";
        let sha = hex::encode(Sha256::digest(exe_bytes));
        let primary = JAVAW_EXE.unwrap_or(JAVA_EXE);

        for ver in ["21.0.7", "21.0.9", "21.0.10-beta.1", "21.0.10"] {
            let id = runtime_id("java-runtime-delta", ver);
            let dir = runtime_dir(&root, &id).unwrap();
            let exe_path = RelPath::new(primary).unwrap().to_path(&dir);
            fs::create_dir_all(exe_path.parent().unwrap()).unwrap();
            fs::write(&exe_path, exe_bytes).unwrap();
            write_manifest(
                &root,
                &RuntimeManifest {
                    schema: 1,
                    id,
                    vendor: "mojang".into(),
                    java_major: 21,
                    arch: "x64".into(),
                    java_exe: RelPath::new(primary).unwrap(),
                    java_exe_sha256: sha.clone(),
                    java_cli_exe: None,
                    java_cli_exe_sha256: None,
                },
            )
            .unwrap();
        }

        let picked = find_installed(&root, "java-runtime-delta").unwrap();
        assert_eq!(
            picked.id,
            format!("java-runtime-delta--21.0.10--{RUNTIME_SUFFIX}")
        );
    }

    #[tokio::test]
    async fn installs_console_only_runtime_when_javaw_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(dir.path());
        root.ensure_layout().unwrap();
        let java_bytes = vec![7u8; 250];
        let url = format!("{BASE}/{}/{JAVA_EXE}", sha1_of(&java_bytes));
        let streamer = FakeStreamer::new();
        streamer.add(&url, &java_bytes);
        let fetcher = FakeFetcher::new();
        let manifest = json!({
            "files": {
                "bin": {"type": "directory"},
                JAVA_EXE: {
                    "type": "file",
                    "downloads": {"raw": {"sha1": sha1_of(&java_bytes), "size": java_bytes.len(), "url": url}}
                }
            }
        })
        .to_string();
        let manifest_url = "https://piston-meta.mojang.com/v1/packages/c/manifest.json";
        fetcher.ok(manifest_url, &manifest);
        fetcher.ok(
            RUNTIME_INDEX_URL,
            json!({INDEX_PLATFORM: {"jre-legacy": [{
                "manifest": {"sha1": sha1_of(manifest.as_bytes()), "size": manifest.len(), "url": manifest_url},
                "version": {"name": "8.0.51"}
            }]}})
            .to_string(),
        );
        let plan = plan_runtime(&fetcher, "jre-legacy").await.unwrap();
        let dl =
            Downloader::new(&streamer, Cancel::new(), Progress::new()).with_backoff(Duration::ZERO);
        let java = install_runtime(&root, &plan, 8, &dl, 2).await.unwrap();
        assert!(
            java.exe.ends_with(Path::new(JAVA_EXE)),
            "{}",
            java.exe.display()
        );
        assert_eq!(resolve_cli_java_exe(&root, &java).unwrap(), java.exe);
    }
}
