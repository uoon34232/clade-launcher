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
//! `java.exe` still matches the hash recorded at install time ([`resolve_java`]).

use std::fs;
use std::io;

use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::download::{DownloadError, Downloader, FileJob};
use crate::mc_meta::Download;
use crate::net::{FetchError, Fetcher, Streamer};
use crate::portable::fsutil::{atomic_write, sha256_hex, Expected};
use crate::portable::{PortableRoot, RelPath};
use crate::runtimes::{resolve_java, runtime_dir, ManagedJava, RuntimeManifest};

pub const RUNTIME_INDEX_URL: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const PLATFORM: &str = "windows-x64";
const MAX_INDEX_BYTES: usize = 2 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const JAVA_EXE: &str = "bin/java.exe";
const STAGING: &str = "java-staging";

#[derive(Debug, thiserror::Error)]
pub enum JavaError {
    #[error("Mojang's Java runtime list could not be read: {0}")]
    Index(&'static str),
    #[error("Mojang offers no Java runtime '{0}' for 64-bit Windows")]
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
}

/// Everything needed to install one runtime, known before any file is downloaded.
#[derive(Debug, Clone)]
pub struct RuntimePlan {
    pub release: RuntimeRelease,
    pub dirs: Vec<RelPath>,
    pub files: Vec<RuntimeFile>,
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

/// Picks `component` for 64-bit Windows out of Mojang's runtime index.
pub fn parse_index(body: &[u8], component: &str) -> Result<RuntimeRelease, JavaError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| JavaError::Index("not JSON"))?;
    let release = v
        .get(PLATFORM)
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

/// Parses a component manifest. Links are refused (a runtime has none on Windows, and a link
/// is how an archive escapes its folder).
pub fn parse_files(body: &[u8]) -> Result<(Vec<RelPath>, Vec<RuntimeFile>), JavaError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| JavaError::Manifest("not JSON"))?;
    let files = v
        .get("files")
        .and_then(Value::as_object)
        .ok_or(JavaError::Manifest("no file list"))?;
    if files.len() > MAX_FILES {
        return Err(JavaError::Manifest("too many files"));
    }
    let (mut dirs, mut out, mut total) = (Vec::new(), Vec::new(), 0u64);
    for (name, entry) in files {
        let path = RelPath::new(name).map_err(|_| JavaError::Manifest("unsafe path"))?;
        match entry.get("type").and_then(Value::as_str) {
            Some("directory") => dirs.push(path),
            Some("file") => {
                let raw = entry
                    .pointer("/downloads/raw")
                    .and_then(download_of)
                    .ok_or(JavaError::Manifest("file without a verifiable download"))?;
                total = total.saturating_add(raw.size);
                out.push(RuntimeFile {
                    path,
                    download: raw,
                });
            }
            _ => {
                return Err(JavaError::Manifest(
                    "links and unknown entries are not allowed",
                ))
            }
        }
    }
    if total > MAX_TOTAL_BYTES {
        return Err(JavaError::Manifest("too large"));
    }
    if !out.iter().any(|f| f.path.as_str() == JAVA_EXE) {
        return Err(JavaError::Manifest("no bin/java.exe"));
    }
    Ok((dirs, out))
}

/// Directory name of a runtime: `<component>--<version>--windows-x64`.
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
    format!("{component}--{v}--windows-x64")
}

/// A verified runtime of this component that is already installed (newest name first). No network.
pub fn find_installed(root: &PortableRoot, component: &str) -> Option<ManagedJava> {
    let prefix = format!("{component}--");
    let mut ids: Vec<String> = fs::read_dir(root.java_runtimes_dir())
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix))
        .collect();
    ids.sort();
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
    let (dirs, files) = parse_files(&body)?;
    Ok(RuntimePlan {
        release,
        dirs,
        files,
    })
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
    if let Ok(java) = resolve_java(root, &id) {
        return Ok(java);
    }
    if target.exists() {
        fs::remove_dir_all(&target)?; // present but failing its check: reinstall
    }
    let staging = root.cache_dir().join(STAGING).join(&id);
    fs::create_dir_all(&staging)?;
    for d in &plan.dirs {
        fs::create_dir_all(d.to_path(&staging))?;
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

    let java_exe = RelPath::new(JAVA_EXE).map_err(|_| JavaError::Manifest("bad exe path"))?;
    let manifest = RuntimeManifest {
        schema: 1,
        id: id.clone(),
        vendor: "mojang".into(),
        java_major,
        arch: "x64".into(),
        java_exe_sha256: sha256_hex(&java_exe.to_path(&staging))?,
        java_exe,
    };
    atomic_write(
        &staging.join("runtime.json"),
        &serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?,
    )?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&staging, &target)?;
    resolve_java(root, &id).map_err(|_| JavaError::Manifest("installed runtime failed its check"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::{Cancel, Progress};
    use crate::net::testing::FakeFetcher;
    use crate::net::testing_stream::FakeStreamer;
    use serde_json::json;
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
        let files: Vec<(&'static str, Vec<u8>)> = vec![
            ("bin/java.exe", vec![1u8; 300]),
            ("bin/javaw.exe", vec![2u8; 200]),
            ("lib/modules", vec![3u8; 5000]),
        ];
        let fetcher = FakeFetcher::new();
        let streamer = FakeStreamer::new();
        let mut entries = serde_json::Map::new();
        entries.insert("bin".into(), json!({"type": "directory"}));
        entries.insert("lib".into(), json!({"type": "directory"}));
        for (path, data) in &files {
            let url = format!("{BASE}/{}/{path}", sha1_of(data));
            streamer.add(&url, data);
            entries.insert(
                (*path).into(),
                json!({"type": "file", "executable": false, "downloads": {"raw": {"sha1": sha1_of(data), "size": data.len(), "url": url}}}),
            );
        }
        let manifest = json!({"files": entries}).to_string();
        let manifest_url = "https://piston-meta.mojang.com/v1/packages/m/manifest.json";
        fetcher.ok(manifest_url, &manifest);
        let index = json!({"windows-x64": {"java-runtime-delta": [{
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
        assert_eq!(java.id, "java-runtime-delta--21.0.7--windows-x64");
        assert_eq!(java.java_major, 21);
        assert!(java.exe.ends_with("bin/java.exe") || java.exe.ends_with("bin\\java.exe"));
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
        assert_eq!(fs::read(&fixed.exe).unwrap(), w.files[0].1);
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
        let staged = w
            .root
            .cache_dir()
            .join(STAGING)
            .join(&id)
            .join("bin")
            .join("java.exe");
        assert!(staged.is_file(), "verified files survive for the retry");
        w.streamer.clear_status(&modules_url);
        let before = w
            .streamer
            .count(&format!("{BASE}/{}/bin/java.exe", sha1_of(&w.files[0].1)));
        install_runtime(&w.root, &plan, 21, &downloader(&w), 1)
            .await
            .unwrap();
        let after = w
            .streamer
            .count(&format!("{BASE}/{}/bin/java.exe", sha1_of(&w.files[0].1)));
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
        let make = |name: &str, e: Value| {
            json!({"files": {"bin/java.exe": ok.clone(), name: e}}).to_string()
        };
        assert!(parse_files(make("fine.txt", ok.clone()).as_bytes()).is_ok());
        assert!(parse_files(make("../escape.dll", ok.clone()).as_bytes()).is_err());
        assert!(parse_files(make("C:/x.dll", ok.clone()).as_bytes()).is_err());
        assert!(
            parse_files(make("link", json!({"type":"link","target":"/etc"})).as_bytes()).is_err()
        );
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
            "must contain java.exe"
        );
    }

    #[test]
    fn runtime_ids_are_safe_single_directory_names() {
        let id = runtime_id("jre-legacy", "8u51-cacert462b08");
        assert_eq!(id, "jre-legacy--8u51-cacert462b08--windows-x64");
        assert!(runtime_id("x", "a/b\\c d")
            .chars()
            .all(|c| c != '/' && c != '\\' && c != ' '));
        let d = tempfile::tempdir().unwrap();
        assert!(runtime_dir(&PortableRoot::at(d.path()), &id).is_ok());
    }
}
