//! Preparing an instance: turning a Pending profile into a Ready one.
//!
//! Everything a launch will need is installed and verified, driven only by Mojang's own metadata:
//! the version JSON (found through the official catalog), the client jar, every library that
//! applies to Windows x64, the native libraries, the asset index and all assets, the logging
//! configuration, and the Java runtime the version asks for. Modded profiles then add their
//! loader on top (see `loader_prepare`).
//!
//! Where things go (all inside the portable folder, nothing machine-specific):
//! - shared, immutable, hash-verified: `minecraft/versions/<id>/`, `libraries/`, `assets/`,
//!   `runtimes/java/`;
//! - instance-only: `instances/<id>/natives/` (rebuilt each time) and, for the pre-1.6 asset
//!   layout only, `instances/<id>/game/resources/`. Nothing else is written per instance.
//!
//! Guarantees:
//! - An instance becomes Ready only after every required file exists and has passed
//!   verification, and the final file check has run. Until then it stays Pending.
//! - Anything verified stays: an interrupted or failed run keeps verified files and `.part`
//!   files, and the next run reuses them (no download for what is already good).
//! - A Ready instance needs no network: [`check_files`] only looks at the local files.
//! - Nothing is trusted from the network without a hash: files are verified against metadata
//!   hashes, and the few files without one use the `.sha1` published beside them.
//! - Progress is structured ([`PrepareProgress`]); percentages exist only where totals are known.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sha1::{Digest, Sha1};
use url::Url;

use crate::download::{Cancel, DownloadError, Downloader, FileJob, Progress};
use crate::instances::{
    Instance, InstanceError, InstanceId, InstanceStore, JavaChoice, Profile, SetupState,
};
use crate::java_install::{self, JavaError, RuntimePlan};
use crate::loader_install::SystemEnv;
use crate::loader_prepare::{self, InstallerRunner};
use crate::loaders::LoaderKind;
use crate::mc_catalog::{self, CatalogError, Origin};
use crate::mc_meta::{
    parse_asset_index, parse_version, AssetIndex, Library, MetaError, VersionPlan, WINDOWS_X64,
};
use crate::natives::extract_natives;
use crate::net::{FetchError, Fetcher, Streamer};
use crate::portable::fsutil::{self, atomic_write, Expected, VerifyError};
use crate::portable::{PortableRoot, RelPath};
use crate::runtimes::{resolve_java, ManagedJava};
use crate::versions::{LoaderVersion, McVersion, VersionId};

const MAX_VERSION_JSON: usize = 4 * 1024 * 1024;
const MAX_INDEX_JSON: usize = 16 * 1024 * 1024;

// ------------------------------------------------------------------------------ errors

#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    #[error("Setup was cancelled. Files that were already downloaded are kept.")]
    Cancelled,
    #[error("Can't reach the network, and {0} isn't saved on this computer yet.")]
    Offline(String),
    #[error("A download failed: {0}")]
    Network(String),
    #[error("A downloaded file ({0}) failed verification, so it was discarded. Try again.")]
    Verification(String),
    #[error("Mojang's information for this version isn't available: {0}")]
    Metadata(String),
    #[error("This version can't be prepared: {0}")]
    Unsupported(String),
    #[error("Java couldn't be set up: {0}")]
    Java(String),
    #[error("{0}")]
    Loader(String),
    #[error("Files couldn't be saved: {0}")]
    Storage(String),
    #[error("There isn't enough free space on the drive that holds Clade: about {} is needed and {} is available. Free some space and try again.", crate::system::human(*.needed), crate::system::human(*.available))]
    DiskSpace { needed: u64, available: u64 },
    #[error("Sign in with a Minecraft account before downloading or verifying game files.")]
    AccountRequired,
    #[error("That profile no longer exists.")]
    NotFound,
    #[error("Another profile is being set up. Wait for it to finish.")]
    Busy,
    #[error(
        "This profile is starting or running. Close the game before setting it up or verifying it."
    )]
    GameRunning,
}

impl PrepareError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Offline(_) => "offline",
            Self::Network(_) => "network",
            Self::Verification(_) => "verification",
            Self::Metadata(_) => "metadata",
            Self::Unsupported(_) => "unsupported",
            Self::Java(_) => "java",
            Self::Loader(_) => "loader",
            Self::Storage(_) => "storage",
            Self::DiskSpace { .. } => "disk_space",
            Self::AccountRequired => "account_required",
            Self::NotFound => "not_found",
            Self::Busy => "busy",
            Self::GameRunning => "already_running",
        }
    }
}

impl From<DownloadError> for PrepareError {
    fn from(e: DownloadError) -> Self {
        match e {
            DownloadError::Cancelled => Self::Cancelled,
            DownloadError::Offline(_) => Self::Offline("some of the files it needs".into()),
            DownloadError::Status(c) => {
                Self::Network(format!("the server answered with status {c}"))
            }
            DownloadError::Verification(f) => Self::Verification(f),
            DownloadError::Policy(m) => {
                Self::Unsupported(format!("blocked by the network policy ({m})"))
            }
            DownloadError::Local(m) => Self::Storage(m),
        }
    }
}

impl From<MetaError> for PrepareError {
    fn from(e: MetaError) -> Self {
        Self::Unsupported(e.to_string())
    }
}

impl From<JavaError> for PrepareError {
    fn from(e: JavaError) -> Self {
        match e {
            JavaError::Download(d) => d.into(),
            JavaError::Fetch(f) => fetch_err(f, "Mojang's Java runtime"),
            other => Self::Java(other.to_string()),
        }
    }
}

impl From<InstanceError> for PrepareError {
    fn from(e: InstanceError) -> Self {
        match e {
            InstanceError::NotFound(_) => Self::NotFound,
            other => Self::Storage(other.to_string()),
        }
    }
}

pub fn fetch_err(e: FetchError, what: &str) -> PrepareError {
    match e {
        FetchError::Network(_) | FetchError::TooLarge(_) => PrepareError::Offline(what.to_owned()),
        FetchError::Status(c) => PrepareError::Network(format!("{what} answered with status {c}")),
        FetchError::Policy(p) => {
            PrepareError::Unsupported(format!("blocked by the network policy ({p})"))
        }
        FetchError::Local(m) => PrepareError::Storage(m),
    }
}

fn storage(e: impl std::fmt::Display) -> PrepareError {
    PrepareError::Storage(e.to_string())
}

// ------------------------------------------------------------------------------ progress

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Planning,
    Java,
    Minecraft,
    Libraries,
    Assets,
    Natives,
    Loader,
    Verifying,
    Done,
}

/// What the UI sees. Counts are `None` unless the total is known; `overall` is a fraction of the
/// bytes planned for the whole preparation and is `None` during steps that cannot be measured
/// (running a loader installer), so no percentage is ever invented.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareProgress {
    pub instance_id: String,
    pub stage: Stage,
    pub label: String,
    pub items_done: Option<u64>,
    pub items_total: Option<u64>,
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
    pub overall: Option<f64>,
}

struct StageInfo {
    stage: Stage,
    label: String,
    indeterminate: bool,
}

pub struct Tracker {
    instance: String,
    pub progress: Arc<Progress>,
    current: Mutex<StageInfo>,
    sink: Box<dyn Fn(PrepareProgress) + Send + Sync>,
}

impl Tracker {
    pub fn new(
        instance: &InstanceId,
        progress: Arc<Progress>,
        sink: Box<dyn Fn(PrepareProgress) + Send + Sync>,
    ) -> Self {
        Self {
            instance: instance.to_string(),
            progress,
            current: Mutex::new(StageInfo {
                stage: Stage::Planning,
                label: "Preparing Minecraft".into(),
                indeterminate: true,
            }),
            sink,
        }
    }

    /// Starts a stage. `items` is the number of files it will handle, when known.
    pub fn enter(
        &self,
        stage: Stage,
        label: impl Into<String>,
        items: Option<u64>,
        indeterminate: bool,
    ) {
        self.progress.begin_items(items.unwrap_or(0));
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = StageInfo {
            stage,
            label: label.into(),
            indeterminate,
        };
        self.emit();
    }

    pub fn snapshot(&self) -> PrepareProgress {
        let cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let s = self.progress.snapshot();
        let measured = !cur.indeterminate && s.bytes_total > 0;
        PrepareProgress {
            instance_id: self.instance.clone(),
            stage: cur.stage,
            label: cur.label.clone(),
            items_done: (s.items_total > 0).then_some(s.items_done.min(s.items_total)),
            items_total: (s.items_total > 0).then_some(s.items_total),
            bytes_done: measured.then_some(s.bytes_done.min(s.bytes_total)),
            bytes_total: measured.then_some(s.bytes_total),
            overall: measured
                .then(|| (s.bytes_done.min(s.bytes_total) as f64) / (s.bytes_total as f64)),
        }
    }

    pub fn emit(&self) {
        (self.sink)(self.snapshot());
    }
}

// ------------------------------------------------------------------------------ context

/// Reports the free bytes on the drive holding a path.
pub type FreeSpace = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

pub struct Ctx<'a, F: Fetcher, S: Streamer, R: InstallerRunner> {
    pub root: &'a PortableRoot,
    /// Mojang and the global allow-list (documents).
    pub meta: &'a F,
    /// Loader hosts only (catalogs, profiles, installer checksums).
    pub loader_meta: &'a F,
    /// Large files from the global allow-list.
    pub streamer: &'a S,
    /// Large files from loader hosts only (installer jars).
    pub loader_streamer: &'a S,
    pub runner: &'a R,
    pub sys: SystemEnv,
    pub concurrency: usize,
    pub now: i64,
    pub backoff: Duration,
    pub cancel: Arc<Cancel>,
    pub tracker: &'a Tracker,
    /// Re-hash every installed file even when the instance is already Ready (a "verify files" run).
    pub verify_all: bool,
    /// Free bytes on the drive holding a path (`None` when unknown, which skips the space check).
    pub free_space: FreeSpace,
}

impl<F: Fetcher, S: Streamer, R: InstallerRunner> Ctx<'_, F, S, R> {
    fn downloader(&self) -> Downloader<'_, S> {
        Downloader::new(
            self.streamer,
            self.cancel.clone(),
            self.tracker.progress.clone(),
        )
        .with_backoff(self.backoff)
        .with_root(self.root.path())
    }
    /// The stage label: a verification run says "Verifying", a normal one says what it downloads.
    fn label(&self, normal: &str, verifying: &str) -> String {
        if self.verify_all { verifying } else { normal }.to_owned()
    }
    fn check_cancel(&self) -> Result<(), PrepareError> {
        if self.cancel.is_cancelled() {
            Err(PrepareError::Cancelled)
        } else {
            Ok(())
        }
    }
}

// ------------------------------------------------------------------------------ paths

fn versions_dir(root: &PortableRoot) -> PathBuf {
    root.minecraft_dir().join("versions")
}

fn version_json_path(root: &PortableRoot, id: &str) -> PathBuf {
    versions_dir(root).join(id).join(format!("{id}.json"))
}

fn client_jar_path(root: &PortableRoot, id: &str) -> PathBuf {
    versions_dir(root).join(id).join(format!("{id}.jar"))
}

fn asset_index_path(root: &PortableRoot, id: &str) -> PathBuf {
    root.assets_dir().join("indexes").join(format!("{id}.json"))
}

fn log_config_path(root: &PortableRoot, file: &str) -> PathBuf {
    root.assets_dir().join("log_configs").join(file)
}

fn object_path(root: &PortableRoot, store_path: &str) -> PathBuf {
    root.assets_dir().join("objects").join(store_path)
}

fn hex_token(body: &[u8], len: usize) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let t = text.split_whitespace().next()?;
    (t.len() == len && t.bytes().all(|b| b.is_ascii_hexdigit())).then(|| t.to_ascii_lowercase())
}

/// The `.sha1` the Maven publishes beside a library, for the few libraries with no hash in
/// their metadata. (Same host as the file: it catches corruption, not a compromised host.)
async fn sidecar_sha1<F: Fetcher>(meta: &F, url: &Url) -> Result<Expected, PrepareError> {
    let mut u = url.clone();
    u.set_path(&format!("{}.sha1", url.path()));
    let body = meta
        .get(&u, 256)
        .await
        .map_err(|e| fetch_err(e, "a library's checksum"))?;
    hex_token(&body, 40)
        .map(Expected::Sha1)
        .ok_or_else(|| PrepareError::Unsupported("a library has no verifiable checksum".into()))
}

/// A library file that an installer generates locally (`url: ""`), verified against any hash/size
/// published in the loader's version profile.
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    pub dest: PathBuf,
    pub expected: Option<Expected>,
    pub size: Option<u64>,
}

/// Verifies a locally generated installer artifact (`url: ""`), rejecting links and checking its
/// declared hash/size (or non-empty presence when neither is declared).
pub fn verify_generated_file(g: &GeneratedFile, root: &PortableRoot) -> Result<(), PrepareError> {
    fsutil::reject_links_between(root.path(), &g.dest).map_err(storage)?;
    if !g.dest.is_file() || fsutil::is_link(&g.dest).unwrap_or(true) {
        return Err(PrepareError::Loader(
            "the loader installer did not produce a file the profile needs".into(),
        ));
    }
    let name = g
        .dest
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if let Some(expected) = &g.expected {
        match fsutil::verify_file(&g.dest, expected, g.size) {
            Ok(()) => Ok(()),
            Err(VerifyError::Hash | VerifyError::Size { .. }) => {
                Err(PrepareError::Verification(name))
            }
            Err(VerifyError::Io(e)) => Err(PrepareError::Storage(e.to_string())),
        }
    } else if let Some(want) = g.size {
        let len = fs::metadata(&g.dest).map_err(storage)?.len();
        if len != want {
            return Err(PrepareError::Verification(name));
        }
        Ok(())
    } else {
        let len = fs::metadata(&g.dest).map_err(storage)?.len();
        if len == 0 {
            return Err(PrepareError::Loader(
                "the loader installer did not produce a file the profile needs".into(),
            ));
        }
        Ok(())
    }
}

enum PlannedLib {
    Ready(FileJob),
    NeedsSidecar {
        url: Url,
        dest: PathBuf,
        size: Option<u64>,
    },
}

/// Download jobs for libraries (deduplicated), plus the files that only an installer can produce.
pub async fn jobs_for_libraries<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    libs: &[&Library],
    recorded: &std::collections::HashMap<String, u64>,
) -> Result<(Vec<FileJob>, Vec<GeneratedFile>), PrepareError> {
    let base = ctx.root.libraries_dir();
    let libs_owned: Vec<Library> = libs.iter().map(|l| (*l).clone()).collect();
    let recorded_owned = recorded.clone();
    let verify_all = ctx.verify_all;

    let (planned, generated) = tokio::task::spawn_blocking(move || {
        let mut seen = HashSet::new();
        let (mut planned, mut generated) = (Vec::new(), Vec::new());
        for lib in libs_owned {
            let dest = lib.artifact.path.to_path(&base);
            if !seen.insert(dest.clone()) {
                continue;
            }
            let Some(url) = lib.artifact.url else {
                generated.push(GeneratedFile {
                    dest,
                    expected: lib.artifact.expected,
                    size: lib.artifact.size,
                });
                continue;
            };
            match lib.artifact.expected {
                Some(expected) => planned.push(PlannedLib::Ready(FileJob {
                    url,
                    dest,
                    expected,
                    size: lib.artifact.size,
                })),
                // Nothing in the metadata to verify an existing copy against: it came through a
                // sidecar-verified download, whose size was recorded then. A missing record, zero
                // size, different size, or explicit full verification fetches the `.sha1` sidecar.
                None if !verify_all
                    && dest.is_file()
                    && recorded_owned
                        .get(lib.artifact.path.as_str())
                        .is_some_and(|&n| {
                            n > 0 && fs::metadata(&dest).map(|m| m.len() == n).unwrap_or(false)
                        }) =>
                {
                    continue;
                }
                None => planned.push(PlannedLib::NeedsSidecar {
                    url,
                    dest,
                    size: lib.artifact.size,
                }),
            }
        }
        (planned, generated)
    })
    .await
    .map_err(storage)?;

    let mut jobs = Vec::with_capacity(planned.len());
    for item in planned {
        match item {
            PlannedLib::Ready(job) => jobs.push(job),
            PlannedLib::NeedsSidecar { url, dest, size } => {
                let expected = sidecar_sha1(ctx.meta, &url).await?;
                jobs.push(FileJob {
                    url,
                    dest,
                    expected,
                    size,
                });
            }
        }
    }
    Ok((jobs, generated))
}

// ------------------------------------------------------------------------------ offline check

fn check_natives_present(root: &PortableRoot, id: &InstanceId) -> Result<(), String> {
    let natives = InstanceStore::new(root).dirs(id).natives_dir();
    if fs::read_dir(&natives)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true)
    {
        return Err("the extracted native libraries".into());
    }
    Ok(())
}

/// Every file a launch of `inst` needs, checked by looking at the local disk only (existence and
/// size, no hashing, no network). `loader_id` is the loader profile for Modded instances.
/// `Err` describes the first thing that is missing.
pub fn check_files(
    root: &PortableRoot,
    inst: &Instance,
    loader_id: Option<&VersionId>,
) -> Result<(), String> {
    let mc = inst.minecraft.as_str();
    let bytes = fsutil::read_bounded(&version_json_path(root, mc), MAX_VERSION_JSON as u64)
        .map_err(|_| "the Minecraft version information".to_owned())?;
    let plan = parse_version(&bytes, &WINDOWS_X64).map_err(|e| e.to_string())?;

    let client = plan.client.as_ref().ok_or("the client download")?;
    let jar = client_jar_path(root, mc);
    if fs::metadata(&jar).map(|m| m.len()).ok() != Some(client.size) {
        return Err("the Minecraft game file".into());
    }
    check_libraries(root, &plan, &Default::default())?;
    if let Some(log) = &plan.logging {
        if !log_config_path(root, &log.file_name).is_file() {
            return Err("the logging configuration".into());
        }
    }
    if let Some(ai) = &plan.asset_index {
        let idx = fsutil::read_bounded(&asset_index_path(root, &ai.id), MAX_INDEX_JSON as u64)
            .map_err(|_| "the asset index".to_owned())?;
        let index = parse_asset_index(&idx).map_err(|e| e.to_string())?;
        for o in &index.objects {
            if fs::metadata(object_path(root, &o.store_path()))
                .map(|m| m.len())
                .ok()
                != Some(o.size)
            {
                return Err("a game asset".into());
            }
        }
        if index.virtual_files || index.map_to_resources {
            let virtual_dir = root.assets_dir().join("virtual").join(&ai.assets_name);
            let resources_dir = InstanceStore::new(root)
                .dirs(&inst.id)
                .game_dir()
                .join("resources");
            for o in &index.objects {
                let name = RelPath::new(&o.name).map_err(|_| "a game asset".to_owned())?;
                if index.virtual_files
                    && fs::metadata(name.to_path(&virtual_dir))
                        .map(|m| m.len())
                        .ok()
                        != Some(o.size)
                {
                    return Err("a game asset".into());
                }
                if index.map_to_resources
                    && fs::metadata(name.to_path(&resources_dir))
                        .map(|m| m.len())
                        .ok()
                        != Some(o.size)
                {
                    return Err("a game asset".into());
                }
            }
        }
    }
    let vanilla_has_natives = plan.natives().next().is_some();
    if vanilla_has_natives {
        check_natives_present(root, &inst.id)?;
    }
    match &inst.java {
        JavaChoice::Auto => {
            if java_install::find_installed(root, &plan.java.component).is_none() {
                return Err("the Java runtime".into());
            }
        }
        JavaChoice::Runtime { id } => {
            resolve_java(root, id).map_err(|_| "the chosen Java runtime".to_owned())?;
        }
    }
    if let Profile::Modded { .. } = inst.profile {
        let id = loader_id.ok_or("the mod loader")?;
        let profile = loader_prepare::read_profile(root, id)
            .map_err(|_| "the mod loader profile".to_owned())?;
        check_libraries(
            root,
            &profile,
            &loader_prepare::read_sizes(root, id.as_str()),
        )?;
        if !vanilla_has_natives && profile.natives().next().is_some() {
            check_natives_present(root, &inst.id)?;
        }
    }
    Ok(())
}

fn check_libraries(
    root: &PortableRoot,
    plan: &VersionPlan,
    recorded: &std::collections::HashMap<String, u64>,
) -> Result<(), String> {
    for lib in &plan.libraries {
        let p = lib.artifact.path.to_path(&root.libraries_dir());
        let md = fs::metadata(&p).map_err(|_| format!("the library {}", lib.name))?;
        if !md.is_file() {
            return Err(format!("the library {}", lib.name));
        }
        let len = md.len();
        if let Some(want) = lib.artifact.size {
            if len != want {
                return Err(format!("the library {}", lib.name));
            }
        } else if lib.artifact.url.is_some() && lib.artifact.expected.is_none() {
            if !recorded
                .get(lib.artifact.path.as_str())
                .is_some_and(|&s| s > 0 && s == len)
            {
                return Err(format!("the library {}", lib.name));
            }
        } else if let Some(&s) = recorded.get(lib.artifact.path.as_str()) {
            if s == 0 || s != len {
                return Err(format!("the library {}", lib.name));
            }
        } else if len == 0 {
            return Err(format!("the library {}", lib.name));
        }
    }
    Ok(())
}

/// [`check_files`] for an instance that is marked Ready.
pub fn check_installed(root: &PortableRoot, inst: &Instance) -> Result<(), String> {
    match &inst.setup {
        SetupState::Ready { version_id, .. } => {
            let loader = (!inst.is_vanilla()).then_some(version_id);
            check_files(root, inst, loader)
        }
        SetupState::Pending => Err("setup has not run yet".into()),
    }
}

// ------------------------------------------------------------------------------ the steps

async fn ensure_version_json<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    mc: &McVersion,
) -> Result<Vec<u8>, PrepareError> {
    let path = version_json_path(ctx.root, mc.as_str());
    let local_valid = {
        let local_path = path.clone();
        let expected_id = mc.as_str().to_owned();
        tokio::task::spawn_blocking(move || {
            let b = fsutil::read_bounded(&local_path, MAX_VERSION_JSON as u64).ok()?;
            let plan = parse_version(&b, &WINDOWS_X64).ok()?;
            (plan.id == expected_id).then_some(b)
        })
        .await
        .map_err(storage)?
    };
    if !ctx.verify_all {
        if let Some(b) = local_valid {
            return Ok(b); // saved earlier, after it was verified: no network needed
        }
    }
    let cache = ctx
        .root
        .cache_dir()
        .join("catalogs")
        .join("version_manifest.json");
    let mut catalog = mc_catalog::load(ctx.meta, &cache, ctx.now, ctx.verify_all)
        .await
        .map_err(|e| match e {
            CatalogError::Unavailable(_) => PrepareError::Offline("Mojang's version list".into()),
            other => PrepareError::Metadata(other.to_string()),
        })?;
    let find = |c: &mc_catalog::VersionCatalog| c.entries.iter().find(|e| &e.id == mc).cloned();
    let mut entry = find(&catalog.catalog);
    if entry
        .as_ref()
        .is_some_and(|e| e.url.is_none() || e.sha1.is_none())
        && catalog.origin != Origin::Network
    {
        // A list saved before preparation existed lacks the metadata links: refresh it once.
        if let Ok(fresh) = mc_catalog::load(ctx.meta, &cache, ctx.now, true).await {
            catalog = fresh;
            entry = find(&catalog.catalog);
        }
    }
    let entry = entry.ok_or_else(|| {
        PrepareError::Metadata(format!("Minecraft {mc} isn't in Mojang's version list"))
    })?;
    let (Some(url), Some(sha1)) = (entry.url, entry.sha1) else {
        return Err(PrepareError::Metadata(
            "the version list has no link for this version".into(),
        ));
    };
    if let Some(local_bytes) = local_valid {
        if hex::encode(Sha1::digest(&local_bytes)).eq_ignore_ascii_case(&sha1) {
            return Ok(local_bytes);
        }
    }
    let url =
        Url::parse(&url).map_err(|_| PrepareError::Metadata("a malformed metadata link".into()))?;
    let body = ctx
        .meta
        .get(&url, MAX_VERSION_JSON)
        .await
        .map_err(|e| fetch_err(e, &format!("the Minecraft {mc} information")))?;
    if !hex::encode(Sha1::digest(&body)).eq_ignore_ascii_case(&sha1) {
        return Err(PrepareError::Verification("version information".into()));
    }
    let parsed = parse_version(&body, &WINDOWS_X64)?;
    if parsed.id != mc.as_str() {
        return Err(PrepareError::Metadata(
            "the version metadata is for a different Minecraft version".into(),
        ));
    }
    let write_body = body.clone();
    tokio::task::spawn_blocking(move || atomic_write(&path, &write_body))
        .await
        .map_err(storage)?
        .map_err(storage)?;
    Ok(body)
}

async fn ensure_asset_index<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    dl: &Downloader<'_, S>,
    plan: &VersionPlan,
) -> Result<Option<(String, AssetIndex)>, PrepareError> {
    let Some(r) = &plan.asset_index else {
        return Ok(None);
    };
    ctx.tracker.progress.plan_bytes(r.download.size);
    let dest = asset_index_path(ctx.root, &r.id);
    dl.fetch(&FileJob {
        url: r.download.url.clone(),
        dest: dest.clone(),
        expected: Expected::Sha1(r.download.sha1.clone()),
        size: Some(r.download.size),
    })
    .await?;
    let assets_name = r.assets_name.clone();
    let index = tokio::task::spawn_blocking(move || -> Result<AssetIndex, PrepareError> {
        let bytes = fsutil::read_bounded(&dest, MAX_INDEX_JSON as u64).map_err(|e| {
            if e.kind() == io::ErrorKind::InvalidData {
                PrepareError::Unsupported("the asset index is unexpectedly large".into())
            } else {
                storage(e)
            }
        })?;
        Ok(parse_asset_index(&bytes)?)
    })
    .await
    .map_err(storage)??;
    Ok(Some((assets_name, index)))
}

async fn ensure_java<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    dl: &Downloader<'_, S>,
    inst: &Instance,
    plan: &VersionPlan,
    runtime_plan: Option<&RuntimePlan>,
) -> Result<ManagedJava, PrepareError> {
    if let JavaChoice::Runtime { id } = &inst.java {
        let root = ctx.root.clone();
        let id_owned = id.clone();
        return tokio::task::spawn_blocking(move || resolve_java(&root, &id_owned))
            .await
            .map_err(storage)?
            .map_err(|_| {
                PrepareError::Java(
                    "the Java runtime chosen for this profile isn't installed".into(),
                )
            });
    }
    let existing = {
        let root = ctx.root.clone();
        let comp = plan.java.component.clone();
        tokio::task::spawn_blocking(move || java_install::find_installed(&root, &comp))
            .await
            .map_err(storage)?
    };
    if let Some(java) = existing {
        return Ok(java);
    }
    let rp = runtime_plan.ok_or_else(|| PrepareError::Java("no Java runtime plan".into()))?;
    ctx.tracker.enter(
        Stage::Java,
        "Preparing Java",
        Some(rp.files.len() as u64),
        false,
    );
    Ok(java_install::install_runtime(ctx.root, rp, plan.java.major, dl, ctx.concurrency).await?)
}

fn copy_named(
    src: &Path,
    dst: &Path,
    size: u64,
    expected: Option<&Expected>,
    checked_parents: &mut HashSet<PathBuf>,
    root_base: &Path,
) -> io::Result<()> {
    let parent = dst
        .parent()
        .ok_or_else(|| io::Error::other("asset destination has no parent directory"))?;
    if checked_parents.insert(parent.to_path_buf()) {
        fsutil::reject_links_between(root_base, parent)?;
        fs::create_dir_all(parent)?;
        fsutil::reject_links_between(root_base, parent)?;
    }
    match fsutil::is_link(dst) {
        Ok(true) => {
            return Err(io::Error::other(format!(
                "refusing to write through link at {}",
                dst.display()
            )))
        }
        Ok(false) => {
            if let Some(exp) = expected {
                if fsutil::verify_file(dst, exp, Some(size)).is_ok() {
                    return Ok(());
                }
            } else if fs::metadata(dst)
                .map(|m| m.is_file() && m.len() == size)
                .unwrap_or(false)
            {
                return Ok(());
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let file_name = dst
        .file_name()
        .ok_or_else(|| io::Error::other("asset destination has no file name"))?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".copy.tmp");
    let tmp = dst.with_file_name(tmp_name);
    if fsutil::is_link(&tmp).unwrap_or(false) {
        return Err(io::Error::other(format!(
            "refusing to write through link at {}",
            tmp.display()
        )));
    }
    let res = (|| -> io::Result<()> {
        fs::copy(src, &tmp)?;
        fsutil::rename_with_retry(&tmp, dst)
    })();
    if res.is_err() {
        let _ = fsutil::remove_file_with_retry(&tmp);
    }
    res
}

async fn ensure_assets<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    dl: &Downloader<'_, S>,
    index: &AssetIndex,
    assets_name: &str,
    game_dir: &Path,
) -> Result<(), PrepareError> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for o in &index.objects {
        if seen.insert(o.hash.clone()) {
            jobs.push(FileJob {
                url: o.url()?,
                dest: object_path(ctx.root, &o.store_path()),
                expected: Expected::Sha1(o.hash.clone()),
                size: Some(o.size),
            });
        }
    }
    ctx.tracker.enter(
        Stage::Assets,
        ctx.label("Downloading assets", "Verifying assets"),
        Some(jobs.len() as u64),
        false,
    );
    dl.fetch_all(jobs, ctx.concurrency).await?;

    // Old versions read assets as plain, named files instead of by hash.
    if index.virtual_files || index.map_to_resources {
        let virtual_dir = ctx.root.assets_dir().join("virtual").join(assets_name);
        let resources = game_dir.join("resources");
        let (objects, wants_virtual, wants_resources, verify_all, cancel) = (
            index.objects.clone(),
            index.virtual_files,
            index.map_to_resources,
            ctx.verify_all,
            ctx.cancel.clone(),
        );
        let root = ctx.root.clone();
        tokio::task::spawn_blocking(move || -> Result<(), PrepareError> {
            let mut checked_parents = HashSet::new();
            for o in &objects {
                if cancel.is_cancelled() {
                    return Err(PrepareError::Cancelled);
                }
                let name = RelPath::new(&o.name)
                    .map_err(|_| PrepareError::Unsupported("an unsafe asset name".into()))?;
                let src = object_path(&root, &o.store_path());
                let expected = verify_all.then(|| Expected::Sha1(o.hash.clone()));
                if wants_virtual {
                    copy_named(
                        &src,
                        &name.to_path(&virtual_dir),
                        o.size,
                        expected.as_ref(),
                        &mut checked_parents,
                        root.path(),
                    )
                    .map_err(storage)?;
                }
                if wants_resources {
                    copy_named(
                        &src,
                        &name.to_path(&resources),
                        o.size,
                        expected.as_ref(),
                        &mut checked_parents,
                        root.path(),
                    )
                    .map_err(storage)?;
                }
            }
            Ok(())
        })
        .await
        .map_err(storage)??;
    }
    Ok(())
}

/// `group:artifact[:classifier]` without version or `@ext`, matching `launch_args::resolve`.
fn library_key(name: &str) -> String {
    let main = name.split('@').next().unwrap_or(name);
    let mut p = main.split(':');
    let (g, a) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
    let _version = p.next();
    match p.next() {
        Some(c) => format!("{g}:{a}:{c}"),
        None => format!("{g}:{a}"),
    }
}

/// Native libraries merged across `[loader, vanilla]`: the loader's copy of any
/// `group:artifact[:classifier]` replaces vanilla's, and loader natives extract last so any
/// overlapping DLL name also prefers the loader's archive.
fn merged_natives(vanilla: &VersionPlan, loader: Option<&VersionPlan>) -> Vec<Library> {
    let mut loader_keys = HashSet::new();
    let mut loader_natives = Vec::new();
    if let Some(lp) = loader {
        for lib in lp.natives() {
            if loader_keys.insert(library_key(&lib.name)) {
                loader_natives.push(lib.clone());
            }
        }
    }
    let mut seen = loader_keys;
    let mut out: Vec<Library> = vanilla
        .natives()
        .filter(|lib| seen.insert(library_key(&lib.name)))
        .cloned()
        .collect();
    out.extend(loader_natives);
    out
}

fn planned_bytes(
    plan: &VersionPlan,
    index: Option<&AssetIndex>,
    runtime: Option<&RuntimePlan>,
) -> u64 {
    let mut seen = HashSet::new();
    let libs = plan
        .libraries
        .iter()
        .filter(|l| l.artifact.url.is_some() && seen.insert(l.artifact.path.clone()))
        .filter_map(|l| l.artifact.size)
        .fold(0u64, u64::saturating_add);
    let mut hashes = HashSet::new();
    let assets = index.map_or(0u64, |i| {
        i.objects
            .iter()
            .filter(|o| hashes.insert(o.hash.clone()))
            .map(|o| o.size)
            .fold(0u64, u64::saturating_add)
    });
    let runtime_bytes = runtime.map_or(0u64, |rp| {
        rp.files
            .iter()
            .map(|f| f.download.size)
            .fold(0u64, u64::saturating_add)
    });
    plan.client
        .as_ref()
        .map_or(0u64, |c| c.size)
        .saturating_add(plan.logging.as_ref().map_or(0u64, |l| l.download.size))
        .saturating_add(libs)
        .saturating_add(assets)
        .saturating_add(runtime_bytes)
}

fn missing(path: &Path, size: Option<u64>) -> bool {
    match (fs::metadata(path).ok().map(|m| m.len()), size) {
        (None, _) => true,
        (Some(len), Some(want)) => len != want,
        (Some(len), None) => len == 0,
    }
}

fn round_up_cluster(bytes: u64, cluster: u64) -> u64 {
    if bytes == 0 || cluster <= 1 {
        return bytes;
    }
    let rem = bytes % cluster;
    if rem == 0 {
        bytes
    } else {
        bytes.saturating_add(cluster - rem)
    }
}

/// On-disk bytes that still have to be written (rounded per missing file to the volume's
/// allocation unit, which matters on exFAT USB sticks with 128–256 KiB clusters). Files already
/// present with matching size add 0, so confirming an installed profile needs 0 extra bytes.
fn bytes_to_download(
    root: &PortableRoot,
    mc: &str,
    plans: &[&VersionPlan],
    assets: Option<(&str, &AssetIndex)>,
    game_dir: &Path,
    runtimes: &[&RuntimePlan],
) -> u64 {
    let cluster = fsutil::cluster_size_bytes(root.path()).max(1);
    let mut n = 0u64;

    for rp in runtimes {
        let id = java_install::runtime_id(&rp.release.component, &rp.release.version_name);
        let staging = root.cache_dir().join("java-staging").join(id);
        for f in &rp.files {
            if missing(&f.path.to_path(&staging), Some(f.download.size)) {
                n = n.saturating_add(round_up_cluster(f.download.size, cluster));
            }
        }
    }

    let mut seen = HashSet::new();
    for plan in plans {
        if let Some(c) = &plan.client {
            if missing(&client_jar_path(root, mc), Some(c.size)) {
                n = n.saturating_add(round_up_cluster(c.size, cluster));
            }
        }
        if let Some(l) = &plan.logging {
            if missing(&log_config_path(root, &l.file_name), Some(l.download.size)) {
                n = n.saturating_add(round_up_cluster(l.download.size, cluster));
            }
        }
        for lib in &plan.libraries {
            let dest = lib.artifact.path.to_path(&root.libraries_dir());
            if lib.artifact.url.is_some()
                && seen.insert(dest.clone())
                && missing(&dest, lib.artifact.size)
            {
                n = n.saturating_add(round_up_cluster(
                    lib.artifact.size.unwrap_or(cluster),
                    cluster,
                ));
            }
        }
    }

    if let Some((assets_name, index)) = assets {
        let mut hashes = HashSet::new();
        for o in &index.objects {
            if hashes.insert(o.hash.clone())
                && missing(&object_path(root, &o.store_path()), Some(o.size))
            {
                n = n.saturating_add(round_up_cluster(o.size, cluster));
            }
        }
        if index.virtual_files {
            let virtual_dir = root.assets_dir().join("virtual").join(assets_name);
            for o in &index.objects {
                if let Ok(rel) = RelPath::new(&o.name) {
                    if missing(&rel.to_path(&virtual_dir), Some(o.size)) {
                        n = n.saturating_add(round_up_cluster(o.size, cluster));
                    }
                }
            }
        }
        if index.map_to_resources {
            let resources_dir = game_dir.join("resources");
            for o in &index.objects {
                if let Ok(rel) = RelPath::new(&o.name) {
                    if missing(&rel.to_path(&resources_dir), Some(o.size)) {
                        n = n.saturating_add(round_up_cluster(o.size, cluster));
                    }
                }
            }
        }
    }
    n
}

enum PlannedLoader {
    Profile {
        kind: LoaderKind,
        version: LoaderVersion,
        profile: loader_prepare::ProfilePlan,
    },
    ReusedInstaller {
        kind: LoaderKind,
        version: LoaderVersion,
        id: VersionId,
        plan: VersionPlan,
    },
    RunInstaller {
        kind: LoaderKind,
        version: LoaderVersion,
    },
}

impl PlannedLoader {
    fn preplanned(&self) -> Option<&VersionPlan> {
        match self {
            Self::Profile { profile, .. } => Some(&profile.plan),
            Self::ReusedInstaller { plan, .. } => Some(plan),
            Self::RunInstaller { .. } => None,
        }
    }
}

// ------------------------------------------------------------------------------ the whole thing

/// Prepares `id`. Safe to call again at any time: it reuses everything already verified.
pub async fn prepare_instance<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    id: &InstanceId,
) -> Result<Instance, PrepareError> {
    let root = ctx.root.clone();
    let id_owned = id.clone();
    let verify_all = ctx.verify_all;
    let (inst, already_ready) =
        tokio::task::spawn_blocking(move || -> Result<(Instance, bool), PrepareError> {
            let store = InstanceStore::new(&root);
            let inst = store.get(&id_owned)?;
            let ready = inst.is_ready() && !verify_all && check_installed(&root, &inst).is_ok();
            Ok((inst, ready))
        })
        .await
        .map_err(storage)??;

    ctx.tracker
        .enter(Stage::Planning, "Preparing Minecraft", None, true);

    if already_ready {
        ctx.tracker
            .enter(Stage::Done, "Ready to play.", None, false);
        return Ok(inst); // already prepared: nothing to do, and no network used
    }
    let result = run(ctx, inst).await;
    if result.is_ok() {
        ctx.tracker
            .enter(Stage::Done, "Ready to play.", None, false);
    }
    result
}

async fn run<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    inst: Instance,
) -> Result<Instance, PrepareError> {
    let mc = inst.minecraft.clone();
    let dirs = InstanceStore::new(ctx.root).dirs(&inst.id);
    let dl = ctx.downloader();

    // ---- 1. read the metadata (everything is planned before anything big is downloaded)
    let vjson = ensure_version_json(ctx, &mc).await?;
    let plan = parse_version(&vjson, &WINDOWS_X64)?;
    let client = plan.client.clone().ok_or_else(|| {
        PrepareError::Unsupported("Mojang publishes no client download for it".into())
    })?;
    let assets = ensure_asset_index(ctx, &dl, &plan).await?;
    ctx.check_cancel()?;

    let (installed_game_java, game_java_component) = {
        let root = ctx.root.clone();
        let choice = inst.java.clone();
        let comp = plan.java.component.clone();
        tokio::task::spawn_blocking(move || match choice {
            JavaChoice::Auto => (java_install::find_installed(&root, &comp), Some(comp)),
            JavaChoice::Runtime { id } => (resolve_java(&root, &id).ok(), None),
        })
        .await
        .map_err(storage)?
    };

    let runtime_plan = match &inst.java {
        JavaChoice::Auto if installed_game_java.is_none() => Some(
            java_install::plan_runtime(ctx.meta, &plan.java.component)
                .await
                .map_err(PrepareError::from)?,
        ),
        _ => None,
    };

    // A full verification also covers the managed Java runtime that is already installed: every file
    // the runtime's manifest lists is checked and the damaged or missing ones are fetched again.
    // (A runtime that is about to be installed fresh needs no check, and the everyday launch only
    // re-hashes java.exe.) Without an authoritative list the check stays at java.exe; nothing is
    // invented for files no manifest covers.
    let java_verify: Option<(ManagedJava, Vec<java_install::RuntimeFile>)> =
        if ctx.verify_all && runtime_plan.is_none() {
            match installed_game_java.clone() {
                Some(java) => java_install::verification_files(
                    ctx.root,
                    &java,
                    game_java_component.as_deref(),
                    ctx.meta,
                )
                .await
                .map(|files| (java, files)),
                None => None,
            }
        } else {
            None
        };
    if ctx.verify_all && runtime_plan.is_none() && java_verify.is_none() {
        tracing::info!(
            "no authoritative file list for the Java runtime; only java.exe was checked"
        );
    }

    let current_ready_id = match &inst.setup {
        SetupState::Ready { version_id, .. } => Some(version_id.clone()),
        SetupState::Pending => None,
    };
    let loader = match &inst.profile {
        Profile::Vanilla => None,
        Profile::Modded {
            loader: kind,
            loader_version,
        } => {
            let lv =
                loader_prepare::resolve_loader_version(ctx, *kind, &mc, loader_version).await?;
            match kind {
                LoaderKind::Fabric | LoaderKind::Quilt => {
                    let profile = loader_prepare::plan_profile_loader(ctx, *kind, &mc, &lv).await?;
                    Some(PlannedLoader::Profile {
                        kind: *kind,
                        version: lv,
                        profile,
                    })
                }
                LoaderKind::Forge | LoaderKind::NeoForge => {
                    let root = ctx.root.clone();
                    let mc_clone = mc.clone();
                    let lv_clone = lv.clone();
                    let kind_val = *kind;
                    let reused = tokio::task::spawn_blocking(move || {
                        loader_prepare::find_reusable_installer_loader(
                            &root,
                            kind_val,
                            &mc_clone,
                            &lv_clone,
                            current_ready_id.as_ref(),
                        )
                    })
                    .await
                    .map_err(storage)?;
                    match reused {
                        Some((id, reused_plan)) => Some(PlannedLoader::ReusedInstaller {
                            kind: *kind,
                            version: lv,
                            id,
                            plan: reused_plan,
                        }),
                        None => Some(PlannedLoader::RunInstaller {
                            kind: *kind,
                            version: lv,
                        }),
                    }
                }
            }
        }
    };
    ctx.check_cancel()?;

    // Pre-1.17 Minecraft uses Java 8 (`jre-legacy`), whose older root store rejects modern Let's
    // Encrypt certificates when Forge's installer downloads libraries. Run the installer itself on
    // Mojang's modern Java 21 runtime while keeping the legacy runtime for launching the game.
    let runs_installer = matches!(&loader, Some(PlannedLoader::RunInstaller { .. }));
    let game_java_major = match &inst.java {
        JavaChoice::Auto => Some(plan.java.major),
        JavaChoice::Runtime { .. } => installed_game_java.as_ref().map(|j| j.java_major),
    };
    let needs_installer_java = runs_installer
        && game_java_major.is_some_and(|m| m < 17)
        && match &inst.java {
            JavaChoice::Auto => plan.java.component != loader_prepare::INSTALLER_JAVA_COMPONENT,
            JavaChoice::Runtime { .. } => true,
        };
    let (installer_java_installed, installer_runtime_plan) = if needs_installer_java {
        let root = ctx.root.clone();
        let existing = tokio::task::spawn_blocking(move || {
            java_install::find_installed(&root, loader_prepare::INSTALLER_JAVA_COMPONENT)
        })
        .await
        .map_err(storage)?;
        let planned = if existing.is_none() {
            Some(
                java_install::plan_runtime(ctx.meta, loader_prepare::INSTALLER_JAVA_COMPONENT)
                    .await
                    .map_err(PrepareError::from)?,
            )
        } else {
            None
        };
        (existing, planned)
    } else {
        (None, None)
    };
    ctx.check_cancel()?;

    let progress = &ctx.tracker.progress;
    progress.plan_bytes(planned_bytes(
        &plan,
        assets.as_ref().map(|a| &a.1),
        runtime_plan.as_ref(),
    ));
    if let Some(irp) = &installer_runtime_plan {
        progress.plan_bytes(irp.total_bytes());
    }
    if let Some((_, files)) = &java_verify {
        progress.plan_bytes(
            files
                .iter()
                .map(|f| f.download.size)
                .fold(0u64, u64::saturating_add),
        );
    }
    if let Some(lp) = loader.as_ref().and_then(PlannedLoader::preplanned) {
        progress.plan_bytes(planned_bytes(lp, None, None));
    }

    // ---- 1b. enough room? (before anything big is downloaded)
    {
        let root = ctx.root.clone();
        let mc_str = mc.as_str().to_owned();
        let mut plans_owned = vec![plan.clone()];
        if let Some(lp) = loader.as_ref().and_then(PlannedLoader::preplanned) {
            plans_owned.push(lp.clone());
        }
        let assets_owned = assets.clone();
        let mut runtimes_owned = Vec::new();
        if let Some(rp) = &runtime_plan {
            runtimes_owned.push(rp.clone());
        }
        if let Some(irp) = &installer_runtime_plan {
            runtimes_owned.push(irp.clone());
        }
        let game_dir = dirs.game_dir();
        let free_space = ctx.free_space.clone();
        tokio::task::spawn_blocking(move || -> Result<(), PrepareError> {
            let plan_refs: Vec<&VersionPlan> = plans_owned.iter().collect();
            let runtime_refs: Vec<&RuntimePlan> = runtimes_owned.iter().collect();
            let to_download = bytes_to_download(
                &root,
                &mc_str,
                &plan_refs,
                assets_owned.as_ref().map(|(n, i)| (n.as_str(), i)),
                &game_dir,
                &runtime_refs,
            );
            if to_download > 0 || runs_installer {
                let needed = crate::system::space_needed(to_download, runs_installer);
                if let Some(free) = (free_space)(root.path()) {
                    if free < needed {
                        return Err(PrepareError::DiskSpace {
                            needed,
                            available: free,
                        });
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(storage)??;
    }

    // ---- 2. Java
    let java = ensure_java(ctx, &dl, &inst, &plan, runtime_plan.as_ref()).await?;
    ctx.check_cancel()?;
    if let Some((installed, files)) = &java_verify {
        ctx.tracker.enter(
            Stage::Java,
            ctx.label("Preparing Java", "Verifying Java"),
            Some(files.len() as u64),
            false,
        );
        dl.fetch_all(
            java_install::verification_jobs(installed, files),
            ctx.concurrency,
        )
        .await?;
        ctx.check_cancel()?;
    }
    let installer_java = if needs_installer_java {
        match installer_java_installed {
            Some(ij) => ij,
            None => {
                let irp = installer_runtime_plan
                    .as_ref()
                    .ok_or_else(|| PrepareError::Java("no installer Java runtime plan".into()))?;
                ctx.tracker.enter(
                    Stage::Java,
                    "Preparing Java",
                    Some(irp.files.len() as u64),
                    false,
                );
                let ij = java_install::install_runtime(
                    ctx.root,
                    irp,
                    loader_prepare::INSTALLER_JAVA_MAJOR,
                    &dl,
                    ctx.concurrency,
                )
                .await?;
                ctx.check_cancel()?;
                ij
            }
        }
    } else {
        java.clone()
    };

    // ---- 3. the game file and logging configuration
    let mut jobs = vec![FileJob {
        url: client.url.clone(),
        dest: client_jar_path(ctx.root, mc.as_str()),
        expected: Expected::Sha1(client.sha1.clone()),
        size: Some(client.size),
    }];
    if let Some(log) = &plan.logging {
        jobs.push(FileJob {
            url: log.download.url.clone(),
            dest: log_config_path(ctx.root, &log.file_name),
            expected: Expected::Sha1(log.download.sha1.clone()),
            size: Some(log.download.size),
        });
    }
    ctx.tracker.enter(
        Stage::Minecraft,
        ctx.label("Downloading Minecraft", "Verifying Minecraft"),
        Some(jobs.len() as u64),
        false,
    );
    dl.fetch_all(jobs, ctx.concurrency).await?;
    ctx.check_cancel()?;

    // ---- 4. libraries
    let libs: Vec<&Library> = plan.libraries.iter().collect();
    let (jobs, generated) = jobs_for_libraries(ctx, &libs, &Default::default()).await?;
    if !generated.is_empty() {
        return Err(PrepareError::Unsupported(
            "a Minecraft library has no download".into(),
        ));
    }
    ctx.tracker.enter(
        Stage::Libraries,
        ctx.label("Downloading libraries", "Verifying libraries"),
        Some(jobs.len() as u64),
        false,
    );
    dl.fetch_all(jobs, ctx.concurrency).await?;
    ctx.check_cancel()?;

    // ---- 5. assets
    if let Some((name, index)) = &assets {
        ensure_assets(ctx, &dl, index, name, &dirs.game_dir()).await?;
        ctx.check_cancel()?;
    }

    // ---- 6. the mod loader (before natives so loader native libraries are included)
    let (version_id, loader_version, installed_loader_plan): (
        VersionId,
        Option<LoaderVersion>,
        Option<VersionPlan>,
    ) = match loader {
        None => (
            VersionId::new(mc.as_str()).map_err(|e| PrepareError::Unsupported(e.to_string()))?,
            None,
            None,
        ),
        Some(PlannedLoader::Profile {
            kind,
            version,
            profile,
        }) => {
            ctx.tracker.enter(
                Stage::Loader,
                ctx.label(
                    &format!("Installing {}", kind.name()),
                    &format!("Verifying {}", kind.name()),
                ),
                None,
                false,
            );
            loader_prepare::install_profile_loader(ctx, &dl, &profile).await?;
            (profile.id, Some(version), Some(profile.plan))
        }
        Some(PlannedLoader::ReusedInstaller {
            kind,
            version,
            id,
            plan: reused_plan,
        }) => {
            ctx.tracker.enter(
                Stage::Loader,
                ctx.label(
                    &format!("Installing {}", kind.name()),
                    &format!("Verifying {}", kind.name()),
                ),
                None,
                false,
            );
            loader_prepare::ensure_libraries(ctx, &dl, &reused_plan).await?;
            (id, Some(version), Some(reused_plan))
        }
        Some(PlannedLoader::RunInstaller { kind, version }) => {
            let id =
                loader_prepare::install_with_installer(ctx, kind, &mc, &version, &installer_java)
                    .await?;
            let produced = {
                let root = ctx.root.clone();
                let id_clone = id.clone();
                tokio::task::spawn_blocking(move || loader_prepare::read_profile(&root, &id_clone))
                    .await
                    .map_err(storage)??
            };
            loader_prepare::ensure_libraries(ctx, &dl, &produced).await?;
            (id, Some(version), Some(produced))
        }
    };
    ctx.check_cancel()?;

    // ---- 7. natives (instance-only, rebuilt every time across merged [loader, vanilla])
    ctx.tracker
        .enter(Stage::Natives, "Extracting natives", None, true);
    {
        let (root, natives_dir) = (ctx.root.clone(), dirs.natives_dir());
        let natives = merged_natives(&plan, installed_loader_plan.as_ref());
        tokio::task::spawn_blocking(move || {
            let refs: Vec<&Library> = natives.iter().collect();
            extract_natives(&root, &natives_dir, &refs)
        })
        .await
        .map_err(storage)?
        .map_err(|e| PrepareError::Unsupported(e.to_string()))?;
    }
    ctx.check_cancel()?;

    // ---- 8. the final gate: nothing becomes Ready on trust
    ctx.tracker
        .enter(Stage::Verifying, "Verifying files", None, true);
    let root = ctx.root.clone();
    let ready = tokio::task::spawn_blocking(move || -> Result<Instance, PrepareError> {
        let loader_id = loader_version.as_ref().map(|_| &version_id);
        check_files(&root, &inst, loader_id)
            .map_err(|what| PrepareError::Storage(format!("{what} is missing after setup")))?;
        Ok(InstanceStore::new(&root).mark_ready(&inst.id, version_id, loader_version)?)
    })
    .await
    .map_err(storage)??;
    Ok(ready)
}

#[cfg(test)]
pub(crate) mod tests;
