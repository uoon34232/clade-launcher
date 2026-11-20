//! Installing a mod loader into the shared stores, as part of preparing a Modded instance.
//!
//! The underlying Vanilla version is always prepared first (see `prepare`). Then:
//! - **Fabric / Quilt**: no code runs. The official profile JSON (from the loader's own host) is
//!   validated, saved as `minecraft/versions/<id>/<id>.json`, and each library it names is
//!   downloaded to `libraries/` and verified with the hash inside the profile (or the `.sha1`
//!   the Maven publishes beside the file).
//! - **Forge / NeoForge**: the official installer jar is downloaded from the loader's Maven,
//!   verified against the `.sha256` published beside it, and then run **only** inside the
//!   sandbox of `loader_install`: a staging tree under `cache/` stands in for `.minecraft`, the
//!   user profile, AppData and TEMP; the process gets a scrubbed environment; a tripwire watches
//!   the real global `.minecraft`; and only `libraries/` and `versions/` are merged into the
//!   shared stores afterwards. The launchable id is read from what the installer produced, and
//!   every library the produced profile names is then verified (including generated local jars).

use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::download::{Cancel, Downloader, FileJob};
use crate::instances::LoaderVersionSpec;
use crate::loader_install::{
    build_installer_command, merge_stage_into_shared, verify_command, InstallerCommand,
    StageSession, Tripwire,
};
use crate::loaders::{self, InstallPlan, LoaderKind};
use crate::mc_meta::{parse_version, VersionPlan, WINDOWS_X64};
use crate::net::{FetchError, Fetcher, Streamer};
use crate::portable::fsutil::{
    atomic_write, is_link, read_bounded, reject_links_between, remove_file_with_retry, Expected,
};
use crate::portable::PortableRoot;
use crate::prepare::{
    fetch_err, jobs_for_libraries, verify_generated_file, Ctx, GeneratedFile, PrepareError, Stage,
};
use crate::runtimes::ManagedJava;
use crate::versions::{LoaderVersion, McVersion, VersionId};

/// Mojang runtime component used to run modern Forge/NeoForge installer jars when the game itself
/// runs on an older Java (for example Minecraft 1.12.2 on Java 8).
pub const INSTALLER_JAVA_COMPONENT: &str = "java-runtime-delta";
/// Java major version corresponding to [`INSTALLER_JAVA_COMPONENT`].
pub const INSTALLER_JAVA_MAJOR: u32 = 21;

/// An installer that runs longer than this is stopped.
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const MAX_PROFILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_SIZES_BYTES: u64 = 1024 * 1024;

// ------------------------------------------------------------------------------ the process

pub struct RunResult {
    pub success: bool,
    /// The end of the installer's output, for a useful error message.
    pub tail: String,
}

/// Runs a verified installer command. The real one starts a process; tests script the outcome.
pub trait InstallerRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        cmd: &'a InstallerCommand,
        cancel: &'a Cancel,
    ) -> impl Future<Output = io::Result<RunResult>> + Send + 'a;
}

pub struct ProcessRunner;

async fn drain<R: tokio::io::AsyncRead + Unpin>(mut r: R, tail: Arc<Mutex<Vec<u8>>>) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
        t.extend_from_slice(&buf[..n]);
        if t.len() > 16 * 1024 {
            let cut = t.len() - 8 * 1024;
            t.drain(..cut);
        }
    }
}

fn attach_kill_on_close_job(
    child: &tokio::process::Child,
) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    let proc_handle = child.raw_handle()?;
    // SAFETY: Anonymous job object with default security attributes; checked for null before
    // wrapping in `OwnedHandle`, which guarantees `CloseHandle` on drop.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            tracing::warn!(
                error = %io::Error::last_os_error(),
                "could not create Win32 job object for loader installer"
            );
            return None;
        }
        let guard = std::os::windows::io::OwnedHandle::from_raw_handle(job.cast());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            tracing::warn!(
                error = %io::Error::last_os_error(),
                "could not configure Win32 job object for loader installer"
            );
            return None;
        }
        if AssignProcessToJobObject(job, proc_handle.cast()) == 0 {
            tracing::warn!(
                error = %io::Error::last_os_error(),
                "could not assign loader installer process to Win32 job object"
            );
            return None;
        }
        Some(guard)
    }
}

async fn run_command(
    cmd: &InstallerCommand,
    cancel: &Cancel,
    timeout: Duration,
) -> io::Result<RunResult> {
    // `to_process` is the single place installer processes are created (scrubbed env).
    let mut c = tokio::process::Command::from(cmd.to_process());
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = c.spawn()?;
    let job_guard = attach_kill_on_close_job(&child);
    let tail = Arc::new(Mutex::new(Vec::new()));
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| tokio::spawn(drain(s, tail.clone()))),
        child
            .stderr
            .take()
            .map(|s| tokio::spawn(drain(s, tail.clone()))),
    ]
    .into_iter()
    .flatten()
    .collect();
    let status = tokio::select! {
        s = child.wait() => s?,
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            drop(job_guard);
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            for r in readers {
                let _ = tokio::time::timeout(Duration::from_secs(1), r).await;
            }
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        () = tokio::time::sleep(timeout) => {
            let _ = child.kill().await;
            drop(job_guard);
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            for r in readers {
                let _ = tokio::time::timeout(Duration::from_secs(1), r).await;
            }
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the installer took too long"));
        }
    };
    drop(job_guard);
    for r in readers {
        let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
    }
    let bytes = tail.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let last: Vec<&str> = text.lines().rev().take(6).collect();
    Ok(RunResult {
        success: status.success(),
        tail: last.into_iter().rev().collect::<Vec<_>>().join(" | "),
    })
}

impl InstallerRunner for ProcessRunner {
    #[allow(clippy::manual_async_fn)] // the trait spells out the borrowed lifetime
    fn run<'a>(
        &'a self,
        cmd: &'a InstallerCommand,
        cancel: &'a Cancel,
    ) -> impl Future<Output = io::Result<RunResult>> + Send + 'a {
        async move { run_command(cmd, cancel, INSTALLER_TIMEOUT).await }
    }
}

// ------------------------------------------------------------------------------ resolving

/// The concrete loader version to install: the pinned one, or the best compatible one from the
/// loader's official catalog (live, or its saved copy when offline).
pub async fn resolve_loader_version<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    kind: LoaderKind,
    mc: &McVersion,
    spec: &LoaderVersionSpec,
) -> Result<LoaderVersion, PrepareError> {
    if let LoaderVersionSpec::Pinned { version } = spec {
        return Ok(version.clone());
    }
    let dir = ctx.root.cache_dir().join("catalogs");
    let options =
        crate::loader_options::options_for(ctx.loader_meta, &dir, kind, mc, ctx.now).await;
    options
        .recommended
        .or_else(|| {
            loaders::choose_auto(&options.versions)
                .and_then(|o| LoaderVersion::new(o.version.as_str()).ok())
        })
        .ok_or_else(|| {
            PrepareError::Loader(format!(
                "{} has no version known to work with Minecraft {mc}.",
                kind.name()
            ))
        })
}

fn version_dir(ctx_root: &PortableRoot, id: &VersionId) -> PathBuf {
    ctx_root.minecraft_dir().join("versions").join(id.as_str())
}

fn profile_path(root: &PortableRoot, id: &VersionId) -> PathBuf {
    version_dir(root, id).join(format!("{id}.json"))
}

/// What a Fabric/Quilt installation will do, decided before anything is downloaded.
pub struct ProfilePlan {
    pub id: VersionId,
    pub bytes: Vec<u8>,
    pub plan: VersionPlan,
}

async fn read_valid_local_profile(
    root: &PortableRoot,
    expected: &VersionId,
    mc: &McVersion,
) -> Option<ProfilePlan> {
    let root = root.clone();
    let expected = expected.clone();
    let mc = mc.clone();
    tokio::task::spawn_blocking(move || {
        let local = profile_path(&root, &expected);
        reject_links_between(root.path(), &local).ok()?;
        let bytes = read_bounded(&local, MAX_PROFILE_BYTES as u64).ok()?;
        let plan = parse_version(&bytes, &WINDOWS_X64).ok()?;
        if plan.id != expected.as_str() || plan.inherits_from.as_deref() != Some(mc.as_str()) {
            return None;
        }
        Some(ProfilePlan {
            id: expected,
            bytes,
            plan,
        })
    })
    .await
    .ok()
    .flatten()
}

/// Fetches (or reuses the saved copy of) the official Fabric/Quilt profile and validates it.
pub async fn plan_profile_loader<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    kind: LoaderKind,
    mc: &McVersion,
    loader: &LoaderVersion,
) -> Result<ProfilePlan, PrepareError> {
    let expected = loaders::expected_version_id(kind, mc, loader)
        .map_err(|e| PrepareError::Loader(e.to_string()))?;
    if !ctx.verify_all {
        if let Some(cached) = read_valid_local_profile(ctx.root, &expected, mc).await {
            return Ok(cached);
        }
    }

    let InstallPlan::Profile { profile_url } =
        loaders::install_plan(kind, mc, loader).map_err(|e| PrepareError::Loader(e.to_string()))?
    else {
        return Err(PrepareError::Loader("unexpected install plan".into()));
    };
    let bytes = match ctx.loader_meta.get(&profile_url, MAX_PROFILE_BYTES).await {
        Ok(b) => b,
        Err(FetchError::Network(msg)) => {
            if let Some(cached) = read_valid_local_profile(ctx.root, &expected, mc).await {
                return Ok(cached);
            }
            return Err(fetch_err(
                FetchError::Network(msg),
                &format!("the {} profile", kind.name()),
            ));
        }
        Err(e) => return Err(fetch_err(e, &format!("the {} profile", kind.name()))),
    };
    let plan = parse_version(&bytes, &WINDOWS_X64).map_err(|e| {
        PrepareError::Loader(format!("the {} profile is not usable: {e}", kind.name()))
    })?;
    let id = VersionId::new(&plan.id)
        .map_err(|_| PrepareError::Loader("the loader profile has an unsafe id".into()))?;
    if plan.inherits_from.as_deref() != Some(mc.as_str()) {
        return Err(PrepareError::Loader(
            "the loader profile is for a different Minecraft version".into(),
        ));
    }
    Ok(ProfilePlan { id, bytes, plan })
}

/// Saves the validated profile and downloads/verifies its libraries.
pub async fn install_profile_loader<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    downloader: &Downloader<'_, S>,
    p: &ProfilePlan,
) -> Result<(), PrepareError> {
    let root = ctx.root.clone();
    let id = p.id.clone();
    let bytes = p.bytes.clone();
    tokio::task::spawn_blocking(move || -> Result<(), PrepareError> {
        let path = profile_path(&root, &id);
        reject_links_between(root.path(), &path)
            .map_err(|e| PrepareError::Storage(e.to_string()))?;
        let unchanged = read_bounded(&path, MAX_PROFILE_BYTES as u64)
            .ok()
            .as_deref()
            == Some(bytes.as_slice());
        if !unchanged {
            atomic_write(&path, &bytes).map_err(|e| PrepareError::Storage(e.to_string()))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| PrepareError::Storage(e.to_string()))??;
    ensure_libraries(ctx, downloader, &p.plan).await
}

/// Libraries whose metadata publishes neither a size nor a hash (Quilt's, and Fabric's own jars)
/// can only be verified through the `.sha1` beside them, once. The size seen then is recorded here
/// so later checks can still notice a truncated file without any network.
fn sizes_path(root: &PortableRoot, id: &str) -> PathBuf {
    root.minecraft_dir()
        .join("versions")
        .join(id)
        .join(format!("{id}.sizes.json"))
}

pub fn read_sizes(root: &PortableRoot, id: &str) -> std::collections::HashMap<String, u64> {
    let path = sizes_path(root, id);
    if reject_links_between(root.path(), &path).is_err() {
        return Default::default();
    }
    read_bounded(&path, MAX_SIZES_BYTES)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_sizes(root: &PortableRoot, plan: &VersionPlan) {
    let mut map = std::collections::BTreeMap::new();
    for lib in plan
        .libraries
        .iter()
        .filter(|l| l.artifact.url.is_some() && l.artifact.size.is_none())
    {
        if let Ok(m) = fs::metadata(lib.artifact.path.to_path(&root.libraries_dir())) {
            map.insert(lib.artifact.path.as_str().to_owned(), m.len());
        }
    }
    if !map.is_empty() {
        let path = sizes_path(root, &plan.id);
        if reject_links_between(root.path(), &path).is_ok() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&map) {
                let _ = atomic_write(&path, &bytes);
            }
        }
    }
}

/// Downloads (or checks) every library a profile names, and verifies any installer-generated jars.
pub async fn ensure_libraries<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    downloader: &Downloader<'_, S>,
    plan: &VersionPlan,
) -> Result<(), PrepareError> {
    let recorded = {
        let root = ctx.root.clone();
        let id = plan.id.clone();
        tokio::task::spawn_blocking(move || read_sizes(&root, &id))
            .await
            .map_err(|e| PrepareError::Storage(e.to_string()))?
    };
    let libs: Vec<_> = plan.libraries.iter().collect();
    let (jobs, generated) = jobs_for_libraries(ctx, &libs, &recorded).await?;
    ctx.tracker.enter(
        Stage::Verifying,
        "Verifying files",
        Some(jobs.len() as u64),
        false,
    );
    ctx.tracker
        .progress
        .plan_bytes(jobs.iter().filter_map(|j| j.size).sum());
    downloader.fetch_all(jobs, ctx.concurrency).await?;

    let root = ctx.root.clone();
    let plan_clone = plan.clone();
    tokio::task::spawn_blocking(move || -> Result<(), PrepareError> {
        write_sizes(&root, &plan_clone);
        for g in &generated {
            verify_generated_file(g, &root)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| PrepareError::Storage(e.to_string()))?
}

// ------------------------------------------------------------------------------ Forge / NeoForge

fn first_hex_token(body: &[u8], len: usize) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let token = text.split_whitespace().next()?;
    (token.len() == len && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

/// Version folders the installer produced, other than the vanilla one it started from.
fn new_version_ids(stage: &StageSession, mc: &McVersion) -> Result<Vec<VersionId>, PrepareError> {
    let dir = stage.minecraft_dir().join("versions");
    let mut ids = Vec::new();
    for e in fs::read_dir(&dir)
        .map_err(|e| PrepareError::Loader(e.to_string()))?
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if name != mc.as_str() && e.path().is_dir() {
            ids.push(VersionId::new(&name).map_err(|_| {
                PrepareError::Loader("the installer produced an unsafe version name".into())
            })?);
        }
    }
    Ok(ids)
}

/// Checks whether a Forge or NeoForge loader profile and all of its locally generated (`url: None`)
/// libraries are already installed and intact in the shared store, allowing preparation or
/// verification to reuse them without downloading the installer or spawning Java again.
pub fn find_reusable_installer_loader(
    root: &PortableRoot,
    kind: LoaderKind,
    mc: &McVersion,
    loader: &LoaderVersion,
    current_ready_id: Option<&VersionId>,
) -> Option<(VersionId, VersionPlan)> {
    if !matches!(kind, LoaderKind::Forge | LoaderKind::NeoForge) {
        return None;
    }
    let versions_dir = root.minecraft_dir().join("versions");
    reject_links_between(root.path(), &versions_dir).ok()?;

    let mut candidates: Vec<VersionId> = Vec::new();
    let mut push_unique = |id: VersionId| {
        if !candidates.contains(&id) {
            candidates.push(id);
        }
    };
    if let Some(cur) = current_ready_id {
        push_unique(cur.clone());
    }
    if let Ok(expected) = loaders::expected_version_id(kind, mc, loader) {
        push_unique(expected);
    }
    if kind == LoaderKind::Forge {
        let prefix = format!("{mc}-forge");
        if let Ok(rd) = fs::read_dir(&versions_dir) {
            let mut extras: Vec<String> = rd
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if is_link(&p).unwrap_or(true) || !e.file_type().ok()?.is_dir() {
                        return None;
                    }
                    let name = e.file_name().into_string().ok()?;
                    (name.starts_with(&prefix) && name.ends_with(loader.as_str())).then_some(name)
                })
                .collect();
            extras.sort();
            for name in extras {
                if let Ok(id) = VersionId::new(&name) {
                    push_unique(id);
                }
            }
        }
    }

    let libs_dir = root.libraries_dir();
    for id in candidates {
        let Ok(plan) = read_profile(root, &id) else {
            continue;
        };
        if plan.id != id.as_str() || plan.inherits_from.as_deref() != Some(mc.as_str()) {
            continue;
        }
        let generated_ok = plan
            .libraries
            .iter()
            .filter(|lib| lib.artifact.url.is_none())
            .all(|lib| {
                let g = GeneratedFile {
                    dest: lib.artifact.path.to_path(&libs_dir),
                    expected: lib.artifact.expected.clone(),
                    size: lib.artifact.size,
                };
                verify_generated_file(&g, root).is_ok()
            });
        if generated_ok {
            return Some((id, plan));
        }
    }
    None
}

/// Runs the official installer in the sandbox and merges its result. Returns the launchable id.
pub async fn install_with_installer<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    kind: LoaderKind,
    mc: &McVersion,
    loader: &LoaderVersion,
    java: &ManagedJava,
) -> Result<VersionId, PrepareError> {
    let InstallPlan::Installer {
        installer_url,
        sha256_url,
    } = loaders::install_plan(kind, mc, loader).map_err(|e| PrepareError::Loader(e.to_string()))?
    else {
        return Err(PrepareError::Loader("unexpected install plan".into()));
    };
    let sha = ctx
        .loader_meta
        .get(&sha256_url, 1024)
        .await
        .map_err(|e| fetch_err(e, &format!("the {} installer's checksum", kind.name())))?;
    let sha = first_hex_token(&sha, 64).ok_or_else(|| {
        PrepareError::Loader("the installer's published checksum is not valid".into())
    })?;

    let jar_name = installer_url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|n| n.ends_with(".jar") && !n.contains(['\\', '/', ':']))
        .ok_or_else(|| PrepareError::Loader("unexpected installer name".into()))?
        .to_owned();
    let jar = ctx.root.downloads_dir().join(&jar_name);
    ctx.tracker.enter(
        Stage::Loader,
        format!("Downloading the {} installer", kind.name()),
        Some(1),
        false,
    );
    let installer_dl = Downloader::new(
        ctx.loader_streamer,
        ctx.cancel.clone(),
        ctx.tracker.progress.clone(),
    )
    .with_backoff(ctx.backoff)
    .with_root(ctx.root.path());
    installer_dl
        .fetch(&FileJob {
            url: installer_url,
            dest: jar.clone(),
            expected: Expected::Sha256(sha),
            size: None,
        })
        .await?;

    let root_for_stage = ctx.root.clone();
    let stage = tokio::task::spawn_blocking(move || StageSession::create(&root_for_stage))
        .await
        .map_err(|e| PrepareError::Storage(e.to_string()))?
        .map_err(|e| PrepareError::Loader(e.to_string()))?;
    let stage = Arc::new(stage);
    let result = run_in_stage(ctx, stage.clone(), kind, mc, java, &jar).await;
    let remove_jar = result.is_ok().then_some(jar);
    let _ = tokio::task::spawn_blocking(move || {
        let _ = stage.cleanup_dir(); // the staging tree never outlives the attempt
        if let Some(j) = remove_jar {
            let _ = remove_file_with_retry(&j);
        }
    })
    .await;
    result
}

async fn run_in_stage<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    stage: Arc<StageSession>,
    kind: LoaderKind,
    mc: &McVersion,
    java: &ManagedJava,
    jar: &Path,
) -> Result<VersionId, PrepareError> {
    let (cmd, tripwire) = {
        let root = ctx.root.clone();
        let stage_ref = stage.clone();
        let mc = mc.clone();
        let java = java.clone();
        let jar = jar.to_path_buf();
        let sys = ctx.sys.clone();
        tokio::task::spawn_blocking(
            move || -> Result<(InstallerCommand, Tripwire), PrepareError> {
                let loader_err =
                    |e: crate::loader_install::InstallError| PrepareError::Loader(e.to_string());
                stage_ref.seed_vanilla(&root, &mc).map_err(loader_err)?;
                let cmd = build_installer_command(&root, &stage_ref, &java, &jar, kind, &sys)
                    .map_err(loader_err)?;
                verify_command(&cmd, &root, &sys).map_err(loader_err)?; // again, right before spawning
                let tripwire = Tripwire::arm(&Tripwire::global_minecraft_dirs());
                Ok((cmd, tripwire))
            },
        )
        .await
        .map_err(|e| PrepareError::Storage(e.to_string()))??
    };

    ctx.tracker.enter(
        Stage::Loader,
        format!("Installing {}", kind.name()),
        None,
        true,
    );
    let run = ctx.runner.run(&cmd, &ctx.cancel).await;
    let outcome = match run {
        Ok(o) => o,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(PrepareError::Cancelled),
        Err(e) => {
            return Err(PrepareError::Loader(format!(
                "the {} installer could not run: {e}",
                kind.name()
            )))
        }
    };

    let root = ctx.root.clone();
    let mc = mc.clone();
    tokio::task::spawn_blocking(move || -> Result<VersionId, PrepareError> {
        let loader_err =
            |e: crate::loader_install::InstallError| PrepareError::Loader(e.to_string());
        tripwire.check().map_err(loader_err)?;
        if !outcome.success {
            return Err(PrepareError::Loader(format!(
                "the {} installer did not finish ({})",
                kind.name(),
                outcome.tail.chars().take(300).collect::<String>()
            )));
        }

        let ids = new_version_ids(&stage, &mc)?;
        let [id] = ids.as_slice() else {
            return Err(PrepareError::Loader(format!(
                "expected the installer to produce one version, found {}",
                ids.len()
            )));
        };

        // Validate the staged version JSON before merging anything into the shared stores.
        let staged_json = stage
            .minecraft_dir()
            .join("versions")
            .join(id.as_str())
            .join(format!("{id}.json"));
        if is_link(&staged_json).unwrap_or(true) {
            return Err(PrepareError::Loader(
                "the installer did not produce a valid version profile".into(),
            ));
        }
        let staged_bytes = read_bounded(&staged_json, MAX_PROFILE_BYTES as u64).map_err(|_| {
            PrepareError::Loader("the installer did not produce a readable version profile".into())
        })?;
        let produced = parse_version(&staged_bytes, &WINDOWS_X64).map_err(|e| {
            PrepareError::Loader(format!(
                "the {} profile produced by the installer is not usable: {e}",
                kind.name()
            ))
        })?;
        if produced.id != id.as_str() {
            return Err(PrepareError::Loader(
                "the installer's version profile id does not match its folder".into(),
            ));
        }
        if produced.inherits_from.as_deref() != Some(mc.as_str()) {
            return Err(PrepareError::Loader(
                "the installer produced a profile for a different Minecraft version".into(),
            ));
        }

        merge_stage_into_shared(&stage, &root).map_err(loader_err)?;
        Ok(id.clone())
    })
    .await
    .map_err(|e| PrepareError::Storage(e.to_string()))?
}

/// Parses the installed loader profile (for the final file checks).
pub fn read_profile(root: &PortableRoot, id: &VersionId) -> Result<VersionPlan, PrepareError> {
    let path = profile_path(root, id);
    reject_links_between(root.path(), &path).map_err(|e| PrepareError::Storage(e.to_string()))?;
    let bytes = read_bounded(&path, MAX_PROFILE_BYTES as u64)
        .map_err(|_| PrepareError::Loader("the loader's version profile is missing".into()))?;
    parse_version(&bytes, &WINDOWS_X64).map_err(|e| {
        PrepareError::Loader(format!("the loader's version profile is not usable: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::Progress;
    use crate::instances::InstanceId;
    use crate::loader_install::SystemEnv;
    use crate::net::testing::FakeFetcher;
    use crate::net::testing_stream::FakeStreamer;
    use crate::prepare::Tracker;
    use serde_json::json;
    use sha1::{Digest as _, Sha1};
    use sha2::Sha256;

    fn sha1_hex(b: &[u8]) -> String {
        hex::encode(Sha1::digest(b))
    }

    fn sha256_hex_bytes(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    struct ScriptedRunner {
        #[allow(clippy::type_complexity)]
        on_run:
            Mutex<Option<Box<dyn Fn(&InstallerCommand) -> io::Result<RunResult> + Send + Sync>>>,
    }

    impl InstallerRunner for ScriptedRunner {
        fn run<'a>(
            &'a self,
            cmd: &'a InstallerCommand,
            _cancel: &'a Cancel,
        ) -> impl Future<Output = io::Result<RunResult>> + Send + 'a {
            let res = match &*self.on_run.lock().unwrap() {
                Some(f) => f(cmd),
                None => Err(io::Error::other("not scripted")),
            };
            async move { res }
        }
    }

    fn test_tracker() -> Tracker {
        Tracker::new(
            &InstanceId::new("test").unwrap(),
            Progress::new(),
            Box::new(|_| {}),
        )
    }

    #[test]
    fn checksums_are_read_from_the_first_token_only() {
        let h = "f392445f5f5c4523d5ba8f6481c14e83a6167d7ea16438d92ea7bfcd5a0e125a";
        assert_eq!(first_hex_token(h.as_bytes(), 64).as_deref(), Some(h));
        assert_eq!(
            first_hex_token(format!("{h}  installer.jar\n").as_bytes(), 64).as_deref(),
            Some(h)
        );
        assert_eq!(
            first_hex_token(h.to_uppercase().as_bytes(), 64).as_deref(),
            Some(h)
        );
        assert!(first_hex_token(b"<html>not found</html>", 64).is_none());
        assert!(first_hex_token(b"abcd", 64).is_none());
        assert!(first_hex_token(b"", 64).is_none());
    }

    #[tokio::test]
    async fn plan_profile_loader_honours_verify_all_and_offline_fallback() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        let fetcher = FakeFetcher::new();
        let streamer = FakeStreamer::new();
        let runner = ScriptedRunner {
            on_run: Mutex::new(None),
        };
        let tracker = test_tracker();
        let mc = McVersion::new("1.21.4").unwrap();
        let lv = LoaderVersion::new("0.16.9").unwrap();
        let expected = VersionId::new("fabric-loader-0.16.9-1.21.4").unwrap();

        let local_json = json!({
            "id": expected.as_str(),
            "inheritsFrom": "1.21.4",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": []
        })
        .to_string();
        let local_path = profile_path(&root, &expected);
        fs::create_dir_all(local_path.parent().unwrap()).unwrap();
        fs::write(&local_path, &local_json).unwrap();

        let make_ctx = |verify_all: bool| Ctx {
            root: &root,
            meta: &fetcher,
            loader_meta: &fetcher,
            streamer: &streamer,
            loader_streamer: &streamer,
            runner: &runner,
            sys: SystemEnv::from_process(),
            concurrency: 2,
            now: 1,
            backoff: Duration::ZERO,
            cancel: Cancel::new(),
            tracker: &tracker,
            verify_all,
            free_space: Arc::new(|_| None),
        };

        // 1. Ordinary run (`!verify_all`): reuses valid local profile with no network call.
        let planned = plan_profile_loader(&make_ctx(false), LoaderKind::Fabric, &mc, &lv)
            .await
            .unwrap();
        assert_eq!(planned.bytes, local_json.as_bytes());
        assert!(fetcher.calls().is_empty());

        // 2. `verify_all = true` while online: fetches fresh profile bytes from official host.
        let fresh_json = json!({
            "id": expected.as_str(),
            "inheritsFrom": "1.21.4",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": [], "jvm": ["-DFresh=1"]},
            "libraries": []
        })
        .to_string();
        let url = "https://meta.fabricmc.net/v2/versions/loader/1.21.4/0.16.9/profile/json";
        fetcher.ok(url, &fresh_json);
        let refreshed = plan_profile_loader(&make_ctx(true), LoaderKind::Fabric, &mc, &lv)
            .await
            .unwrap();
        assert_eq!(refreshed.bytes, fresh_json.as_bytes());
        assert_eq!(fetcher.calls(), vec![url.to_owned()]);

        // 3. `verify_all = true` while offline: falls back to valid local profile.
        fetcher.fail_all();
        let offline_fallback = plan_profile_loader(&make_ctx(true), LoaderKind::Fabric, &mc, &lv)
            .await
            .unwrap();
        assert_eq!(offline_fallback.bytes, local_json.as_bytes());

        // 4. Local profile with mismatched `inheritsFrom` is never reused.
        fs::write(
            &local_path,
            json!({"id": expected.as_str(), "inheritsFrom": "1.20.1", "libraries": []}).to_string(),
        )
        .unwrap();
        assert!(
            plan_profile_loader(&make_ctx(false), LoaderKind::Fabric, &mc, &lv)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn installer_validates_staged_profile_before_merge_and_removes_installer_jar() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        let vdir = root.minecraft_dir().join("versions").join("1.21.4");
        fs::create_dir_all(&vdir).unwrap();
        fs::write(vdir.join("1.21.4.jar"), b"client").unwrap();
        fs::write(vdir.join("1.21.4.json"), b"{}").unwrap();

        let java_home = root.java_runtimes_dir().join("jre-test");
        fs::create_dir_all(java_home.join("bin")).unwrap();
        let java_exe = java_home.join("bin").join("java.exe");
        fs::write(&java_exe, b"MZ").unwrap();
        let java = ManagedJava {
            id: "jre-test".into(),
            java_major: 21,
            exe: java_exe,
            home: java_home,
        };

        let installer_bytes = b"fake-forge-installer";
        let jar_url = "https://maven.minecraftforge.net/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-installer.jar";
        let fetcher = FakeFetcher::new();
        fetcher.ok(
            &format!("{jar_url}.sha256"),
            sha256_hex_bytes(installer_bytes),
        );
        let streamer = FakeStreamer::new();
        streamer.add(jar_url, installer_bytes);

        let runner = ScriptedRunner {
            on_run: Mutex::new(Some(Box::new(|cmd| {
                // Bad installer output: inheritsFrom does not match 1.21.4.
                let target = Path::new(cmd.args.last().unwrap());
                let out_v = target.join("versions/1.21.4-forge-54.1.14");
                fs::create_dir_all(&out_v)?;
                fs::write(
                    out_v.join("1.21.4-forge-54.1.14.json"),
                    json!({
                        "id": "1.21.4-forge-54.1.14",
                        "inheritsFrom": "1.20.1",
                        "libraries": []
                    })
                    .to_string(),
                )?;
                let out_l = target.join("libraries/com/bad");
                fs::create_dir_all(&out_l)?;
                fs::write(out_l.join("bad.jar"), b"untrusted")?;
                Ok(RunResult {
                    success: true,
                    tail: String::new(),
                })
            }))),
        };
        let tracker = test_tracker();
        let ctx = Ctx {
            root: &root,
            meta: &fetcher,
            loader_meta: &fetcher,
            streamer: &streamer,
            loader_streamer: &streamer,
            runner: &runner,
            sys: SystemEnv {
                system_root: PathBuf::from(r"C:\Windows"),
            },
            concurrency: 2,
            now: 1,
            backoff: Duration::ZERO,
            cancel: Cancel::new(),
            tracker: &tracker,
            verify_all: false,
            free_space: Arc::new(|_| None),
        };
        let mc = McVersion::new("1.21.4").unwrap();
        let lv = LoaderVersion::new("54.1.14").unwrap();

        // Rejected BEFORE merging into shared stores!
        assert!(
            install_with_installer(&ctx, LoaderKind::Forge, &mc, &lv, &java)
                .await
                .is_err()
        );
        assert!(!root
            .minecraft_dir()
            .join("versions/1.21.4-forge-54.1.14")
            .exists());
        assert!(!root.libraries_dir().join("com/bad/bad.jar").exists());

        // Now script a valid profile: merges cleanly and deletes the installer jar from cache/downloads.
        *runner.on_run.lock().unwrap() = Some(Box::new(|cmd| {
            let target = Path::new(cmd.args.last().unwrap());
            let out_v = target.join("versions/1.21.4-forge-54.1.14");
            fs::create_dir_all(&out_v)?;
            fs::write(
                out_v.join("1.21.4-forge-54.1.14.json"),
                json!({
                    "id": "1.21.4-forge-54.1.14",
                    "inheritsFrom": "1.21.4",
                    "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
                    "libraries": []
                })
                .to_string(),
            )?;
            Ok(RunResult {
                success: true,
                tail: String::new(),
            })
        }));
        let id = install_with_installer(&ctx, LoaderKind::Forge, &mc, &lv, &java)
            .await
            .unwrap();
        assert_eq!(id.as_str(), "1.21.4-forge-54.1.14");
        assert!(
            !root
                .downloads_dir()
                .join("forge-1.21.4-54.1.14-installer.jar")
                .exists(),
            "installer jar is cleaned up from cache/downloads after success"
        );
    }

    #[test]
    fn find_reusable_installer_loader_discovers_forge_variants_and_verifies_generated_jars() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();

        // Legacy 1.12.2 Forge folder naming (`1.12.2-forge1.12.2-14.23.5.2860`) + generated jar.
        let mc = McVersion::new("1.12.2").unwrap();
        let lv = LoaderVersion::new("14.23.5.2860").unwrap();
        let forge_id = "1.12.2-forge1.12.2-14.23.5.2860";
        let gen_rel =
            "net/minecraftforge/forge/1.12.2-14.23.5.2860/forge-1.12.2-14.23.5.2860-client.jar";
        let gen_bytes = b"patched-1122-client";
        let vdir = root.minecraft_dir().join("versions").join(forge_id);
        fs::create_dir_all(&vdir).unwrap();
        fs::write(
            vdir.join(format!("{forge_id}.json")),
            json!({
                "id": forge_id,
                "inheritsFrom": "1.12.2",
                "mainClass": "net.minecraft.launchwrapper.Launch",
                "libraries": [{
                    "name": "net.minecraftforge:forge:1.12.2-14.23.5.2860:client",
                    "downloads": {"artifact": {
                        "path": gen_rel,
                        "url": "",
                        "sha1": sha1_hex(gen_bytes),
                        "size": gen_bytes.len()
                    }}
                }]
            })
            .to_string(),
        )
        .unwrap();
        let gen_path = root.libraries_dir().join(gen_rel);
        fs::create_dir_all(gen_path.parent().unwrap()).unwrap();
        fs::write(&gen_path, gen_bytes).unwrap();

        let (found_id, _) =
            find_reusable_installer_loader(&root, LoaderKind::Forge, &mc, &lv, None)
                .expect("valid Forge 1.12.2 install is reusable");
        assert_eq!(found_id.as_str(), forge_id);

        // Corrupt the generated jar (same size, flipped byte): must NOT be reused!
        let mut damaged = gen_bytes.to_vec();
        damaged[0] ^= 0xFF;
        fs::write(&gen_path, damaged).unwrap();
        assert!(
            find_reusable_installer_loader(&root, LoaderKind::Forge, &mc, &lv, None).is_none(),
            "corrupted generated jar forces fresh installer run"
        );
    }

    #[tokio::test]
    async fn job_object_terminates_grandchild_processes_on_cancel_and_timeout() {
        let d = tempfile::tempdir().unwrap();
        let sys = SystemEnv::from_process();
        let cmd_exe = sys.system_root.join("System32").join("cmd.exe");
        let sys_root = sys.system_root.display().to_string();
        let sys_path = format!(r"{sys_root}\System32");

        for use_cancel in [true, false] {
            let work = d.path().join(if use_cancel {
                "cancel-stage"
            } else {
                "timeout-stage"
            });
            fs::create_dir_all(&work).unwrap();
            let held = work.join("grandchild.lock");
            // Spawn a parent cmd.exe that launches a child cmd.exe + grandchild ping.exe holding
            // `grandchild.lock` and `work` open for 60s. No inner quotes so `cmd /c` parses cleanly.
            let icmd = InstallerCommand {
                program: cmd_exe.clone(),
                args: vec![
                    "/c".into(),
                    "cmd /c ping -n 60 127.0.0.1 > grandchild.lock".into(),
                ],
                cwd: work.clone(),
                env: std::collections::BTreeMap::from([
                    ("SystemRoot".to_owned(), sys_root.clone()),
                    ("windir".to_owned(), sys_root.clone()),
                    ("PATH".to_owned(), sys_path.clone()),
                ]),
            };
            let cancel = Cancel::new();
            if use_cancel {
                let c2 = cancel.clone();
                let held_check = held.clone();
                tokio::spawn(async move {
                    for _ in 0..100 {
                        if held_check.exists() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    c2.cancel();
                });
                let err = run_command(&icmd, &cancel, Duration::from_secs(30))
                    .await
                    .err()
                    .expect("cancelled");
                assert_eq!(err.kind(), io::ErrorKind::Interrupted);
            } else {
                let err = run_command(&icmd, &cancel, Duration::from_millis(400))
                    .await
                    .err()
                    .expect("timed out");
                assert_eq!(err.kind(), io::ErrorKind::TimedOut);
            }
            assert!(
                held.exists(),
                "grandchild actually started and opened the lock file"
            );
            // If the grandchild were still alive, removing `work` on Windows would fail with lock error.
            crate::portable::fsutil::remove_dir_all_with_retry(&work)
                .expect("job object terminated grandchild and released all handles");
            assert!(!work.exists());
        }
    }
}
