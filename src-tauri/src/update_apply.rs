//! Verified, transactional self-updating for portable Clade installations.
//!
//! Never touches `data/`, `instances/`, `logs/`, `minecraft/`, `libraries/`, `assets/`, or
//! `runtimes/java/`. Every update ZIP is downloaded strictly from [`crate::download_policy::UPDATE_DOWNLOAD_HOSTS`],
//! verified against both `.sha256` and any GitHub API digest, unpacked into `cache/updates/`,
//! checked against a fail-closed package allowlist, and applied by the staged helper binary after
//! this launcher exits, with full rollback on failure.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::coded_error::{params, CodedError};
use crate::download::{Cancel, DownloadError, Downloader, FileJob, Progress};
use crate::launch_cmd::LaunchState;
use crate::net::{FetchError, Fetcher, Streamer};
use crate::portable::fsutil::{
    self, is_link, reject_links_between, remove_dir_all_with_retry, remove_file_with_retry,
    rename_with_retry, sha256_hex, strip_prefix_portable, verify_file, Expected,
};
use crate::portable::relpath::RelPath;
use crate::portable::{PortableRoot, RootLock};
use crate::prepare_cmd::PrepareState;
use crate::safe_archive::{extract_zip_into_cancellable, ExtractError, Limits};
use crate::startup;
use crate::startup_text::StartupMessage;
use crate::system;
use crate::update_check::{
    self, installed_version_tag, parse_portable_sha256, parse_release_tag, CheckedRelease,
    UpdateState, PORTABLE_SHA256_ASSET, PORTABLE_ZIP_ASSET,
};

pub const UPDATE_PROGRESS_EVENT: &str = "update-progress";

const PACKAGE_ROOT_DIR: &str = "Clade-Launcher";
const MAX_SHA256_BYTES: usize = 4 * 1024;
const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const PROGRESS_TICK: Duration = Duration::from_millis(120);
const WAIT_PID_TIMEOUT_MS: u32 = 20_000;
const LOCK_WAIT_ATTEMPTS: usize = 30;
const LOCK_WAIT_STEP: Duration = Duration::from_millis(200);
const WEBVIEW2_RETRY_ATTEMPTS: usize = 12;
const WEBVIEW2_RETRY_STEP: Duration = Duration::from_millis(500);

const REQUIRED_FILES: [&str; 5] = [
    "Clade.exe",
    "README.txt",
    "LICENSE",
    "THIRD-PARTY-NOTICES.txt",
    "licenses/WebView2-Fixed-Version-License.txt",
];
const TOP_LEVEL_FILES: [&str; 4] = [
    "Clade.exe",
    "README.txt",
    "LICENSE",
    "THIRD-PARTY-NOTICES.txt",
];
const FORBIDDEN_DIRS: [&str; 24] = [
    ".git",
    ".github",
    ".vscode",
    ".idea",
    "node_modules",
    "target",
    "test-results",
    "playwright-report",
    "screenshots",
    "shots",
    "e2e",
    "src",
    "src-tauri",
    "dist",
    "data",
    "instances",
    "logs",
    "cache",
    "minecraft",
    "libraries",
    "assets",
    "worlds",
    "saves",
    "mods",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStage {
    Idle,
    Downloading,
    Verifying,
    Extracting,
    Ready,
    Applying,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgressDto {
    pub stage: UpdateStage,
    pub version: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    pub version: String,
    pub stage_dir_name: String,
    pub exe_sha256: String,
    pub has_webview2: bool,
}

pub struct ApplyRuntime {
    inner: Mutex<RuntimeInner>,
}

struct RuntimeInner {
    stage: UpdateStage,
    starting: bool,
    version: Option<String>,
    bytes_done: u64,
    bytes_total: u64,
    cancel: Option<Arc<Cancel>>,
    staged: Option<StagedUpdate>,
}

impl Default for ApplyRuntime {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                stage: UpdateStage::Idle,
                starting: false,
                version: None,
                bytes_done: 0,
                bytes_total: 0,
                cancel: None,
                staged: None,
            }),
        }
    }
}

impl ApplyRuntime {
    pub fn snapshot(&self) -> UpdateProgressDto {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        UpdateProgressDto {
            stage: g.stage,
            version: g.version.clone(),
            bytes_done: g.bytes_done,
            bytes_total: g.bytes_total,
        }
    }

    pub fn cancel_active(&self) {
        let cancel = {
            let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            g.cancel.clone()
        };
        if let Some(c) = cancel {
            c.cancel();
        }
    }

    fn set_progress(&self, dto: &UpdateProgressDto) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.stage = dto.stage;
        g.version.clone_from(&dto.version);
        g.bytes_done = dto.bytes_done;
        g.bytes_total = dto.bytes_total;
    }

    fn reset_idle(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.stage = UpdateStage::Idle;
        g.starting = false;
        g.version = None;
        g.bytes_done = 0;
        g.bytes_total = 0;
        g.cancel = None;
        g.staged = None;
    }
}

fn portable_exe_name(root: &PortableRoot) -> Result<String, CodedError> {
    let exe = std::env::current_exe()
        .map_err(|e| CodedError::new("update_not_portable", e.to_string()))?;
    let exe_clean = crate::portable::root::normalize_verbatim_path(exe);
    if is_link(&exe_clean).unwrap_or(false) {
        return Err(CodedError::new(
            "update_not_portable",
            "launcher executable cannot be a link",
        ));
    }
    let rel = strip_prefix_portable(&exe_clean, root.path())
        .ok_or_else(|| CodedError::new("update_not_portable", "launcher exe is outside root"))?;
    let name = rel
        .to_str()
        .ok_or_else(|| CodedError::new("update_not_portable", "invalid executable path"))?;
    if !is_valid_exe_name(name) {
        return Err(CodedError::new(
            "update_not_portable",
            "launcher executable is not directly inside portable root",
        ));
    }
    Ok(name.to_owned())
}

fn is_valid_exe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', '/', ':'])
        && name.len() <= 80
        && name.to_ascii_lowercase().ends_with(".exe")
        && RelPath::new(name).is_ok()
}

fn random_hex(bytes: usize) -> Result<String, io::Error> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(hex::encode(buf))
}

fn stage_limits() -> Limits {
    Limits {
        max_entries: 10_000,
        max_file_bytes: 512 * MIB,
        max_total_bytes: 2 * GIB,
        max_ratio: 1000,
    }
}

fn is_forbidden_filename(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "vault.bin"
            | "vault.bin.bak"
            | "accounts.json"
            | "settings.json"
            | "clade.lock"
            | "claude.md"
            | "thumbs.db"
            | ".ds_store"
    ) {
        return true;
    }
    if lower == ".env" || lower.starts_with(".env.") {
        return true;
    }
    if (lower.starts_with("playtime") && lower.ends_with(".json"))
        || (lower.starts_with("diagnostics") && lower.ends_with(".zip"))
    {
        return true;
    }
    if lower.contains(".corrupt") {
        return true;
    }
    [
        ".log", ".pem", ".pfx", ".p12", ".key", ".bak", ".tmp", ".part", ".pdb", ".zip",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn check_allowed_relpath(rel: &str, is_dir: bool) -> Result<(), String> {
    if rel.is_empty() || rel.contains('\\') || rel.starts_with('/') {
        return Err(format!("unsafe package entry {rel:?}"));
    }
    let segments: Vec<&str> = rel.split('/').collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return Err(format!("invalid path component in {rel:?}"));
    }
    let in_runtime = rel == "runtimes/webview2" || rel.starts_with("runtimes/webview2/");
    let dir_count = if is_dir {
        segments.len()
    } else {
        segments.len() - 1
    };
    for dir in &segments[..dir_count] {
        let dl = dir.to_ascii_lowercase();
        if (FORBIDDEN_DIRS.contains(&dl.as_str()) && !(in_runtime && dl == "src"))
            || is_forbidden_filename(dir)
        {
            return Err(format!("forbidden folder in package: {rel}"));
        }
    }
    if is_dir {
        let allowed_dir = rel == "licenses"
            || rel == "runtimes"
            || rel == "runtimes/webview2"
            || rel.starts_with("runtimes/webview2/");
        if !allowed_dir {
            return Err(format!("unlisted directory in package: {rel}"));
        }
        return Ok(());
    }
    let file = segments[segments.len() - 1];
    if is_forbidden_filename(file) {
        return Err(format!("forbidden file in package: {rel}"));
    }
    if TOP_LEVEL_FILES.contains(&rel) {
        return Ok(());
    }
    if rel.starts_with("licenses/") && segments.len() == 2 {
        let fl = file.to_ascii_lowercase();
        if fl.ends_with(".txt") || fl.ends_with(".md") {
            return Ok(());
        }
        return Err(format!("invalid license file {rel}"));
    }
    if rel.starts_with("runtimes/webview2/") && segments.len() >= 3 {
        return Ok(());
    }
    Err(format!("file not on package allowlist: {rel}"))
}

fn walk_pkg_tree(dir: &Path, rel_prefix: &str, files: &mut HashSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if is_link(&path).unwrap_or(true) {
            return Err(format!("link not allowed in package: {}", path.display()));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 name in update package".to_owned())?;
        let rel = if rel_prefix.is_empty() {
            name
        } else {
            format!("{rel_prefix}/{name}")
        };
        let md = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if md.is_dir() {
            check_allowed_relpath(&rel, true)?;
            walk_pkg_tree(&path, &rel, files)?;
        } else if md.is_file() {
            check_allowed_relpath(&rel, false)?;
            files.insert(rel);
        } else {
            return Err(format!("unsupported filesystem entry in package: {rel}"));
        }
    }
    Ok(())
}

fn verify_pe_magic(exe: &Path) -> Result<(), String> {
    let md = fs::symlink_metadata(exe).map_err(|e| e.to_string())?;
    if !md.is_file() || md.len() < 64 {
        return Err("staged Clade.exe is missing or too small".into());
    }
    let mut f = File::open(exe).map_err(|e| e.to_string())?;
    let mut head = [0u8; 2];
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    if head != *b"MZ" {
        return Err("staged Clade.exe does not have a valid PE header".into());
    }
    Ok(())
}

/// Validates an unpacked stage directory strictly against the Clade portable package allowlist.
pub fn validate_staged_package(stage_dir: &Path) -> Result<StagedUpdate, CodedError> {
    let invalid = |msg: String| CodedError::new("update_invalid_package", msg);
    if is_link(stage_dir).unwrap_or(true) {
        return Err(invalid("stage directory cannot be a link".into()));
    }
    let mut top_entries = Vec::new();
    for entry in fs::read_dir(stage_dir).map_err(|e| invalid(e.to_string()))? {
        let entry = entry.map_err(|e| invalid(e.to_string()))?;
        top_entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    if top_entries.as_slice() != [PACKAGE_ROOT_DIR] {
        return Err(invalid(format!(
            "expected single top-level {PACKAGE_ROOT_DIR} directory, found {top_entries:?}"
        )));
    }
    let pkg_root = stage_dir.join(PACKAGE_ROOT_DIR);
    if is_link(&pkg_root).unwrap_or(true) || !pkg_root.is_dir() {
        return Err(invalid("package root is missing or invalid".into()));
    }
    let mut files = HashSet::new();
    walk_pkg_tree(&pkg_root, "", &mut files).map_err(invalid)?;
    for req in REQUIRED_FILES {
        if !files.contains(req) {
            return Err(invalid(format!("required package entry missing: {req}")));
        }
    }
    let exe_path = pkg_root.join("Clade.exe");
    verify_pe_magic(&exe_path).map_err(invalid)?;
    let exe_sha256 = sha256_hex(&exe_path).map_err(|e| invalid(e.to_string()))?;
    let has_webview2 = pkg_root
        .join("runtimes")
        .join("webview2")
        .join("msedgewebview2.exe")
        .is_file();
    let stage_dir_name = stage_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("invalid stage folder name".into()))?
        .to_owned();
    Ok(StagedUpdate {
        version: String::new(),
        stage_dir_name,
        exe_sha256,
        has_webview2,
    })
}

fn dir_top_signature(dir: &Path) -> io::Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let is_dir = fs::symlink_metadata(entry.path())?.is_dir();
        out.push((name, is_dir));
    }
    out.sort();
    Ok(out)
}

/// Returns `true` when the bundled WebView2 runtime in `staged_wv2` differs from `target_wv2` and
/// should be swapped. Unchanged ~300 MB WebView2 folders are kept in place cleanly.
pub fn webview2_needs_replace(target_wv2: &Path, staged_wv2: &Path) -> io::Result<bool> {
    let staged_exe = staged_wv2.join("msedgewebview2.exe");
    if !staged_exe.is_file() {
        return Ok(false);
    }
    let target_exe = target_wv2.join("msedgewebview2.exe");
    if !target_exe.is_file() {
        return Ok(true);
    }
    if sha256_hex(&target_exe)? != sha256_hex(&staged_exe)? {
        return Ok(true);
    }
    Ok(dir_top_signature(target_wv2)? != dir_top_signature(staged_wv2)?)
}

fn emit_progress(on_progress: &mut impl FnMut(UpdateProgressDto), dto: UpdateProgressDto) {
    on_progress(dto);
}

pub async fn download_and_stage_with<F: Fetcher, S: Streamer>(
    root: &PortableRoot,
    checked: &CheckedRelease,
    sha_fetcher: &F,
    zip_streamer: &S,
    cancel: Arc<Cancel>,
    mut on_progress: impl FnMut(UpdateProgressDto),
) -> Result<StagedUpdate, CodedError> {
    let tag = checked.tag.clone();
    let sha_url = update_check::release_asset_url(&tag, PORTABLE_SHA256_ASSET)?;
    let zip_url = update_check::release_asset_url(&tag, PORTABLE_ZIP_ASSET)?;

    let emit_stage = |cb: &mut _, stage, done, total| {
        emit_progress(
            cb,
            UpdateProgressDto {
                stage,
                version: Some(tag.clone()),
                bytes_done: done,
                bytes_total: total,
            },
        );
    };

    let zip_est = checked.zip_size.unwrap_or(350 * MIB);
    emit_stage(&mut on_progress, UpdateStage::Downloading, 0, zip_est);

    if cancel.is_cancelled() {
        return Err(CodedError::new("update_cancelled", "update cancelled"));
    }

    let sha_bytes = sha_fetcher
        .get(&sha_url, MAX_SHA256_BYTES)
        .await
        .map_err(|e| match e {
            FetchError::Network(m) => CodedError::new("offline", m),
            FetchError::Status(404) => CodedError::new("update_unavailable", "sha256 missing"),
            other => CodedError::new("update_check_failed", other.to_string()),
        })?;
    let sha_text = std::str::from_utf8(&sha_bytes)
        .map_err(|e| CodedError::new("update_verify_failed", e.to_string()))?;
    let expected_hex = parse_portable_sha256(sha_text)?;
    if let Some(api_hex) = &checked.api_sha256 {
        if !api_hex.eq_ignore_ascii_case(&expected_hex) {
            return Err(CodedError::new(
                "update_verify_failed",
                "release digest disagrees with sha256 file",
            ));
        }
    }

    if cancel.is_cancelled() {
        return Err(CodedError::new("update_cancelled", "update cancelled"));
    }

    let required = zip_est.saturating_mul(3).saturating_add(256 * MIB);
    if let Some(avail) = system::free_disk_bytes(root.path()) {
        if avail < required {
            return Err(CodedError::new(
                "not_enough_space",
                format!("need {required} bytes, only {avail} available"),
            )
            .with(params([
                ("requiredBytes", serde_json::json!(required)),
                ("availableBytes", serde_json::json!(avail)),
            ])));
        }
    }

    let ver_dir = root.updates_dir().join(&tag);
    reject_links_between(root.path(), &ver_dir)
        .map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;
    fs::create_dir_all(&ver_dir)
        .map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;
    reject_links_between(root.path(), &ver_dir)
        .map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;
    let zip_dest = ver_dir.join(PORTABLE_ZIP_ASSET);

    let progress = Progress::new();
    progress.plan_bytes(zip_est);
    let dl = Downloader::new(zip_streamer, cancel.clone(), progress.clone())
        .with_root(root.path().to_path_buf());
    let job = FileJob {
        url: zip_url,
        dest: zip_dest.clone(),
        expected: Expected::Sha256(expected_hex.clone()),
        size: checked.zip_size,
    };

    let dl_res = {
        let fut = dl.fetch(&job);
        tokio::pin!(fut);
        let mut tick = tokio::time::interval(PROGRESS_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                res = &mut fut => break res,
                _ = tick.tick() => {
                    let snap = progress.snapshot();
                    let total = if snap.bytes_total > 0 { snap.bytes_total } else { zip_est };
                    let done = if total > 0 { snap.bytes_done.min(total) } else { snap.bytes_done };
                    emit_stage(&mut on_progress, UpdateStage::Downloading, done, total);
                }
            }
        }
    };
    dl_res.map_err(|e| match e {
        DownloadError::Cancelled => CodedError::new("update_cancelled", "update cancelled"),
        DownloadError::Offline(m) => CodedError::new("offline", m),
        DownloadError::Status(404) => CodedError::new("update_unavailable", "archive missing"),
        DownloadError::Verification(m) => CodedError::new("update_verify_failed", m),
        DownloadError::Policy(m) | DownloadError::Local(m) => {
            if cancel.is_cancelled() {
                CodedError::new("update_cancelled", "update cancelled")
            } else {
                CodedError::new("update_install_failed", m)
            }
        }
        DownloadError::Status(status) => CodedError::new(
            "update_install_failed",
            format!("download server returned status {status}"),
        ),
    })?;

    if cancel.is_cancelled() {
        return Err(CodedError::new("update_cancelled", "update cancelled"));
    }

    emit_stage(&mut on_progress, UpdateStage::Verifying, zip_est, zip_est);
    let expected_clone = expected_hex.clone();
    let zip_verify_path = zip_dest.clone();
    let expected_size = checked.zip_size;
    tokio::task::spawn_blocking(move || {
        verify_file(
            &zip_verify_path,
            &Expected::Sha256(expected_clone),
            expected_size,
        )
    })
    .await
    .map_err(|e| CodedError::new("update_verify_failed", e.to_string()))?
    .map_err(|e| CodedError::new("update_verify_failed", e.to_string()))?;

    if cancel.is_cancelled() {
        return Err(CodedError::new("update_cancelled", "update cancelled"));
    }

    emit_stage(&mut on_progress, UpdateStage::Extracting, 0, 0);
    let nonce =
        random_hex(6).map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;
    let stage_dir = root.updates_dir().join(format!("stage-{nonce}"));
    let updates_root = root.path().to_path_buf();
    let zip_read_path = zip_dest;
    let stage_dir_task = stage_dir.clone();
    let cancel_extract = cancel.clone();
    let staged_res = tokio::task::spawn_blocking(move || -> Result<StagedUpdate, CodedError> {
        reject_links_between(&updates_root, &stage_dir_task)
            .map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;
        fs::create_dir_all(&stage_dir_task)
            .map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;
        let file = File::open(&zip_read_path)
            .map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;
        extract_zip_into_cancellable(file, &stage_dir_task, stage_limits(), &|| {
            cancel_extract.is_cancelled()
        })
        .map_err(|e| match e {
            ExtractError::Cancelled => CodedError::new("update_cancelled", "update cancelled"),
            ExtractError::Io(ioe) => CodedError::new("update_install_failed", ioe.to_string()),
            other => CodedError::new("update_invalid_package", other.to_string()),
        })?;
        if cancel_extract.is_cancelled() {
            return Err(CodedError::new("update_cancelled", "update cancelled"));
        }
        validate_staged_package(&stage_dir_task)
    })
    .await
    .map_err(|e| CodedError::new("update_install_failed", e.to_string()))?;

    let mut staged = match staged_res {
        Ok(_s) if cancel.is_cancelled() => {
            let _ = remove_dir_all_with_retry(&stage_dir);
            return Err(CodedError::new("update_cancelled", "update cancelled"));
        }
        Ok(s) => s,
        Err(e) => {
            let _ = remove_dir_all_with_retry(&stage_dir);
            return Err(e);
        }
    };
    staged.version = tag;
    Ok(staged)
}

enum SwapAction {
    ReplacedFile { target: PathBuf, backup: PathBuf },
    CreatedFile { target: PathBuf },
    ReplacedDir { target: PathBuf, backup: PathBuf },
    CreatedDir { target: PathBuf },
}

fn rollback_actions(actions: Vec<SwapAction>) -> bool {
    let mut all_ok = true;
    for action in actions.into_iter().rev() {
        match action {
            SwapAction::ReplacedFile { target, backup } => {
                if let Err(e) = rename_with_retry(&backup, &target) {
                    tracing::warn!(target = %target.display(), error = %e, "update rollback failed restoring file");
                    all_ok = false;
                }
            }
            SwapAction::CreatedFile { target } => {
                if fs::symlink_metadata(&target).is_ok() {
                    if let Err(e) = remove_file_with_retry(&target) {
                        tracing::warn!(target = %target.display(), error = %e, "update rollback failed removing created file");
                        all_ok = false;
                    }
                }
            }
            SwapAction::ReplacedDir { target, backup } => {
                if fs::symlink_metadata(&target).is_ok() {
                    if let Err(e) = remove_dir_all_with_retry(&target) {
                        tracing::warn!(target = %target.display(), error = %e, "update rollback failed clearing replaced dir");
                        all_ok = false;
                        continue;
                    }
                }
                if let Err(e) = rename_dir_with_lock_wait(&backup, &target) {
                    tracing::warn!(target = %target.display(), error = %e, "update rollback failed restoring dir");
                    all_ok = false;
                }
            }
            SwapAction::CreatedDir { target } => {
                if fs::symlink_metadata(&target).is_ok() {
                    if let Err(e) = remove_dir_all_with_retry(&target) {
                        tracing::warn!(target = %target.display(), error = %e, "update rollback failed removing created dir");
                        all_ok = false;
                    }
                }
            }
        }
    }
    all_ok
}

fn copy_synced(src: &Path, dst: &Path) -> io::Result<()> {
    let mut r = File::open(src)?;
    let mut w = OpenOptions::new().write(true).create_new(true).open(dst)?;
    io::copy(&mut r, &mut w)?;
    w.sync_all()?;
    Ok(())
}

fn swap_file_with_backup(
    src: &Path,
    dst: &Path,
    backup: &Path,
    tmp: &Path,
    expected_sha256: Option<&str>,
    actions: &mut Vec<SwapAction>,
) -> Result<(), String> {
    if is_link(dst).unwrap_or(false) {
        return Err(format!("refusing to replace link at {}", dst.display()));
    }
    copy_synced(src, tmp).map_err(|e| format!("staging {}: {e}", dst.display()))?;
    if let Some(hex) = expected_sha256 {
        let actual = sha256_hex(tmp).map_err(|e| e.to_string())?;
        if !actual.eq_ignore_ascii_case(hex) {
            let _ = remove_file_with_retry(tmp);
            return Err("copied executable failed checksum verification".into());
        }
    }
    let existed = fs::symlink_metadata(dst).is_ok();
    if existed {
        if let Some(bp) = backup.parent() {
            fs::create_dir_all(bp).map_err(|e| e.to_string())?;
        }
        rename_with_retry(dst, backup).map_err(|e| {
            let _ = remove_file_with_retry(tmp);
            format!("backing up {}: {e}", dst.display())
        })?;
        actions.push(SwapAction::ReplacedFile {
            target: dst.to_path_buf(),
            backup: backup.to_path_buf(),
        });
    }
    if let Err(e) = rename_with_retry(tmp, dst) {
        let _ = remove_file_with_retry(tmp);
        return Err(format!("installing {}: {e}", dst.display()));
    }
    if !existed {
        actions.push(SwapAction::CreatedFile {
            target: dst.to_path_buf(),
        });
    }
    Ok(())
}

fn rename_dir_with_lock_wait(src: &Path, dst: &Path) -> io::Result<()> {
    for attempt in 0..=WEBVIEW2_RETRY_ATTEMPTS {
        match rename_with_retry(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) if fsutil::is_transient_win_lock(&e) && attempt < WEBVIEW2_RETRY_ATTEMPTS => {
                std::thread::sleep(WEBVIEW2_RETRY_STEP);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Atomically swaps `Clade.exe`, documentation/licenses, and changed WebView2 runtime into
/// `target_root`, rolling back completely on any error. Never touches user data folders.
pub fn apply_update_transaction(
    target_root: &PortableRoot,
    target_exe_name: &str,
    staged_pkg_dir: &Path,
    expected_exe_sha256: &str,
) -> Result<(), String> {
    if !is_valid_exe_name(target_exe_name) {
        return Err("invalid target executable name".into());
    }
    let staged_exe = staged_pkg_dir.join("Clade.exe");
    verify_pe_magic(&staged_exe)?;
    let staged_hex = sha256_hex(&staged_exe).map_err(|e| e.to_string())?;
    if !staged_hex.eq_ignore_ascii_case(expected_exe_sha256) {
        return Err("staged executable digest mismatch".into());
    }
    reject_links_between(target_root.path(), staged_pkg_dir).map_err(|e| e.to_string())?;

    let nonce = random_hex(6).map_err(|e| e.to_string())?;
    let backup_dir = target_root.updates_dir().join(format!("backup-{nonce}"));
    fs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
    let mut actions: Vec<SwapAction> = Vec::new();

    let result = (|| -> Result<(), String> {
        let exe_dst = target_root.path().join(target_exe_name);
        let exe_backup = backup_dir.join(target_exe_name);
        let exe_tmp = target_root
            .path()
            .join(format!("{target_exe_name}.new-{nonce}.tmp"));
        swap_file_with_backup(
            &staged_exe,
            &exe_dst,
            &exe_backup,
            &exe_tmp,
            Some(expected_exe_sha256),
            &mut actions,
        )?;

        for name in ["README.txt", "LICENSE", "THIRD-PARTY-NOTICES.txt"] {
            let src = staged_pkg_dir.join(name);
            let dst = target_root.path().join(name);
            let backup = backup_dir.join(name);
            let tmp = target_root.path().join(format!("{name}.new-{nonce}.tmp"));
            swap_file_with_backup(&src, &dst, &backup, &tmp, None, &mut actions)?;
        }

        let staged_licenses = staged_pkg_dir.join("licenses");
        let target_licenses = target_root.path().join("licenses");
        if is_link(&target_licenses).unwrap_or(false) {
            return Err("licenses folder cannot be a link".into());
        }
        fs::create_dir_all(&target_licenses).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(&staged_licenses).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .is_file()
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let dst = target_licenses.join(&name);
            let backup = backup_dir.join("licenses").join(&name);
            let tmp = target_licenses.join(format!("{name}.new-{nonce}.tmp"));
            swap_file_with_backup(&path, &dst, &backup, &tmp, None, &mut actions)?;
        }

        let staged_wv2 = staged_pkg_dir.join("runtimes").join("webview2");
        let target_wv2 = target_root.fixed_webview2_dir();
        if webview2_needs_replace(&target_wv2, &staged_wv2).map_err(|e| e.to_string())? {
            let runtimes = target_root.runtimes_dir();
            if is_link(&runtimes).unwrap_or(false) || is_link(&target_wv2).unwrap_or(false) {
                return Err("runtimes folder cannot be a link".into());
            }
            fs::create_dir_all(&runtimes).map_err(|e| e.to_string())?;
            let existed = fs::symlink_metadata(&target_wv2).is_ok();
            let wv2_backup = backup_dir.join("webview2");
            if existed {
                rename_dir_with_lock_wait(&target_wv2, &wv2_backup)
                    .map_err(|e| format!("replacing WebView2 runtime: {e}"))?;
                actions.push(SwapAction::ReplacedDir {
                    target: target_wv2.clone(),
                    backup: wv2_backup,
                });
            }
            rename_dir_with_lock_wait(&staged_wv2, &target_wv2)
                .map_err(|e| format!("moving staged WebView2 runtime: {e}"))?;
            if !existed {
                actions.push(SwapAction::CreatedDir { target: target_wv2 });
            }
        }
        Ok(())
    })();

    if let Err(err) = result {
        if rollback_actions(actions) {
            let _ = remove_dir_all_with_retry(&backup_dir);
        } else {
            tracing::warn!(backup = %backup_dir.display(), "preserving update backup after incomplete rollback");
        }
        return Err(err);
    }
    let _ = remove_dir_all_with_retry(&backup_dir);
    Ok(())
}

fn spawn_clade_exe(exe: &Path, cwd: &Path, args: &[&str]) -> io::Result<()> {
    // portability-allow: spawns only the verified staged update helper or relaunches the updated Clade executable inside the portable folder
    let mut cmd = std::process::Command::new(exe);
    cmd.current_dir(cwd)
        .args(args)
        .env_remove("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER")
        .env_remove("WEBVIEW2_USER_DATA_FOLDER")
        .env_remove("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS");
    cmd.spawn()?;
    Ok(())
}

fn wait_for_parent_exit(pid: u32) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    if pid == 0 || pid == std::process::id() {
        return;
    }
    // SAFETY: `OpenProcess` with `PROCESS_SYNCHRONIZE` returns a handle we own and close on drop.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        return;
    }
    // SAFETY: `raw` is a fresh valid handle returned by `OpenProcess`.
    let owned = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    // SAFETY: waits boundedly until the launcher process exits.
    unsafe {
        WaitForSingleObject(owned.as_raw_handle().cast(), WAIT_PID_TIMEOUT_MS);
    }
}

fn acquire_root_lock_with_wait(root: &PortableRoot) -> Result<RootLock, String> {
    for _ in 0..LOCK_WAIT_ATTEMPTS {
        match RootLock::acquire(root) {
            Ok(lock) => return Ok(lock),
            Err(crate::portable::root::RootError::AlreadyRunning) => {
                std::thread::sleep(LOCK_WAIT_STEP);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("timed out waiting for previous launcher process to release portable lock".into())
}

fn append_helper_log(root: &PortableRoot, msg: &str) {
    let safe = startup::redact_detail(msg, Some(root.path()));
    let path = root.logs_dir().join("clade.log");
    let _ = fs::create_dir_all(root.logs_dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "WARN update helper failed: {safe}");
    }
}

fn comparable_path_key(path: &Path) -> String {
    let resolved = fs::canonicalize(path)
        .unwrap_or_else(|_| crate::portable::root::normalize_verbatim_path(path.to_path_buf()));
    let raw = resolved.to_string_lossy();
    let stripped = if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        raw.into_owned()
    };
    stripped.to_lowercase()
}

fn paths_equal_identity(a: &Path, b: &Path) -> bool {
    comparable_path_key(a) == comparable_path_key(b)
}

fn parse_flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut i = 2usize;
    while i + 1 < args.len() {
        if args[i] == flag {
            return Some(&args[i + 1]);
        }
        i += 2;
    }
    None
}

/// Runs the standalone update swap helper when launched as `<stage>/Clade-Launcher/Clade.exe --apply-update ...`.
/// Returns `Some(exit_code)` when helper mode ran so callers exit immediately without starting UI.
pub fn maybe_run_helper_mode() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--apply-update") {
        return None;
    }

    let (
        Some(raw_root),
        Some(target_exe),
        Some(wait_pid),
        Some(stage_dir_name),
        Some(expected_sha256),
    ) = (
        parse_flag_value(&args, "--target-root"),
        parse_flag_value(&args, "--target-exe"),
        parse_flag_value(&args, "--wait-pid").and_then(|s| s.parse::<u32>().ok()),
        parse_flag_value(&args, "--stage-dir"),
        parse_flag_value(&args, "--exe-sha256"),
    )
    else {
        return Some(1);
    };

    if !is_valid_exe_name(target_exe)
        || !stage_dir_name.starts_with("stage-")
        || RelPath::new(stage_dir_name).is_err()
        || expected_sha256.len() != 64
        || !expected_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Some(1);
    }

    let target_root = PortableRoot::at(PathBuf::from(raw_root));
    let expected_self = target_root
        .updates_dir()
        .join(stage_dir_name)
        .join(PACKAGE_ROOT_DIR)
        .join("Clade.exe");
    let Ok(cur_exe) = std::env::current_exe() else {
        startup::show_message(
            StartupMessage::UpdateFailed,
            None,
            Some("could not locate update helper executable"),
        );
        return Some(1);
    };
    if !paths_equal_identity(&cur_exe, &expected_self) {
        startup::show_message(
            StartupMessage::UpdateFailed,
            None,
            Some("update helper executable path mismatch"),
        );
        return Some(1);
    }

    wait_for_parent_exit(wait_pid);
    let lock = match acquire_root_lock_with_wait(&target_root) {
        Ok(l) => l,
        Err(e) => {
            append_helper_log(&target_root, &e);
            startup::show_message(
                StartupMessage::UpdateFailed,
                Some(target_root.path()),
                Some(&e),
            );
            return Some(1);
        }
    };

    let staged_pkg = target_root
        .updates_dir()
        .join(stage_dir_name)
        .join(PACKAGE_ROOT_DIR);
    let apply_res =
        apply_update_transaction(&target_root, target_exe, &staged_pkg, expected_sha256);
    drop(lock);

    let target_exe_path = target_root.path().join(target_exe);
    match apply_res {
        Ok(()) => {
            if let Err(e) = spawn_clade_exe(&target_exe_path, target_root.path(), &[]) {
                append_helper_log(&target_root, &e.to_string());
                startup::show_message(
                    StartupMessage::UpdateFailed,
                    Some(target_root.path()),
                    Some(&e.to_string()),
                );
                return Some(1);
            }
            Some(0)
        }
        Err(err) => {
            append_helper_log(&target_root, &err);
            startup::show_message(
                StartupMessage::UpdateFailed,
                Some(target_root.path()),
                Some(&err),
            );
            if target_exe_path.is_file() {
                let _ = spawn_clade_exe(&target_exe_path, target_root.path(), &[]);
            }
            Some(1)
        }
    }
}

/// Cleans finished update staging, backup directories, leftover `.tmp` files in root, and
/// cached ZIPs belonging to the current or older releases. Preserves resumable `.part` files for
/// releases newer than the running launcher.
pub fn sweep_updates(root: &PortableRoot) {
    sweep_updates_for_version(root, installed_version_tag());
}

fn sweep_updates_for_version(root: &PortableRoot, current_tag: &str) {
    if let Ok(entries) = fs::read_dir(root.path()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".new-") && name.ends_with(".tmp") {
                let _ = remove_file_with_retry(&entry.path());
            }
        }
    }
    let updates = root.updates_dir();
    let Ok(entries) = fs::read_dir(&updates) else {
        return;
    };
    let current_ver = parse_release_tag(current_tag).map(|(_, v)| v);
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("stage-") || name.starts_with("backup-") {
            let _ = remove_dir_all_with_retry(&p);
            continue;
        }
        if let (Some(cur), Some((_, dir_ver))) = (current_ver, parse_release_tag(&name)) {
            if dir_ver <= cur {
                let _ = remove_dir_all_with_retry(&p);
            }
        }
    }
}

pub fn cancel_active(state: State<'_, UpdateState>) {
    state.apply.cancel_active();
}

#[tauri::command]
pub fn get_update_status(state: State<'_, UpdateState>) -> UpdateProgressDto {
    state.apply.snapshot()
}

#[tauri::command]
pub fn cancel_update(state: State<'_, UpdateState>) {
    state.apply.cancel_active();
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, UpdateState>,
    launch: State<'_, LaunchState>,
    prepare: State<'_, PrepareState>,
    tag: String,
) -> Result<UpdateProgressDto, CodedError> {
    let _ = portable_exe_name(&state.root)?;
    if launch.is_busy() {
        return Err(CodedError::new(
            "update_game_running",
            "close Minecraft before installing a launcher update",
        ));
    }
    if prepare.is_busy() {
        return Err(CodedError::new(
            "update_prepare_running",
            "wait for profile setup to finish before updating",
        ));
    }

    let (canonical_tag, requested_ver) = parse_release_tag(&tag)
        .ok_or_else(|| CodedError::new("unsupported_value", "invalid release tag"))?;
    let current_ver = parse_release_tag(installed_version_tag()).map(|(_, v)| v);
    if current_ver.is_some_and(|c| requested_ver <= c) {
        return Err(CodedError::new(
            "update_unavailable",
            "requested release is not newer than the installed launcher",
        ));
    }

    struct StartingGuard<'a> {
        runtime: &'a ApplyRuntime,
        active: bool,
    }
    impl Drop for StartingGuard<'_> {
        fn drop(&mut self) {
            if self.active {
                let mut g = self.runtime.inner.lock().unwrap_or_else(|e| e.into_inner());
                g.starting = false;
            }
        }
    }

    let ready_candidate = {
        let mut g = state.apply.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.starting
            || matches!(
                g.stage,
                UpdateStage::Downloading
                    | UpdateStage::Verifying
                    | UpdateStage::Extracting
                    | UpdateStage::Applying
            )
        {
            return Err(CodedError::new(
                "update_in_progress",
                "an update operation is already running",
            ));
        }
        g.starting = true;
        if g.stage == UpdateStage::Ready {
            g.staged
                .as_ref()
                .filter(|s| s.version == canonical_tag)
                .cloned()
        } else {
            None
        }
    };
    let mut starting_guard = StartingGuard {
        runtime: &state.apply,
        active: true,
    };

    if let Some(staged) = ready_candidate {
        let exe = state
            .root
            .updates_dir()
            .join(&staged.stage_dir_name)
            .join(PACKAGE_ROOT_DIR)
            .join("Clade.exe");
        let expected = staged.exe_sha256.clone();
        let verified = tokio::task::spawn_blocking(move || {
            sha256_hex(&exe).is_ok_and(|h| h.eq_ignore_ascii_case(&expected))
        })
        .await
        .unwrap_or(false);
        if verified {
            return Ok(UpdateProgressDto {
                stage: UpdateStage::Ready,
                version: Some(staged.version),
                bytes_done: 0,
                bytes_total: 0,
            });
        }
    }

    let cached = state
        .checked
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let checked = match cached {
        Some(c) if c.tag == canonical_tag => c,
        _ => {
            let (_, refreshed) = update_check::refresh_checked_release(&state).await?;
            refreshed
                .filter(|c| c.tag == canonical_tag)
                .ok_or_else(|| {
                    CodedError::new("update_unavailable", "release metadata not found")
                })?
        }
    };
    if !checked.can_install {
        return Err(CodedError::new(
            "update_unavailable",
            "release does not ship verified portable zip and sha256 assets",
        ));
    }
    if launch.is_busy() {
        return Err(CodedError::new(
            "update_game_running",
            "close Minecraft before installing a launcher update",
        ));
    }
    if prepare.is_busy() {
        return Err(CodedError::new(
            "update_prepare_running",
            "wait for profile setup to finish before updating",
        ));
    }

    let sha_fetcher = state
        .sha_http
        .as_ref()
        .map_err(|m| CodedError::new("network_unavailable", m.clone()))?;
    let zip_streamer = state
        .zip_http
        .as_ref()
        .map_err(|m| CodedError::new("network_unavailable", m.clone()))?;

    let cancel = Cancel::new();
    let stale_stage = {
        let mut g = state.apply.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(g.stage, UpdateStage::Idle | UpdateStage::Ready) {
            return Err(CodedError::new(
                "update_in_progress",
                "an update operation is already running",
            ));
        }
        let stale = g
            .staged
            .take()
            .map(|old| state.root.updates_dir().join(&old.stage_dir_name));
        g.starting = false;
        starting_guard.active = false;
        g.stage = UpdateStage::Downloading;
        g.version = Some(canonical_tag.clone());
        g.bytes_done = 0;
        g.bytes_total = checked.zip_size.unwrap_or(0);
        g.cancel = Some(cancel.clone());
        stale
    };

    struct ResetGuard<'a> {
        runtime: &'a ApplyRuntime,
        done: Arc<AtomicBool>,
    }
    impl Drop for ResetGuard<'_> {
        fn drop(&mut self) {
            if !self.done.load(Ordering::Acquire) {
                self.runtime.reset_idle();
            }
        }
    }

    let completed = Arc::new(AtomicBool::new(false));
    let _guard = ResetGuard {
        runtime: &state.apply,
        done: completed.clone(),
    };

    if let Some(old_dir) = stale_stage {
        let _ = tokio::task::spawn_blocking(move || {
            let _ = remove_dir_all_with_retry(&old_dir);
        })
        .await;
    }

    let res = download_and_stage_with(
        &state.root,
        &checked,
        sha_fetcher,
        zip_streamer,
        cancel,
        |dto| {
            state.apply.set_progress(&dto);
            let _ = app.emit(UPDATE_PROGRESS_EVENT, &dto);
        },
    )
    .await;

    match res {
        Ok(staged) => {
            let ready = UpdateProgressDto {
                stage: UpdateStage::Ready,
                version: Some(staged.version.clone()),
                bytes_done: 0,
                bytes_total: 0,
            };
            {
                let mut g = state.apply.inner.lock().unwrap_or_else(|e| e.into_inner());
                g.stage = UpdateStage::Ready;
                g.version = Some(staged.version.clone());
                g.bytes_done = 0;
                g.bytes_total = 0;
                g.cancel = None;
                g.staged = Some(staged);
            }
            completed.store(true, Ordering::Release);
            let _ = app.emit(UPDATE_PROGRESS_EVENT, &ready);
            Ok(ready)
        }
        Err(err) => {
            state.apply.reset_idle();
            completed.store(true, Ordering::Release);
            let _ = app.emit(UPDATE_PROGRESS_EVENT, state.apply.snapshot());
            Err(err)
        }
    }
}

#[tauri::command]
pub fn apply_staged_update(
    app: AppHandle,
    state: State<'_, UpdateState>,
    launch: State<'_, LaunchState>,
    prepare: State<'_, PrepareState>,
) -> Result<(), CodedError> {
    if launch.is_busy() {
        return Err(CodedError::new(
            "update_game_running",
            "close Minecraft before applying a launcher update",
        ));
    }
    if prepare.is_busy() {
        return Err(CodedError::new(
            "update_prepare_running",
            "wait for profile setup to finish before updating",
        ));
    }
    let target_exe = portable_exe_name(&state.root)?;
    let staged = {
        let mut g = state.apply.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.starting {
            return Err(CodedError::new(
                "update_in_progress",
                "an update operation is already running",
            ));
        }
        if g.stage != UpdateStage::Ready {
            return Err(CodedError::new(
                "update_install_failed",
                "no staged update is ready",
            ));
        }
        let Some(staged) = g.staged.clone() else {
            return Err(CodedError::new(
                "update_install_failed",
                "missing staged update metadata",
            ));
        };
        g.stage = UpdateStage::Applying;
        staged
    };

    let helper_exe = state
        .root
        .updates_dir()
        .join(&staged.stage_dir_name)
        .join(PACKAGE_ROOT_DIR)
        .join("Clade.exe");
    if let Err(err) = verify_pe_magic(&helper_exe).and_then(|()| {
        let actual = sha256_hex(&helper_exe).map_err(|e| e.to_string())?;
        if actual.eq_ignore_ascii_case(&staged.exe_sha256) {
            Ok(())
        } else {
            Err("staged helper executable digest mismatch".into())
        }
    }) {
        state.apply.reset_idle();
        return Err(CodedError::new("update_verify_failed", err));
    }

    let pid_str = std::process::id().to_string();
    let root_str = state.root.path().to_string_lossy().into_owned();
    let args = [
        "--apply-update",
        "--target-root",
        root_str.as_str(),
        "--target-exe",
        target_exe.as_str(),
        "--wait-pid",
        pid_str.as_str(),
        "--stage-dir",
        staged.stage_dir_name.as_str(),
        "--exe-sha256",
        staged.exe_sha256.as_str(),
    ];

    if let Err(e) = spawn_clade_exe(&helper_exe, state.root.path(), &args) {
        let mut g = state.apply.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.stage = UpdateStage::Ready;
        return Err(CodedError::new("update_install_failed", e.to_string()));
    }

    let applying = UpdateProgressDto {
        stage: UpdateStage::Applying,
        version: Some(staged.version),
        bytes_done: 0,
        bytes_total: 0,
    };
    let _ = app.emit(UPDATE_PROGRESS_EVENT, &applying);

    let app_clone = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        if let Some(w) = app_clone.get_webview_window("main") {
            let _ = w.destroy();
        }
        app_clone.exit(0);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    use crate::net::testing::FakeFetcher;
    use crate::net::testing_stream::FakeStreamer;

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn fake_exe(marker: &[u8]) -> Vec<u8> {
        let mut bytes = vec![b'M', b'Z'];
        bytes.resize(64, 0);
        bytes.extend_from_slice(marker);
        bytes
    }

    fn valid_package_entries<'a>(
        exe: &'a [u8],
        wv2_exe: Option<&'a [u8]>,
    ) -> Vec<(&'a str, &'a [u8])> {
        let mut entries = vec![
            ("Clade-Launcher/Clade.exe", exe),
            ("Clade-Launcher/README.txt", b"readme-new"),
            ("Clade-Launcher/LICENSE", b"apache-2.0"),
            ("Clade-Launcher/THIRD-PARTY-NOTICES.txt", b"notices"),
            (
                "Clade-Launcher/licenses/WebView2-Fixed-Version-License.txt",
                b"wv2-license",
            ),
        ];
        if let Some(wv2) = wv2_exe {
            entries.push(("Clade-Launcher/runtimes/webview2/msedgewebview2.exe", wv2));
            entries.push((
                "Clade-Launcher/runtimes/webview2/143.0.0/EmbeddedBrowserWebView.dll",
                b"dll",
            ));
        }
        entries
    }

    fn sha256_bytes_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn download_and_stage_succeeds_and_rejects_bad_digests_and_packages() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();

        let exe = fake_exe(b"v2");
        let zip_bytes = build_zip(&valid_package_entries(&exe, Some(b"wv2-v2")));
        let zip_hex = sha256_bytes_hex(&zip_bytes);

        let tag = "v0.2.0";
        let sha_url = update_check::release_asset_url(tag, PORTABLE_SHA256_ASSET)
            .unwrap()
            .to_string();
        let zip_url = update_check::release_asset_url(tag, PORTABLE_ZIP_ASSET)
            .unwrap()
            .to_string();

        let sha_fetcher = FakeFetcher::new();
        sha_fetcher.ok(
            &sha_url,
            format!("{zip_hex}  Clade-Launcher-Portable.zip\n"),
        );
        let zip_streamer = FakeStreamer::new();
        zip_streamer.add(&zip_url, &zip_bytes);

        let checked = CheckedRelease {
            tag: tag.into(),
            zip_size: Some(zip_bytes.len() as u64),
            api_sha256: Some(zip_hex.clone()),
            can_install: true,
        };
        let mut stages = Vec::new();
        let staged = download_and_stage_with(
            &root,
            &checked,
            &sha_fetcher,
            &zip_streamer,
            Cancel::new(),
            |p| stages.push(p.stage),
        )
        .await
        .unwrap();
        assert_eq!(staged.version, "v0.2.0");
        assert!(staged.has_webview2);
        assert!(stages.contains(&UpdateStage::Downloading));
        assert!(stages.contains(&UpdateStage::Verifying));
        assert!(stages.contains(&UpdateStage::Extracting));

        // API digest mismatch stops before downloading the ZIP.
        let mut mismatched_checked = checked.clone();
        mismatched_checked.api_sha256 = Some("0".repeat(64));
        let err = download_and_stage_with(
            &root,
            &mismatched_checked,
            &sha_fetcher,
            &zip_streamer,
            Cancel::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "update_verify_failed");

        // Package containing `data/settings.json` fails allowlist validation cleanly.
        let mut bad_entries = valid_package_entries(&exe, None);
        bad_entries.push(("Clade-Launcher/data/settings.json", b"{}"));
        let bad_zip = build_zip(&bad_entries);
        let bad_hex = sha256_bytes_hex(&bad_zip);
        let bad_tag = "v0.3.0";
        let bad_sha_url = update_check::release_asset_url(bad_tag, PORTABLE_SHA256_ASSET)
            .unwrap()
            .to_string();
        let bad_zip_url = update_check::release_asset_url(bad_tag, PORTABLE_ZIP_ASSET)
            .unwrap()
            .to_string();
        sha_fetcher.ok(
            &bad_sha_url,
            format!("{bad_hex}  Clade-Launcher-Portable.zip\n"),
        );
        zip_streamer.add(&bad_zip_url, &bad_zip);
        let bad_checked = CheckedRelease {
            tag: bad_tag.into(),
            zip_size: Some(bad_zip.len() as u64),
            api_sha256: Some(bad_hex),
            can_install: true,
        };
        let err = download_and_stage_with(
            &root,
            &bad_checked,
            &sha_fetcher,
            &zip_streamer,
            Cancel::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "update_invalid_package");

        // Pre-cancelled update leaves cleanly without error noise.
        let cancel = Cancel::new();
        cancel.cancel();
        let err =
            download_and_stage_with(&root, &checked, &sha_fetcher, &zip_streamer, cancel, |_| {})
                .await
                .unwrap_err();
        assert_eq!(err.code, "update_cancelled");
    }

    #[test]
    fn transaction_swaps_skips_unchanged_webview2_replaces_changed_and_rolls_back_on_error() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("clade"));
        root.ensure_layout().unwrap();

        let old_exe = fake_exe(b"old-exe");
        fs::write(root.path().join("Clade.exe"), &old_exe).unwrap();
        fs::write(root.path().join("README.txt"), b"old-readme").unwrap();
        fs::write(root.settings_file(), br#"{"ui":{"language":"ru"}}"#).unwrap();
        let save_dir = root.instances_dir().join("survival");
        fs::create_dir_all(&save_dir).unwrap();
        fs::write(save_dir.join("level.dat"), b"world-bytes").unwrap();

        let wv2_dir = root.fixed_webview2_dir();
        fs::create_dir_all(wv2_dir.join("143.0.0")).unwrap();
        fs::write(wv2_dir.join("msedgewebview2.exe"), b"wv2-same").unwrap();
        fs::write(
            wv2_dir.join("143.0.0").join("keep.marker"),
            b"unchanged-marker",
        )
        .unwrap();

        // 1. Unchanged WebView2 stays untouched while Clade.exe and README.txt swap cleanly.
        let stage1 = root.updates_dir().join("stage-1");
        let pkg1 = stage1.join(PACKAGE_ROOT_DIR);
        fs::create_dir_all(pkg1.join("licenses")).unwrap();
        fs::create_dir_all(pkg1.join("runtimes/webview2/143.0.0")).unwrap();
        let new_exe = fake_exe(b"new-exe");
        let new_exe_hex = sha256_bytes_hex(&new_exe);
        fs::write(pkg1.join("Clade.exe"), &new_exe).unwrap();
        fs::write(pkg1.join("README.txt"), b"new-readme").unwrap();
        fs::write(pkg1.join("LICENSE"), b"apache-new").unwrap();
        fs::write(pkg1.join("THIRD-PARTY-NOTICES.txt"), b"notices-new").unwrap();
        fs::write(
            pkg1.join("licenses/WebView2-Fixed-Version-License.txt"),
            b"lic",
        )
        .unwrap();
        fs::write(
            pkg1.join("runtimes/webview2/msedgewebview2.exe"),
            b"wv2-same",
        )
        .unwrap();

        apply_update_transaction(&root, "Clade.exe", &pkg1, &new_exe_hex).unwrap();
        assert_eq!(fs::read(root.path().join("Clade.exe")).unwrap(), new_exe);
        assert_eq!(
            fs::read(root.path().join("README.txt")).unwrap(),
            b"new-readme"
        );
        assert!(wv2_dir.join("143.0.0").join("keep.marker").is_file());
        assert_eq!(
            fs::read(save_dir.join("level.dat")).unwrap(),
            b"world-bytes"
        );

        // 2. Changed WebView2 swaps cleanly; broken later package rolls everything back.
        let stage2 = root.updates_dir().join("stage-2");
        let pkg2 = stage2.join(PACKAGE_ROOT_DIR);
        fs::create_dir_all(pkg2.join("licenses")).unwrap();
        fs::create_dir_all(pkg2.join("runtimes/webview2")).unwrap();
        let v3_exe = fake_exe(b"v3-exe");
        let v3_hex = sha256_bytes_hex(&v3_exe);
        fs::write(pkg2.join("Clade.exe"), &v3_exe).unwrap();
        fs::write(pkg2.join("README.txt"), b"v3-readme").unwrap();
        fs::write(pkg2.join("LICENSE"), b"apache-v3").unwrap();
        fs::write(pkg2.join("THIRD-PARTY-NOTICES.txt"), b"notices-v3").unwrap();
        fs::write(
            pkg2.join("licenses/WebView2-Fixed-Version-License.txt"),
            b"lic",
        )
        .unwrap();
        fs::write(
            pkg2.join("runtimes/webview2/msedgewebview2.exe"),
            b"wv2-newer",
        )
        .unwrap();

        apply_update_transaction(&root, "Clade.exe", &pkg2, &v3_hex).unwrap();
        assert_eq!(
            fs::read(wv2_dir.join("msedgewebview2.exe")).unwrap(),
            b"wv2-newer"
        );

        // Missing README.txt inside pkg3 causes transaction to fail AFTER Clade.exe replaced -> full rollback!
        let stage3 = root.updates_dir().join("stage-3");
        let pkg3 = stage3.join(PACKAGE_ROOT_DIR);
        fs::create_dir_all(&pkg3).unwrap();
        let v4_exe = fake_exe(b"v4-exe");
        let v4_hex = sha256_bytes_hex(&v4_exe);
        fs::write(pkg3.join("Clade.exe"), &v4_exe).unwrap();
        assert!(apply_update_transaction(&root, "Clade.exe", &pkg3, &v4_hex).is_err());
        assert_eq!(fs::read(root.path().join("Clade.exe")).unwrap(), v3_exe);
        assert_eq!(
            fs::read(root.path().join("README.txt")).unwrap(),
            b"v3-readme"
        );
    }

    #[test]
    fn sweep_updates_cleans_stage_backup_old_versions_and_keeps_newer_part() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();

        fs::create_dir_all(root.updates_dir().join("stage-abc")).unwrap();
        fs::create_dir_all(root.updates_dir().join("backup-xyz")).unwrap();
        fs::create_dir_all(root.updates_dir().join("v0.1.0-preview.3")).unwrap();
        fs::create_dir_all(root.updates_dir().join("v0.1.0-preview.4")).unwrap();
        fs::create_dir_all(root.updates_dir().join("v0.2.0")).unwrap();
        fs::write(
            root.updates_dir()
                .join("v0.2.0")
                .join("Clade-Launcher-Portable.zip.part"),
            b"partial",
        )
        .unwrap();
        fs::write(root.path().join("Clade.exe.new-123.tmp"), b"stale").unwrap();

        sweep_updates_for_version(&root, "v0.1.0-preview.4");

        assert!(!root.updates_dir().join("stage-abc").exists());
        assert!(!root.updates_dir().join("backup-xyz").exists());
        assert!(!root.updates_dir().join("v0.1.0-preview.3").exists());
        assert!(!root.updates_dir().join("v0.1.0-preview.4").exists());
        assert!(!root.path().join("Clade.exe.new-123.tmp").exists());
        assert!(root
            .updates_dir()
            .join("v0.2.0")
            .join("Clade-Launcher-Portable.zip.part")
            .is_file());
    }

    #[test]
    fn parse_flag_value_extracts_all_helper_flags_after_mode_switch() {
        let args: Vec<String> = [
            r"D:\Clade\cache\updates\stage-1\Clade-Launcher\Clade.exe",
            "--apply-update",
            "--target-root",
            r"D:\Clade",
            "--target-exe",
            "Clade.exe",
            "--wait-pid",
            "4242",
            "--stage-dir",
            "stage-1",
            "--exe-sha256",
            &"a".repeat(64),
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();

        assert_eq!(parse_flag_value(&args, "--target-root"), Some(r"D:\Clade"));
        assert_eq!(parse_flag_value(&args, "--target-exe"), Some("Clade.exe"));
        assert_eq!(parse_flag_value(&args, "--wait-pid"), Some("4242"));
        assert_eq!(parse_flag_value(&args, "--stage-dir"), Some("stage-1"));
        assert_eq!(
            parse_flag_value(&args, "--exe-sha256"),
            Some("a".repeat(64).as_str())
        );
        assert_eq!(parse_flag_value(&args, "--missing"), None);

        let truncated = args[..args.len() - 1].to_vec();
        assert_eq!(parse_flag_value(&truncated, "--exe-sha256"), None);
    }

    #[test]
    fn check_allowed_relpath_rejects_forbidden_directory_names() {
        assert!(check_allowed_relpath("runtimes/webview2/.env", true).is_err());
        assert!(check_allowed_relpath("runtimes/webview2/bad.zip/foo.dll", false).is_err());
        assert!(check_allowed_relpath("runtimes/webview2/143.0/foo.dll", false).is_ok());
    }

    #[test]
    fn paths_equal_identity_normalizes_verbatim_prefix_and_unicode_case() {
        let a = Path::new(r"\\?\C:\Пользователи\Иван\Clade-Launcher\Clade.exe");
        let b = Path::new(r"c:\пользователи\иван\Clade-Launcher\clade.exe");
        assert!(paths_equal_identity(a, b));
    }

    #[test]
    fn rollback_returns_false_when_backup_cannot_be_restored() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("Clade.exe");
        let missing_backup = d.path().join("backup").join("Clade.exe");
        assert!(!rollback_actions(vec![SwapAction::ReplacedFile {
            target,
            backup: missing_backup,
        }]));
    }

    #[tokio::test]
    async fn download_and_stage_honors_cancellation_during_extraction() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();

        let exe = fake_exe(b"v2");
        let zip_bytes = build_zip(&valid_package_entries(&exe, Some(b"wv2-v2")));
        let zip_hex = sha256_bytes_hex(&zip_bytes);

        let tag = "v0.2.0";
        let sha_url = update_check::release_asset_url(tag, PORTABLE_SHA256_ASSET)
            .unwrap()
            .to_string();
        let zip_url = update_check::release_asset_url(tag, PORTABLE_ZIP_ASSET)
            .unwrap()
            .to_string();

        let sha_fetcher = FakeFetcher::new();
        sha_fetcher.ok(
            &sha_url,
            format!("{zip_hex}  Clade-Launcher-Portable.zip\n"),
        );
        let zip_streamer = FakeStreamer::new();
        zip_streamer.add(&zip_url, &zip_bytes);

        let checked = CheckedRelease {
            tag: tag.into(),
            zip_size: Some(zip_bytes.len() as u64),
            api_sha256: Some(zip_hex),
            can_install: true,
        };
        let cancel = Cancel::new();
        let cancel_on_extract = cancel.clone();

        let err = download_and_stage_with(
            &root,
            &checked,
            &sha_fetcher,
            &zip_streamer,
            cancel,
            |dto| {
                if dto.stage == UpdateStage::Extracting {
                    cancel_on_extract.cancel();
                }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "update_cancelled");
    }
}
