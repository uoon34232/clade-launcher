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
//!   every library the produced profile names is then verified (or, for files the installer
//!   generates locally, checked to exist).

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
use crate::net::{Fetcher, Streamer};
use crate::portable::fsutil::{atomic_write, Expected};
use crate::prepare::{fetch_err, jobs_for_libraries, Ctx, PrepareError, Stage};
use crate::runtimes::ManagedJava;
use crate::versions::{LoaderVersion, McVersion, VersionId};

/// An installer that runs longer than this is stopped.
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const MAX_PROFILE_BYTES: usize = 4 * 1024 * 1024;

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

impl InstallerRunner for ProcessRunner {
    #[allow(clippy::manual_async_fn)] // the trait spells out the borrowed lifetime
    fn run<'a>(
        &'a self,
        cmd: &'a InstallerCommand,
        cancel: &'a Cancel,
    ) -> impl Future<Output = io::Result<RunResult>> + Send + 'a {
        async move {
            // `to_process` is the single place installer processes are created (scrubbed env).
            let mut c = tokio::process::Command::from(cmd.to_process());
            c.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = c.spawn()?;
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
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                () = tokio::time::sleep(INSTALLER_TIMEOUT) => {
                    let _ = child.kill().await;
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the installer took too long"));
                }
            };
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

fn version_dir(ctx_root: &crate::portable::PortableRoot, id: &VersionId) -> PathBuf {
    ctx_root.minecraft_dir().join("versions").join(id.as_str())
}

fn profile_path(root: &crate::portable::PortableRoot, id: &VersionId) -> PathBuf {
    version_dir(root, id).join(format!("{id}.json"))
}

/// What a Fabric/Quilt installation will do, decided before anything is downloaded.
pub struct ProfilePlan {
    pub id: VersionId,
    pub bytes: Vec<u8>,
    pub plan: VersionPlan,
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
    let local = profile_path(ctx.root, &expected);
    let bytes = match fs::read(&local) {
        Ok(b) if parse_version(&b, &WINDOWS_X64).is_ok() => b,
        _ => {
            let InstallPlan::Profile { profile_url } = loaders::install_plan(kind, mc, loader)
                .map_err(|e| PrepareError::Loader(e.to_string()))?
            else {
                return Err(PrepareError::Loader("unexpected install plan".into()));
            };
            ctx.loader_meta
                .get(&profile_url, MAX_PROFILE_BYTES)
                .await
                .map_err(|e| fetch_err(e, &format!("the {} profile", kind.name())))?
        }
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
    let path = profile_path(ctx.root, &p.id);
    if fs::read(&path).ok().as_deref() != Some(&p.bytes) {
        atomic_write(&path, &p.bytes).map_err(|e| PrepareError::Storage(e.to_string()))?;
    }
    ensure_libraries(ctx, downloader, &p.plan).await
}

/// Libraries whose metadata publishes neither a size nor a hash (Quilt's, and Fabric's own jars)
/// can only be verified through the `.sha1` beside them, once. The size seen then is recorded here
/// so later checks can still notice a truncated file without any network.
fn sizes_path(root: &crate::portable::PortableRoot, id: &str) -> PathBuf {
    root.minecraft_dir()
        .join("versions")
        .join(id)
        .join(format!("{id}.sizes.json"))
}

pub fn read_sizes(
    root: &crate::portable::PortableRoot,
    id: &str,
) -> std::collections::HashMap<String, u64> {
    fs::read(sizes_path(root, id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_sizes(root: &crate::portable::PortableRoot, plan: &VersionPlan) {
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
        if let Ok(bytes) = serde_json::to_vec_pretty(&map) {
            let _ = atomic_write(&sizes_path(root, &plan.id), &bytes);
        }
    }
}

/// Downloads (or checks) every library a profile names.
pub async fn ensure_libraries<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    downloader: &Downloader<'_, S>,
    plan: &VersionPlan,
) -> Result<(), PrepareError> {
    let libs: Vec<_> = plan.libraries.iter().collect();
    let recorded = read_sizes(ctx.root, &plan.id);
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
    write_sizes(ctx.root, plan);
    for g in generated {
        if !g.is_file() {
            return Err(PrepareError::Loader(
                "the loader installer did not produce a file the profile needs".into(),
            ));
        }
    }
    Ok(())
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
    .with_backoff(ctx.backoff);
    installer_dl
        .fetch(&FileJob {
            url: installer_url,
            dest: jar.clone(),
            expected: Expected::Sha256(sha),
            size: None,
        })
        .await?;

    let stage = StageSession::create(ctx.root).map_err(|e| PrepareError::Loader(e.to_string()))?;
    let result = run_in_stage(ctx, &stage, kind, mc, java, &jar).await;
    let _ = stage.cleanup(); // the staging tree never outlives the attempt
    result
}

async fn run_in_stage<F: Fetcher, S: Streamer, R: InstallerRunner>(
    ctx: &Ctx<'_, F, S, R>,
    stage: &StageSession,
    kind: LoaderKind,
    mc: &McVersion,
    java: &ManagedJava,
    jar: &Path,
) -> Result<VersionId, PrepareError> {
    let loader_err = |e: crate::loader_install::InstallError| PrepareError::Loader(e.to_string());
    stage.seed_vanilla(ctx.root, mc).map_err(loader_err)?;
    let cmd =
        build_installer_command(ctx.root, stage, java, jar, kind, &ctx.sys).map_err(loader_err)?;
    verify_command(&cmd, ctx.root, &ctx.sys).map_err(loader_err)?; // again, right before spawning
    let tripwire = Tripwire::arm(&Tripwire::global_minecraft_dirs());

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
    tripwire.check().map_err(loader_err)?;
    if !outcome.success {
        return Err(PrepareError::Loader(format!(
            "the {} installer did not finish ({})",
            kind.name(),
            outcome.tail.chars().take(300).collect::<String>()
        )));
    }

    let ids = new_version_ids(stage, mc)?;
    let [id] = ids.as_slice() else {
        return Err(PrepareError::Loader(format!(
            "expected the installer to produce one version, found {}",
            ids.len()
        )));
    };
    merge_stage_into_shared(stage, ctx.root).map_err(loader_err)?;
    Ok(id.clone())
}

/// Parses the installed loader profile (for the final file checks).
pub fn read_profile(
    root: &crate::portable::PortableRoot,
    id: &VersionId,
) -> Result<VersionPlan, PrepareError> {
    let bytes = fs::read(profile_path(root, id))
        .map_err(|_| PrepareError::Loader("the loader's version profile is missing".into()))?;
    parse_version(&bytes, &WINDOWS_X64).map_err(|e| {
        PrepareError::Loader(format!("the loader's version profile is not usable: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
