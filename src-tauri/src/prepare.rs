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
use crate::portable::fsutil::{atomic_write, Expected};
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

/// Download jobs for libraries (deduplicated), plus the files that only an installer can produce.
pub async fn jobs_for_libraries<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    libs: &[&Library],
    recorded: &std::collections::HashMap<String, u64>,
) -> Result<(Vec<FileJob>, Vec<PathBuf>), PrepareError> {
    let base = ctx.root.libraries_dir();
    let mut seen = HashSet::new();
    let (mut jobs, mut generated) = (Vec::new(), Vec::new());
    for lib in libs {
        let dest = lib.artifact.path.to_path(&base);
        if !seen.insert(dest.clone()) {
            continue;
        }
        let Some(url) = &lib.artifact.url else {
            generated.push(dest);
            continue;
        };
        let expected = match &lib.artifact.expected {
            Some(e) => e.clone(),
            // Nothing in the metadata to verify an existing copy against: it came through a
            // sidecar-verified download, whose size was recorded then. A different size means damage.
            None if dest.is_file()
                && recorded.get(lib.artifact.path.as_str()).is_none_or(|&n| {
                    fs::metadata(&dest).map(|m| m.len() == n).unwrap_or(false)
                }) =>
            {
                continue
            }
            None => sidecar_sha1(ctx.meta, url).await?,
        };
        jobs.push(FileJob {
            url: url.clone(),
            dest,
            expected,
            size: lib.artifact.size,
        });
    }
    Ok((jobs, generated))
}

// ------------------------------------------------------------------------------ offline check

/// Every file a launch of `inst` needs, checked by looking at the local disk only (existence and
/// size, no hashing, no network). `loader_id` is the loader profile for Modded instances.
/// `Err` describes the first thing that is missing.
pub fn check_files(
    root: &PortableRoot,
    inst: &Instance,
    loader_id: Option<&VersionId>,
) -> Result<(), String> {
    let mc = inst.minecraft.as_str();
    let bytes = fs::read(version_json_path(root, mc))
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
        let idx =
            fs::read(asset_index_path(root, &ai.id)).map_err(|_| "the asset index".to_owned())?;
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
    }
    if plan.natives().next().is_some() {
        let natives = InstanceStore::new(root).dirs(&inst.id).natives_dir();
        if fs::read_dir(&natives)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
        {
            return Err("the extracted native libraries".into());
        }
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
        let len = fs::metadata(&p)
            .map(|m| m.len())
            .map_err(|_| format!("the library {}", lib.name))?;
        let want = lib
            .artifact
            .size
            .or_else(|| recorded.get(lib.artifact.path.as_str()).copied());
        if want.is_some_and(|s| s != len) {
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
    if let Ok(b) = fs::read(&path) {
        if parse_version(&b, &WINDOWS_X64).is_ok() {
            return Ok(b); // saved earlier, after it was verified: no network needed
        }
    }
    let cache = ctx
        .root
        .cache_dir()
        .join("catalogs")
        .join("version_manifest.json");
    let mut catalog = mc_catalog::load(ctx.meta, &cache, ctx.now, false)
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
    let url =
        Url::parse(&url).map_err(|_| PrepareError::Metadata("a malformed metadata link".into()))?;
    let body = ctx
        .meta
        .get(&url, MAX_VERSION_JSON)
        .await
        .map_err(|e| fetch_err(e, &format!("the Minecraft {mc} information")))?;
    if hex::encode(Sha1::digest(&body)) != sha1 {
        return Err(PrepareError::Verification("version information".into()));
    }
    atomic_write(&path, &body).map_err(storage)?;
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
    let bytes = fs::read(&dest).map_err(storage)?;
    if bytes.len() > MAX_INDEX_JSON {
        return Err(PrepareError::Unsupported(
            "the asset index is unexpectedly large".into(),
        ));
    }
    Ok(Some((r.assets_name.clone(), parse_asset_index(&bytes)?)))
}

async fn ensure_java<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    dl: &Downloader<'_, S>,
    inst: &Instance,
    plan: &VersionPlan,
    runtime_plan: Option<&RuntimePlan>,
) -> Result<ManagedJava, PrepareError> {
    if let JavaChoice::Runtime { id } = &inst.java {
        return resolve_java(ctx.root, id).map_err(|_| {
            PrepareError::Java("the Java runtime chosen for this profile isn't installed".into())
        });
    }
    if let Some(java) = java_install::find_installed(ctx.root, &plan.java.component) {
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

fn copy_named(src: &Path, dst: &Path, size: u64) -> io::Result<()> {
    if fs::metadata(dst).map(|m| m.len()).ok() == Some(size) {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = dst.with_extension("copy.tmp");
    fs::copy(src, &tmp)?;
    fs::rename(&tmp, dst)
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
        let (objects, wants_virtual, wants_resources) = (
            index.objects.clone(),
            index.virtual_files,
            index.map_to_resources,
        );
        let root = ctx.root.clone();
        tokio::task::spawn_blocking(move || -> Result<(), PrepareError> {
            for o in &objects {
                let name = RelPath::new(&o.name)
                    .map_err(|_| PrepareError::Unsupported("an unsafe asset name".into()))?;
                let src = object_path(&root, &o.store_path());
                if wants_virtual {
                    copy_named(&src, &name.to_path(&virtual_dir), o.size).map_err(storage)?;
                }
                if wants_resources {
                    copy_named(&src, &name.to_path(&resources), o.size).map_err(storage)?;
                }
            }
            Ok(())
        })
        .await
        .map_err(storage)??;
    }
    Ok(())
}

fn planned_bytes(
    plan: &VersionPlan,
    index: Option<&AssetIndex>,
    runtime: Option<&RuntimePlan>,
) -> u64 {
    let mut seen = HashSet::new();
    let libs: u64 = plan
        .libraries
        .iter()
        .filter(|l| l.artifact.url.is_some() && seen.insert(l.artifact.path.clone()))
        .filter_map(|l| l.artifact.size)
        .sum();
    let mut hashes = HashSet::new();
    let assets: u64 = index
        .map(|i| {
            i.objects
                .iter()
                .filter(|o| hashes.insert(o.hash.clone()))
                .map(|o| o.size)
                .sum()
        })
        .unwrap_or(0);
    plan.client.as_ref().map_or(0, |c| c.size)
        + plan.logging.as_ref().map_or(0, |l| l.download.size)
        + libs
        + assets
        + runtime.map_or(0, RuntimePlan::total_bytes)
}

fn missing(path: &Path, size: Option<u64>) -> bool {
    match (fs::metadata(path).ok().map(|m| m.len()), size) {
        (None, _) => true,
        (Some(len), Some(want)) => len != want,
        (Some(_), None) => false,
    }
}

/// Bytes that still have to be downloaded: planned files that are absent or the wrong size.
/// (Files already present and verified need no space.) The Java runtime counts in full when it
/// has to be installed.
fn bytes_to_download(
    root: &PortableRoot,
    mc: &str,
    plans: &[&VersionPlan],
    index: Option<&AssetIndex>,
    runtime: Option<&RuntimePlan>,
) -> u64 {
    let mut n = runtime.map_or(0, RuntimePlan::total_bytes);
    let mut seen = HashSet::new();
    for plan in plans {
        if let Some(c) = &plan.client {
            if missing(&client_jar_path(root, mc), Some(c.size)) {
                n += c.size;
            }
        }
        if let Some(l) = &plan.logging {
            if missing(&log_config_path(root, &l.file_name), Some(l.download.size)) {
                n += l.download.size;
            }
        }
        for lib in &plan.libraries {
            let dest = lib.artifact.path.to_path(&root.libraries_dir());
            if lib.artifact.url.is_some()
                && seen.insert(dest.clone())
                && missing(&dest, lib.artifact.size)
            {
                n += lib.artifact.size.unwrap_or(0);
            }
        }
    }
    let mut hashes = HashSet::new();
    for o in index.map(|i| i.objects.as_slice()).unwrap_or_default() {
        if hashes.insert(o.hash.clone())
            && missing(&object_path(root, &o.store_path()), Some(o.size))
        {
            n += o.size;
        }
    }
    n
}

// ------------------------------------------------------------------------------ the whole thing

/// Prepares `id`. Safe to call again at any time: it reuses everything already verified.
pub async fn prepare_instance<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    id: &InstanceId,
) -> Result<Instance, PrepareError> {
    let store = InstanceStore::new(ctx.root);
    let inst = store.get(id)?;
    ctx.tracker
        .enter(Stage::Planning, "Preparing Minecraft", None, true);

    if inst.is_ready() && !ctx.verify_all && check_installed(ctx.root, &inst).is_ok() {
        ctx.tracker
            .enter(Stage::Done, "Ready to play.", None, false);
        return Ok(inst); // already prepared: nothing to do, and no network used
    }
    let result = run(ctx, &store, inst).await;
    if result.is_ok() {
        ctx.tracker
            .enter(Stage::Done, "Ready to play.", None, false);
    }
    result
}

async fn run<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    store: &InstanceStore<'_>,
    inst: Instance,
) -> Result<Instance, PrepareError> {
    let mc = inst.minecraft.clone();
    let dirs = store.dirs(&inst.id);
    let dl = ctx.downloader();

    // ---- 1. read the metadata (everything is planned before anything big is downloaded)
    let vjson = ensure_version_json(ctx, &mc).await?;
    let plan = parse_version(&vjson, &WINDOWS_X64)?;
    let client = plan.client.clone().ok_or_else(|| {
        PrepareError::Unsupported("Mojang publishes no client download for it".into())
    })?;
    let assets = ensure_asset_index(ctx, &dl, &plan).await?;
    ctx.check_cancel()?;

    let runtime_plan = match &inst.java {
        JavaChoice::Auto
            if java_install::find_installed(ctx.root, &plan.java.component).is_none() =>
        {
            Some(
                java_install::plan_runtime(ctx.meta, &plan.java.component)
                    .await
                    .map_err(PrepareError::from)?,
            )
        }
        _ => None,
    };

    // A full verification also covers the managed Java runtime that is already installed: every file
    // the runtime's manifest lists is checked and the damaged or missing ones are fetched again.
    // (A runtime that is about to be installed fresh needs no check, and the everyday launch only
    // re-hashes java.exe.) Without an authoritative list the check stays at java.exe; nothing is
    // invented for files no manifest covers.
    let java_verify: Option<(ManagedJava, Vec<java_install::RuntimeFile>)> = if ctx.verify_all
        && runtime_plan.is_none()
    {
        let (installed, component) = match &inst.java {
            JavaChoice::Auto => (
                java_install::find_installed(ctx.root, &plan.java.component),
                Some(plan.java.component.as_str()),
            ),
            JavaChoice::Runtime { id } => (resolve_java(ctx.root, id).ok(), None),
        };
        match installed {
            Some(java) => java_install::verification_files(ctx.root, &java, component, ctx.meta)
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

    let loader = match &inst.profile {
        Profile::Vanilla => None,
        Profile::Modded {
            loader,
            loader_version,
        } => {
            let lv =
                loader_prepare::resolve_loader_version(ctx, *loader, &mc, loader_version).await?;
            let profile = match loader {
                LoaderKind::Fabric | LoaderKind::Quilt => {
                    Some(loader_prepare::plan_profile_loader(ctx, *loader, &mc, &lv).await?)
                }
                LoaderKind::Forge | LoaderKind::NeoForge => None,
            };
            Some((*loader, lv, profile))
        }
    };
    ctx.check_cancel()?;

    let progress = &ctx.tracker.progress;
    progress.plan_bytes(planned_bytes(
        &plan,
        assets.as_ref().map(|a| &a.1),
        runtime_plan.as_ref(),
    ));
    if let Some((_, files)) = &java_verify {
        progress.plan_bytes(files.iter().map(|f| f.download.size).sum());
    }
    if let Some((_, _, Some(p))) = &loader {
        progress.plan_bytes(planned_bytes(&p.plan, None, None));
    }

    // ---- 1b. enough room? (before anything big is downloaded)
    {
        let loader_plan = loader
            .as_ref()
            .and_then(|(_, _, p)| p.as_ref())
            .map(|p| &p.plan);
        let plans: Vec<&VersionPlan> = std::iter::once(&plan).chain(loader_plan).collect();
        let to_download = bytes_to_download(
            ctx.root,
            mc.as_str(),
            &plans,
            assets.as_ref().map(|a| &a.1),
            runtime_plan.as_ref(),
        );
        let installer = matches!(&loader, Some((_, _, None)));
        if to_download > 0 || installer {
            let needed = crate::system::space_needed(to_download, installer);
            if let Some(free) = (ctx.free_space)(ctx.root.path()) {
                if free < needed {
                    return Err(PrepareError::DiskSpace {
                        needed,
                        available: free,
                    });
                }
            }
        }
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

    // ---- 6. natives (instance-only, rebuilt every time)
    ctx.tracker
        .enter(Stage::Natives, "Extracting natives", None, true);
    {
        let (root, natives_dir) = (ctx.root.clone(), dirs.natives_dir());
        let natives: Vec<Library> = plan.natives().cloned().collect();
        tokio::task::spawn_blocking(move || {
            let refs: Vec<&Library> = natives.iter().collect();
            extract_natives(&root, &natives_dir, &refs)
        })
        .await
        .map_err(storage)?
        .map_err(|e| PrepareError::Unsupported(e.to_string()))?;
    }
    ctx.check_cancel()?;

    // ---- 7. the mod loader
    let (version_id, loader_version): (VersionId, Option<LoaderVersion>) = match loader {
        None => (
            VersionId::new(mc.as_str()).map_err(|e| PrepareError::Unsupported(e.to_string()))?,
            None,
        ),
        Some((kind, lv, Some(profile))) => {
            ctx.tracker.enter(
                Stage::Loader,
                format!("Installing {}", kind.name()),
                None,
                false,
            );
            loader_prepare::install_profile_loader(ctx, &dl, &profile).await?;
            (profile.id, Some(lv))
        }
        Some((kind, lv, None)) => {
            let id = loader_prepare::install_with_installer(ctx, kind, &mc, &lv, &java).await?;
            let produced = loader_prepare::read_profile(ctx.root, &id)?;
            loader_prepare::ensure_libraries(ctx, &dl, &produced).await?;
            (id, Some(lv))
        }
    };
    ctx.check_cancel()?;

    // ---- 8. the final gate: nothing becomes Ready on trust
    ctx.tracker
        .enter(Stage::Verifying, "Verifying files", None, true);
    let loader_id = loader_version.as_ref().map(|_| &version_id);
    check_files(ctx.root, &inst, loader_id)
        .map_err(|what| PrepareError::Storage(format!("{what} is missing after setup")))?;
    Ok(store.mark_ready(&inst.id, version_id, loader_version)?)
}

#[cfg(test)]
pub(crate) mod tests;
