//! The file download pipeline: verified, resumable, cancellable and bounded.
//!
//! Every file is described by a [`FileJob`] with a mandatory hash (SHA-1 or SHA-256) and an
//! optional size taken from official metadata; a download that cannot be verified is never
//! created. For each job:
//! 1. An existing destination that verifies is **reused** (no network). One that does not
//!    verify (a corrupted cached file) is simply downloaded again and only replaced once the
//!    new bytes have been verified.
//! 2. Bytes stream into `<dest>.part` next to the destination (same volume, so the final step
//!    is an atomic rename). A `.part` left by an interrupted run is **resumed** with an HTTP
//!    `Range` request when the server supports it, and restarted from zero when it does not.
//!    A `.part` is never trusted as complete: it must verify against the expected hash.
//! 3. Transient failures (no connection, timeouts, 408/429/5xx) are retried with a growing
//!    delay; a corrupt result is discarded and fetched again a limited number of times; any
//!    other 4xx is final.
//! 4. Cancellation is honoured between chunks and while waiting; the `.part` stays so the next
//!    attempt resumes instead of starting from zero.
//!
//! Concurrency is bounded ([`Downloader::fetch_all`]); progress is counted in bytes and items
//! so callers can report real numbers (and nothing is reported when a total is unknown).

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use url::Url;

use crate::net::{FetchError, StreamHead, StreamSink, Streamer};
use crate::portable::fsutil::{
    is_link, reject_links_between, remove_file_with_retry, rename_with_retry, verify_file, Expected,
};

/// Failed attempts tolerated per file before giving up on the connection.
const MAX_TRANSIENT: u32 = 3;
/// Corrupt results tolerated per file before reporting a verification failure.
const MAX_CORRUPT: u32 = 2;
/// Hard ceiling when a job carries no declared metadata size (`job.size` is `None`).
const MAX_UNDECLARED_BYTES: u64 = 512 * 1024 * 1024;
/// Write buffer size per active download stream to coalesce small network chunks on USB flash drives.
const PART_BUF_CAPACITY: usize = 256 * 1024;

// ------------------------------------------------------------------------------ cancel

/// A cancellation switch shared between the UI command and every running download.
#[derive(Debug, Default)]
pub struct Cancel {
    flag: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Cancel {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    /// Resolves once cancelled.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

struct CancelScope<'a> {
    outer: &'a Cancel,
    batch: Option<&'a Cancel>,
}

impl CancelScope<'_> {
    fn is_cancelled(&self) -> bool {
        self.outer.is_cancelled() || self.batch.is_some_and(Cancel::is_cancelled)
    }

    async fn cancelled(&self) {
        match self.batch {
            None => self.outer.cancelled().await,
            Some(b) => {
                tokio::select! {
                    () = self.outer.cancelled() => {}
                    () = b.cancelled() => {}
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------ progress

/// Shared counters. `bytes_total` only grows as work is planned; `bytes_done` includes files that
/// were already present (so progress reflects what is left to do).
#[derive(Debug, Default)]
pub struct Progress {
    bytes_total: AtomicU64,
    bytes_done: AtomicU64,
    items_total: AtomicU64,
    items_done: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub items_total: u64,
    pub items_done: u64,
}

impl Progress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Announces planned bytes (call before the work starts, so percentages are honest).
    pub fn plan_bytes(&self, n: u64) {
        self.bytes_total.fetch_add(n, Ordering::SeqCst);
    }
    /// Starts a new counting section (a stage), e.g. "libraries".
    pub fn begin_items(&self, total: u64) {
        self.items_total.store(total, Ordering::SeqCst);
        self.items_done.store(0, Ordering::SeqCst);
    }
    pub fn item_done(&self) {
        self.items_done.fetch_add(1, Ordering::SeqCst);
    }
    pub fn add_done(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::SeqCst);
    }
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            bytes_total: self.bytes_total.load(Ordering::SeqCst),
            bytes_done: self.bytes_done.load(Ordering::SeqCst),
            items_total: self.items_total.load(Ordering::SeqCst),
            items_done: self.items_done.load(Ordering::SeqCst),
        }
    }
    /// Moves a job's byte credit to `target` (resume, restart or completion).
    fn set_credit(&self, credited: &mut u64, target: u64) {
        if target >= *credited {
            self.bytes_done
                .fetch_add(target - *credited, Ordering::SeqCst);
        } else {
            self.bytes_done
                .fetch_sub(*credited - target, Ordering::SeqCst);
        }
        *credited = target;
    }
}

// ------------------------------------------------------------------------------ jobs and errors

#[derive(Debug, Clone)]
pub struct FileJob {
    pub url: Url,
    pub dest: PathBuf,
    pub expected: Expected,
    /// Exact size when the metadata gives one (always checked, and used to resume safely).
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Already present and verified.
    Reused,
    Downloaded,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub reused: usize,
    pub downloaded: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("cancelled")]
    Cancelled,
    #[error("no connection: {0}")]
    Offline(String),
    #[error("the server refused a file (status {0})")]
    Status(u16),
    #[error("a downloaded file failed verification ({0})")]
    Verification(String),
    #[error("blocked by the network policy: {0}")]
    Policy(String),
    #[error("could not store a file: {0}")]
    Local(String),
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// What the part writer says when the server sends more than the file can be. Compared by the
/// downloader, never shown.
const OVERSIZE: &str = "the server sent more data than the file's declared size";

enum InlineHasher {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

impl InlineHasher {
    fn for_expected(expected: &Expected) -> Self {
        use sha2::digest::Digest;
        match expected {
            Expected::Sha1(_) => Self::Sha1(sha1::Sha1::new()),
            Expected::Sha256(_) => Self::Sha256(sha2::Sha256::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        use sha2::digest::Digest;
        match self {
            Self::Sha1(h) => h.update(data),
            Self::Sha256(h) => h.update(data),
        }
    }

    fn finalize_hex(self) -> String {
        use sha2::digest::Digest;
        match self {
            Self::Sha1(h) => hex::encode(h.finalize()),
            Self::Sha256(h) => hex::encode(h.finalize()),
        }
    }
}

/// Writes the stream into the `.part` file, resuming or restarting as the server answered. When the
/// file's size is known it is a hard ceiling for the whole part (bytes already kept from an earlier
/// attempt count towards it); otherwise [`MAX_UNDECLARED_BYTES`] caps the stream.
struct PartWriter<'a> {
    part: &'a Path,
    from: u64,
    limit: Option<u64>,
    expected: &'a Expected,
    /// Bytes the part holds (kept from before plus written now).
    len: u64,
    writer: Option<BufWriter<File>>,
    hasher: Option<InlineHasher>,
    progress: &'a Progress,
    credited: &'a mut u64,
    cancel: &'a CancelScope<'a>,
}

impl PartWriter<'_> {
    /// Flushes buffered bytes, closes the underlying file handle, and returns the inline
    /// `(hex_digest, len)` when streamed from byte zero (`None` when resumed from `start > 0`).
    fn finish(mut self) -> io::Result<Option<(String, u64)>> {
        let Some(mut w) = self.writer.take() else {
            return Err(io::Error::other("response completed without headers"));
        };
        w.flush()?;
        let file = w.into_inner().map_err(|e| e.into_error())?;
        drop(file);
        Ok(self.hasher.take().map(|h| (h.finalize_hex(), self.len)))
    }
}

impl StreamSink for PartWriter<'_> {
    fn begin(&mut self, head: &StreamHead) -> io::Result<()> {
        // Refuse before touching the file when the server announces too much.
        let resuming = head.status == 206 && self.from > 0;
        let start = if resuming { self.from } else { 0 };
        let ceiling = self.limit.unwrap_or(MAX_UNDECLARED_BYTES);
        if let Some(announced) = head.content_length {
            if start.saturating_add(announced) > ceiling {
                return Err(io::Error::new(io::ErrorKind::InvalidData, OVERSIZE));
            }
        }
        self.len = start;
        if resuming {
            let mut f = OpenOptions::new().write(true).open(self.part)?;
            f.seek(SeekFrom::Start(self.from))?;
            f.set_len(self.from)?;
            self.writer = Some(BufWriter::with_capacity(PART_BUF_CAPACITY, f));
            self.hasher = None;
            self.progress.set_credit(self.credited, self.from);
        } else {
            // Fresh download or the server ignored the range and sent 200: start over and hash inline.
            let f = File::create(self.part)?;
            self.writer = Some(BufWriter::with_capacity(PART_BUF_CAPACITY, f));
            self.hasher = Some(InlineHasher::for_expected(self.expected));
            self.progress.set_credit(self.credited, 0);
        }
        Ok(())
    }

    fn chunk(&mut self, data: &[u8]) -> io::Result<()> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let ceiling = self.limit.unwrap_or(MAX_UNDECLARED_BYTES);
        if self.len.saturating_add(data.len() as u64) > ceiling {
            return Err(io::Error::new(io::ErrorKind::InvalidData, OVERSIZE));
        }
        let w = self
            .writer
            .as_mut()
            .ok_or_else(|| io::Error::other("data before headers"))?;
        w.write_all(data)?;
        if let Some(h) = self.hasher.as_mut() {
            h.update(data);
        }
        self.len += data.len() as u64;
        self.progress.add_done(data.len() as u64);
        *self.credited += data.len() as u64;
        Ok(())
    }
}

fn reject_if_link(p: &Path) -> io::Result<()> {
    match is_link(p) {
        Ok(true) => Err(io::Error::other(format!(
            "refusing to follow link at {}",
            p.display()
        ))),
        Ok(false) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn ensure_parent_safe(root: Option<&Path>, parent: &Path) -> io::Result<()> {
    if let Some(base) = root {
        reject_links_between(base, parent)?;
        fs::create_dir_all(parent)?;
        reject_links_between(base, parent)?;
    } else {
        let mut existing = parent;
        while fs::symlink_metadata(existing).is_err() {
            match existing.parent() {
                Some(p) if !p.as_os_str().is_empty() => existing = p,
                _ => break,
            }
        }
        reject_links_between(existing, parent)?;
        fs::create_dir_all(parent)?;
        reject_links_between(existing, parent)?;
        reject_if_link(parent)?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------ downloader

pub struct Downloader<'a, S: Streamer> {
    streamer: &'a S,
    cancel: Arc<Cancel>,
    progress: Arc<Progress>,
    backoff: Duration,
    root: Option<PathBuf>,
}

impl<'a, S: Streamer> Downloader<'a, S> {
    pub fn new(streamer: &'a S, cancel: Arc<Cancel>, progress: Arc<Progress>) -> Self {
        Self {
            streamer,
            cancel,
            progress,
            backoff: Duration::from_millis(400),
            root: None,
        }
    }

    /// Base delay between retries (it triples per attempt). Tests use zero.
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    /// Confines downloads to `root`, verifying no component between `root` and each destination
    /// is a symlink or NTFS junction.
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    pub fn progress(&self) -> &Arc<Progress> {
        &self.progress
    }

    async fn verifies(path: PathBuf, expected: Expected, size: Option<u64>) -> bool {
        tokio::task::spawn_blocking(move || verify_file(&path, &expected, size).is_ok())
            .await
            .unwrap_or(false)
    }

    async fn pause(&self, attempt: u32, scope: &CancelScope<'_>) -> Result<(), DownloadError> {
        let wait = self.backoff * 3u32.saturating_pow(attempt.saturating_sub(1));
        if wait.is_zero() {
            return if scope.is_cancelled() {
                Err(DownloadError::Cancelled)
            } else {
                Ok(())
            };
        }
        tokio::select! {
            () = tokio::time::sleep(wait) => Ok(()),
            () = scope.cancelled() => Err(DownloadError::Cancelled),
        }
    }

    /// Makes `job.dest` exist and verify, downloading only what is needed.
    pub async fn fetch(&self, job: &FileJob) -> Result<Outcome, DownloadError> {
        let scope = CancelScope {
            outer: &self.cancel,
            batch: None,
        };
        self.fetch_with_scope(job, &scope, false).await
    }

    async fn fetch_with_scope(
        &self,
        job: &FileJob,
        scope: &CancelScope<'_>,
        parents_ready: bool,
    ) -> Result<Outcome, DownloadError> {
        if scope.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let local = |e: io::Error| DownloadError::Local(e.to_string());
        let mut credited = 0u64;
        let part = part_path(&job.dest);

        let dest_init = job.dest.clone();
        let part_init = part.clone();
        let expected_init = job.expected.clone();
        let size_init = job.size;
        let root_init = self.root.clone();
        let reuse_len = tokio::task::spawn_blocking(move || -> io::Result<Option<u64>> {
            match (
                parents_ready,
                dest_init.parent().filter(|p| !p.as_os_str().is_empty()),
                root_init.as_deref(),
            ) {
                (false, Some(parent), root) => ensure_parent_safe(root, parent)?,
                (_, None, Some(base)) => reject_links_between(base, &dest_init)?,
                _ => {}
            }
            reject_if_link(&dest_init)?;
            reject_if_link(&part_init)?;
            match fs::metadata(&dest_init) {
                Ok(md)
                    if md.is_file()
                        && verify_file(&dest_init, &expected_init, size_init).is_ok() =>
                {
                    return Ok(Some(md.len()));
                }
                _ => {}
            }
            Ok(None)
        })
        .await
        .map_err(|e| DownloadError::Local(e.to_string()))?
        .map_err(local)?;

        if let Some(len) = reuse_len {
            self.progress
                .set_credit(&mut credited, job.size.unwrap_or(len));
            self.progress.item_done();
            return Ok(Outcome::Reused);
        }

        let (mut transient, mut corrupt) = (0u32, 0u32);

        loop {
            if scope.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            let max_part = job.size.unwrap_or(MAX_UNDECLARED_BYTES);
            let part_check = part.clone();
            let from = tokio::task::spawn_blocking(move || -> io::Result<u64> {
                reject_if_link(&part_check)?;
                let mut len = match fs::metadata(&part_check) {
                    Ok(m) => m.len(),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
                    Err(e) => return Err(e),
                };
                if len > max_part {
                    let _ = remove_file_with_retry(&part_check); // longer than the file can be: garbage
                    len = 0;
                }
                Ok(len)
            })
            .await
            .map_err(|e| DownloadError::Local(e.to_string()))?
            .map_err(local)?;

            let already_whole = from > 0 && job.size == Some(from);
            let mut inline_verified: Option<(String, u64)> = None;
            if !already_whole {
                let mut sink = PartWriter {
                    part: &part,
                    from,
                    limit: job.size,
                    expected: &job.expected,
                    len: from,
                    writer: None,
                    hasher: None,
                    progress: &self.progress,
                    credited: &mut credited,
                    cancel: scope,
                };
                let result = tokio::select! {
                    r = self.streamer.stream(&job.url, from, &mut sink) => r,
                    () = scope.cancelled() => {
                        drop(sink);
                        return Err(DownloadError::Cancelled);
                    }
                };
                match result {
                    Ok(()) => {
                        inline_verified = sink.finish().map_err(local)?;
                    }
                    Err(FetchError::Local(m)) if m.contains(OVERSIZE) => {
                        drop(sink);
                        // More bytes than the file can have: whatever was written is not this file.
                        let part_rm = part.clone();
                        let _ =
                            tokio::task::spawn_blocking(move || remove_file_with_retry(&part_rm))
                                .await;
                        self.progress.set_credit(&mut credited, 0);
                        corrupt += 1;
                        if corrupt > MAX_CORRUPT {
                            let name = job.dest.file_name().unwrap_or_default().to_string_lossy();
                            return Err(DownloadError::Verification(name.into_owned()));
                        }
                        continue;
                    }
                    Err(FetchError::Local(m)) => {
                        drop(sink);
                        return Err(if scope.is_cancelled() {
                            DownloadError::Cancelled
                        } else {
                            DownloadError::Local(m)
                        });
                    }
                    Err(FetchError::Policy(e)) => {
                        drop(sink);
                        return Err(DownloadError::Policy(e.to_string()));
                    }
                    Err(FetchError::TooLarge(_)) => {
                        drop(sink);
                        return Err(DownloadError::Local("unexpected size limit".into()));
                    }
                    Err(FetchError::Status(416)) => {
                        drop(sink);
                        // The part is not a prefix of the file any more: start over.
                        let part_rm = part.clone();
                        let _ =
                            tokio::task::spawn_blocking(move || remove_file_with_retry(&part_rm))
                                .await;
                        self.progress.set_credit(&mut credited, 0);
                        corrupt += 1;
                        if corrupt > MAX_CORRUPT {
                            return Err(DownloadError::Status(416));
                        }
                        continue;
                    }
                    Err(FetchError::Status(code)) if code == 408 || code == 429 || code >= 500 => {
                        drop(sink);
                        transient += 1;
                        if transient > MAX_TRANSIENT {
                            return Err(DownloadError::Status(code));
                        }
                        self.pause(transient, scope).await?;
                        continue;
                    }
                    Err(FetchError::Status(code)) => {
                        drop(sink);
                        return Err(DownloadError::Status(code));
                    }
                    Err(FetchError::Network(m)) => {
                        drop(sink);
                        transient += 1;
                        if transient > MAX_TRANSIENT {
                            return Err(DownloadError::Offline(m));
                        }
                        self.pause(transient, scope).await?;
                        continue;
                    }
                }
            }

            // Fresh downloads (`start == 0`) verify size + digest in memory without re-reading `.part`
            // from USB; resumed (`start > 0`) or pre-existing whole `.part` files verify from disk.
            if let Some((actual_hex, len)) = inline_verified {
                let want_hex = match &job.expected {
                    Expected::Sha1(h) | Expected::Sha256(h) => h.as_str(),
                };
                if job.size.is_none_or(|s| s == len) && actual_hex.eq_ignore_ascii_case(want_hex) {
                    let part_p = part.clone();
                    let dest_p = job.dest.clone();
                    tokio::task::spawn_blocking(move || -> io::Result<()> {
                        reject_if_link(&dest_p)?;
                        rename_with_retry(&part_p, &dest_p)
                    })
                    .await
                    .map_err(|e| DownloadError::Local(e.to_string()))?
                    .map_err(local)?;
                    self.progress
                        .set_credit(&mut credited, job.size.unwrap_or(len));
                    self.progress.item_done();
                    return Ok(Outcome::Downloaded);
                }
            } else if Self::verifies(part.clone(), job.expected.clone(), job.size).await {
                let part_p = part.clone();
                let dest_p = job.dest.clone();
                let len = tokio::task::spawn_blocking(move || -> io::Result<u64> {
                    reject_if_link(&dest_p)?;
                    let len = fs::metadata(&part_p)?.len();
                    rename_with_retry(&part_p, &dest_p)?;
                    Ok(len)
                })
                .await
                .map_err(|e| DownloadError::Local(e.to_string()))?
                .map_err(local)?;
                self.progress
                    .set_credit(&mut credited, job.size.unwrap_or(len));
                self.progress.item_done();
                return Ok(Outcome::Downloaded);
            }

            let part_rm = part.clone();
            let _ = tokio::task::spawn_blocking(move || remove_file_with_retry(&part_rm)).await;
            self.progress.set_credit(&mut credited, 0);
            corrupt += 1;
            if corrupt > MAX_CORRUPT {
                let name = job.dest.file_name().unwrap_or_default().to_string_lossy();
                return Err(DownloadError::Verification(name.into_owned()));
            }
        }
    }

    /// Fetches many files with at most `concurrency` in flight; stops at the first failure,
    /// cancels sibling jobs immediately, and drains every in-flight task to completion so all
    /// file handles are closed before returning.
    pub async fn fetch_all(
        &self,
        jobs: Vec<FileJob>,
        concurrency: usize,
    ) -> Result<Report, DownloadError> {
        if jobs.is_empty() {
            return Ok(Report::default());
        }
        if self.cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }

        let mut parents = HashSet::new();
        for job in &jobs {
            if let Some(parent) = job.dest.parent().filter(|p| !p.as_os_str().is_empty()) {
                parents.insert(parent.to_path_buf());
            }
        }
        if !parents.is_empty() {
            let root = self.root.clone();
            tokio::task::spawn_blocking(move || -> io::Result<()> {
                for parent in &parents {
                    ensure_parent_safe(root.as_deref(), parent)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| DownloadError::Local(e.to_string()))?
            .map_err(|e| DownloadError::Local(e.to_string()))?;
        }

        let batch_cancel = Cancel::new();
        let mut report = Report::default();
        let mut results = stream::iter(jobs)
            .map(|job| {
                let scope = CancelScope {
                    outer: &self.cancel,
                    batch: Some(&batch_cancel),
                };
                async move { self.fetch_with_scope(&job, &scope, true).await }
            })
            .buffer_unordered(concurrency.clamp(1, 32));
        let mut first_err: Option<DownloadError> = None;
        while let Some(r) = results.next().await {
            match r {
                Ok(Outcome::Reused) => report.reused += 1,
                Ok(Outcome::Downloaded) => report.downloaded += 1,
                Err(e) => {
                    batch_cancel.cancel();
                    first_err = Some(e);
                    break;
                }
            }
        }
        if let Some(err) = first_err {
            while results.next().await.is_some() {}
            return Err(err);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::testing_stream::FakeStreamer;
    use sha1::{Digest, Sha1};

    fn sha1_of(b: &[u8]) -> Expected {
        Expected::Sha1(hex::encode(Sha1::digest(b)))
    }

    const URL: &str = "https://libraries.minecraft.net/a/b.jar";

    fn body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    struct Env {
        dir: tempfile::TempDir,
        net: FakeStreamer,
        progress: Arc<Progress>,
        cancel: Arc<Cancel>,
    }

    fn env() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
            net: FakeStreamer::new(),
            progress: Progress::new(),
            cancel: Cancel::new(),
        }
    }

    impl Env {
        fn dl(&self) -> Downloader<'_, FakeStreamer> {
            Downloader::new(&self.net, self.cancel.clone(), self.progress.clone())
                .with_backoff(Duration::ZERO)
        }
        fn job(&self, name: &str, data: &[u8]) -> FileJob {
            FileJob {
                url: Url::parse(URL).unwrap(),
                dest: self.dir.path().join(name),
                expected: sha1_of(data),
                size: Some(data.len() as u64),
            }
        }
    }

    #[tokio::test]
    async fn downloads_verifies_and_then_reuses_without_the_network() {
        let e = env();
        let data = body(1000);
        e.net.add(URL, &data);
        let job = e.job("lib/b.jar", &data);
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Downloaded);
        assert_eq!(fs::read(&job.dest).unwrap(), data);
        assert!(!part_path(&job.dest).exists(), "no part left behind");
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Reused);
        assert_eq!(e.net.total_calls(), 1);
        let s = e.progress.snapshot();
        assert_eq!(
            s.bytes_done, 2000,
            "each fetch credits its file once (download, then reuse)"
        );
        assert_eq!(s.items_done, 2);
    }

    #[tokio::test]
    async fn an_interrupted_download_resumes_from_where_it_stopped() {
        let e = env();
        let data = body(1000);
        e.net.add(URL, &data);
        e.net.break_once_after(URL, 300);
        let job = e.job("b.jar", &data);
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Downloaded);
        assert_eq!(fs::read(&job.dest).unwrap(), data);
        let calls = e.net.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, 0);
        assert!(calls[1].1 >= 300, "resumed, not restarted: {calls:?}");
        assert_eq!(e.progress.snapshot().bytes_done, 1000, "no double counting");
    }

    /// How large the `.part` is on disk after an attempt.
    fn part_len(job: &FileJob) -> u64 {
        fs::metadata(part_path(&job.dest))
            .map(|m| m.len())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn an_exact_size_response_is_accepted() {
        let e = env();
        let data = body(1000);
        e.net.add(URL, &data);
        e.net.hide_length(URL);
        let job = e.job("b.jar", &data);
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Downloaded);
        assert_eq!(fs::read(&job.dest).unwrap(), data);
    }

    #[tokio::test]
    async fn an_undersized_response_is_never_promoted() {
        let e = env();
        let data = body(1000);
        e.net.add(URL, &data[..700]);
        let job = e.job("b.jar", &data);
        assert!(e.dl().fetch(&job).await.is_err());
        assert!(!job.dest.exists());
    }

    #[tokio::test]
    async fn an_oversized_response_never_grows_the_part_past_the_declared_size() {
        // announced up front (Content-Length): refused before a byte is written
        let e = env();
        let data = body(1000);
        e.net.add(URL, vec![7u8; 100_000]);
        let job = e.job("b.jar", &data);
        assert!(matches!(
            e.dl().fetch(&job).await,
            Err(DownloadError::Verification(_))
        ));
        assert!(!job.dest.exists());
        assert_eq!(part_len(&job), 0, "nothing was written");

        // not announced (chunked): stopped as soon as it passes the size
        let e = env();
        e.net.add(URL, vec![7u8; 100_000]);
        e.net.hide_length(URL);
        let job = e.job("c.jar", &data);
        assert!(matches!(
            e.dl().fetch(&job).await,
            Err(DownloadError::Verification(_))
        ));
        assert!(!job.dest.exists());
        assert!(part_len(&job) <= 1000, "the part stayed within the size");
        assert_eq!(
            e.net.total_calls(),
            3,
            "one attempt plus the two corrupt-result retries"
        );
    }

    #[tokio::test]
    async fn a_resumed_transfer_may_reach_exactly_the_size_but_not_pass_it() {
        // resumed from 500 bytes, 500 more arrive: exactly 1000 in total, accepted
        let e = env();
        let data = body(1000);
        e.net.add(URL, &data);
        e.net.hide_length(URL);
        let job = e.job("b.jar", &data);
        fs::write(part_path(&job.dest), &data[..500]).unwrap();
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Downloaded);
        assert_eq!(fs::read(&job.dest).unwrap(), data);

        // resumed from 500 bytes, but more than 500 would follow: the limit is the total size
        let mut longer = data.clone();
        longer.extend_from_slice(&body(400)); // 1400 bytes behind the same URL
        let e = env();
        e.net.add(URL, &longer);
        e.net.hide_length(URL);
        let job = e.job("c.jar", &data);
        fs::write(part_path(&job.dest), &data[..500]).unwrap();
        assert!(e.dl().fetch(&job).await.is_err());
        assert!(!job.dest.exists());
        assert!(part_len(&job) <= 1000);
        // with the length announced, the refusal comes before anything is written
        let e = env();
        e.net.add(URL, &longer);
        let job = e.job("d.jar", &data);
        fs::write(part_path(&job.dest), &data[..500]).unwrap();
        assert!(e.dl().fetch(&job).await.is_err());
        assert!(!job.dest.exists());
    }

    #[tokio::test]
    async fn a_part_left_by_a_previous_run_is_resumed_after_a_restart() {
        let e = env();
        let data = body(800);
        e.net.add(URL, &data);
        let job = e.job("b.jar", &data);
        fs::write(part_path(&job.dest), &data[..500]).unwrap(); // what a crashed run left
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Downloaded);
        assert_eq!(e.net.calls(), vec![(URL.to_owned(), 500)]);
        assert_eq!(fs::read(&job.dest).unwrap(), data);
    }

    #[tokio::test]
    async fn a_server_without_range_support_restarts_from_zero_correctly() {
        let e = env();
        let data = body(600);
        e.net.add(URL, &data);
        e.net.ignore_ranges(URL);
        let job = e.job("b.jar", &data);
        fs::write(part_path(&job.dest), &data[..200]).unwrap();
        e.dl().fetch(&job).await.unwrap();
        assert_eq!(fs::read(&job.dest).unwrap(), data);
        assert_eq!(e.progress.snapshot().bytes_done, 600);
    }

    #[tokio::test]
    async fn a_corrupt_part_is_never_promoted_and_the_file_is_fetched_cleanly() {
        let e = env();
        let data = body(500);
        e.net.add(URL, &data);
        let job = e.job("b.jar", &data);
        fs::write(part_path(&job.dest), vec![9u8; 500]).unwrap(); // right size, wrong bytes
        e.dl().fetch(&job).await.unwrap();
        assert_eq!(fs::read(&job.dest).unwrap(), data);
    }

    #[tokio::test]
    async fn a_corrupt_cached_file_is_replaced_only_by_verified_bytes() {
        let e = env();
        let data = body(500);
        e.net.add(URL, &data);
        let job = e.job("b.jar", &data);
        fs::write(&job.dest, b"corrupted").unwrap();
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Downloaded);
        assert_eq!(fs::read(&job.dest).unwrap(), data);

        // A server that now serves garbage can neither replace a good file nor create a bad one.
        e.net.replace_body(URL, vec![1u8; 500]);
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Reused);
        let bad = e.job("other.jar", &data);
        let r = e.dl().fetch(&bad).await;
        assert!(matches!(r, Err(DownloadError::Verification(_))), "{r:?}");
        assert!(!bad.dest.exists() && !part_path(&bad.dest).exists());
    }

    #[tokio::test]
    async fn transient_failures_are_retried_and_permanent_ones_are_not() {
        let e = env();
        let data = body(300);
        e.net.add(URL, &data);
        let job = e.job("b.jar", &data);
        e.net.answer_status(URL, 503);
        let first = e.dl().fetch(&job).await;
        assert!(matches!(first, Err(DownloadError::Status(503))));
        assert_eq!(e.net.count(URL), 1 + MAX_TRANSIENT as usize);
        e.net.clear_status(URL);
        assert_eq!(e.dl().fetch(&job).await.unwrap(), Outcome::Downloaded);

        let e = env();
        e.net.add(URL, &data);
        e.net.answer_status(URL, 404);
        let r = e.dl().fetch(&e.job("c.jar", &data)).await;
        assert!(matches!(r, Err(DownloadError::Status(404))));
        assert_eq!(e.net.count(URL), 1, "a 404 is final");
    }

    #[tokio::test]
    async fn no_connection_is_reported_as_offline_after_retrying() {
        let e = env();
        let data = body(10);
        let r = e.dl().fetch(&e.job("b.jar", &data)).await; // no route at all
        assert!(matches!(r, Err(DownloadError::Offline(_))), "{r:?}");
    }

    #[tokio::test]
    async fn cancelling_keeps_the_part_so_the_next_attempt_resumes() {
        let e = env();
        let data = body(2000);
        e.net.add(URL, &data);
        let job = e.job("b.jar", &data);
        e.cancel.cancel();
        assert!(matches!(
            e.dl().fetch(&job).await,
            Err(DownloadError::Cancelled)
        ));
        assert!(!job.dest.exists());

        // Cancel half-way: a sink that stops after the first chunks.
        let e = env();
        e.net.add(URL, &data);
        let job = e.job("b.jar", &data);
        let cancel = e.cancel.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            cancel.cancel();
        });
        let r = e.dl().fetch(&job).await;
        canceller.await.unwrap();
        // Either it finished first or it was cancelled; it must never leave a bad final file.
        match r {
            Ok(_) => assert_eq!(fs::read(&job.dest).unwrap(), data),
            Err(DownloadError::Cancelled) => assert!(!job.dest.exists()),
            Err(other) => panic!("{other:?}"),
        }
        // A fresh run (new cancel token) completes the file.
        let e2 = Env {
            cancel: Cancel::new(),
            ..e
        };
        e2.dl().fetch(&job).await.unwrap();
        assert_eq!(fs::read(&job.dest).unwrap(), data);
    }

    #[tokio::test]
    async fn fetch_all_downloads_in_parallel_and_counts_items_and_bytes() {
        let e = env();
        let mut jobs = Vec::new();
        for i in 0..12 {
            let data = body(100 + i);
            let url = format!("https://libraries.minecraft.net/f/{i}.jar");
            e.net.add(&url, &data);
            jobs.push(FileJob {
                url: Url::parse(&url).unwrap(),
                dest: e.dir.path().join(format!("f{i}.jar")),
                expected: sha1_of(&data),
                size: Some(data.len() as u64),
            });
        }
        let total: u64 = jobs.iter().map(|j| j.size.unwrap()).sum();
        e.progress.plan_bytes(total);
        e.progress.begin_items(12);
        let report = e.dl().fetch_all(jobs.clone(), 4).await.unwrap();
        assert_eq!(
            report,
            Report {
                reused: 0,
                downloaded: 12
            }
        );
        let s = e.progress.snapshot();
        assert_eq!(
            (s.bytes_done, s.bytes_total, s.items_done),
            (total, total, 12)
        );
        let again = e.dl().fetch_all(jobs, 4).await.unwrap();
        assert_eq!(
            again,
            Report {
                reused: 12,
                downloaded: 0
            }
        );
    }

    #[tokio::test]
    async fn one_failure_stops_the_batch_and_keeps_what_was_verified() {
        let e = env();
        let good = body(200);
        e.net.add("https://libraries.minecraft.net/ok.jar", &good);
        let jobs = vec![
            FileJob {
                url: Url::parse("https://libraries.minecraft.net/ok.jar").unwrap(),
                dest: e.dir.path().join("ok.jar"),
                expected: sha1_of(&good),
                size: Some(200),
            },
            FileJob {
                url: Url::parse("https://libraries.minecraft.net/missing.jar").unwrap(),
                dest: e.dir.path().join("missing.jar"),
                expected: sha1_of(b"x"),
                size: Some(1),
            },
        ];
        assert!(e.dl().fetch_all(jobs, 1).await.is_err());
        assert_eq!(fs::read(e.dir.path().join("ok.jar")).unwrap(), good);
        assert!(!e.dir.path().join("missing.jar").exists());
    }

    #[tokio::test]
    async fn cancel_wakes_waiters_immediately_without_polling_delay() {
        let cancel = Cancel::new();
        let started = Arc::new(tokio::sync::Notify::new());
        let cancel_wait = cancel.clone();
        let started_wait = started.clone();

        let waiter = tokio::spawn(async move {
            let fut = cancel_wait.cancelled();
            tokio::pin!(fut);
            // Poll once so the waiter is parked on Notify before `cancel()` fires.
            let _ = futures::poll!(&mut fut);
            started_wait.notify_one();
            fut.await;
        });

        started.notified().await;
        let t0 = std::time::Instant::now();
        cancel.cancel();
        tokio::time::timeout(Duration::from_millis(25), waiter)
            .await
            .expect("cancel wakeup must be immediate, not 40ms-polled")
            .unwrap();
        assert!(t0.elapsed() < Duration::from_millis(25));
    }

    #[tokio::test]
    async fn enforces_max_undeclared_bytes_when_size_is_none() {
        struct OversizedUndeclaredStreamer;
        impl Streamer for OversizedUndeclaredStreamer {
            #[allow(clippy::manual_async_fn)]
            fn stream<'a>(
                &'a self,
                _url: &'a Url,
                _from: u64,
                sink: &'a mut dyn StreamSink,
            ) -> impl std::future::Future<Output = Result<(), FetchError>> + Send + 'a {
                async move {
                    sink.begin(&StreamHead {
                        status: 200,
                        content_length: Some(MAX_UNDECLARED_BYTES + 1),
                    })
                    .map_err(|e| FetchError::Local(e.to_string()))
                }
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let net = OversizedUndeclaredStreamer;
        let dl = Downloader::new(&net, Cancel::new(), Progress::new()).with_backoff(Duration::ZERO);
        let job = FileJob {
            url: Url::parse(URL).unwrap(),
            dest: dir.path().join("undeclared.jar"),
            expected: sha1_of(b"ok"),
            size: None,
        };
        assert!(matches!(
            dl.fetch(&job).await,
            Err(DownloadError::Verification(_))
        ));
        assert!(!job.dest.exists());
        assert!(!part_path(&job.dest).exists());

        // Also enforce ceiling in `PartWriter::chunk` when Content-Length is omitted.
        let part = dir.path().join("chunked.jar.part");
        File::create(&part).unwrap();
        let expected = sha1_of(b"ok");
        let progress = Progress::new();
        let outer = Cancel::new();
        let scope = CancelScope {
            outer: &outer,
            batch: None,
        };
        let mut credited = 0u64;
        let mut writer = PartWriter {
            part: &part,
            from: 0,
            limit: None,
            expected: &expected,
            len: 0,
            writer: None,
            hasher: None,
            progress: &progress,
            credited: &mut credited,
            cancel: &scope,
        };
        writer
            .begin(&StreamHead {
                status: 200,
                content_length: None,
            })
            .unwrap();
        writer.len = MAX_UNDECLARED_BYTES - 4;
        let err = writer.chunk(b"0123456789").unwrap_err();
        assert!(err.to_string().contains(OVERSIZE));
    }

    #[tokio::test]
    async fn inline_hash_verifies_fresh_downloads_and_disk_fallback_verifies_resumed_ones() {
        let e = env();
        let data = body(4096);
        let sha256_upper = hex::encode(sha2::Sha256::digest(&data)).to_ascii_uppercase();
        e.net.add(URL, &data);

        // Fresh download (`start == 0`): `PartWriter::finish` returns `Some((hex, len))` and
        // `Downloader::fetch` verifies SHA-256 inline without declared size.
        let fresh = FileJob {
            url: Url::parse(URL).unwrap(),
            dest: e.dir.path().join("nested/fresh.jar"),
            expected: Expected::Sha256(sha256_upper.clone()),
            size: None,
        };
        let dl = e.dl().with_root(e.dir.path());
        assert_eq!(dl.fetch(&fresh).await.unwrap(), Outcome::Downloaded);
        assert_eq!(fs::read(&fresh.dest).unwrap(), data);

        // Direct check on `PartWriter::finish`: `Some` on 200, `None` on 206 resume.
        let p200 = e.dir.path().join("direct200.part");
        let exp = Expected::Sha256(sha256_upper);
        let scope = CancelScope {
            outer: &e.cancel,
            batch: None,
        };
        let mut cred = 0u64;
        let mut pw_fresh = PartWriter {
            part: &p200,
            from: 0,
            limit: Some(data.len() as u64),
            expected: &exp,
            len: 0,
            writer: None,
            hasher: None,
            progress: &e.progress,
            credited: &mut cred,
            cancel: &scope,
        };
        pw_fresh
            .begin(&StreamHead {
                status: 200,
                content_length: Some(data.len() as u64),
            })
            .unwrap();
        pw_fresh.chunk(&data).unwrap();
        let inline = pw_fresh.finish().unwrap();
        assert!(inline.is_some());

        let mut cred2 = 0u64;
        fs::write(&p200, &data[..1000]).unwrap();
        let mut pw_resume = PartWriter {
            part: &p200,
            from: 1000,
            limit: Some(data.len() as u64),
            expected: &exp,
            len: 1000,
            writer: None,
            hasher: None,
            progress: &e.progress,
            credited: &mut cred2,
            cancel: &scope,
        };
        pw_resume
            .begin(&StreamHead {
                status: 206,
                content_length: Some((data.len() - 1000) as u64),
            })
            .unwrap();
        pw_resume.chunk(&data[1000..]).unwrap();
        assert!(
            pw_resume.finish().unwrap().is_none(),
            "206 resume must fall back to full-file disk verification"
        );

        // End-to-end 206 resume with tampered prefix on disk: disk fallback catches corruption,
        // discards `.part`, and re-downloads cleanly from zero.
        let resumed = e.job("resumed.jar", &data);
        fs::write(part_path(&resumed.dest), vec![0xAA; 1000]).unwrap();
        assert_eq!(dl.fetch(&resumed).await.unwrap(), Outcome::Downloaded);
        assert_eq!(fs::read(&resumed.dest).unwrap(), data);
    }

    #[tokio::test]
    async fn fetch_all_cancels_siblings_and_drains_active_tasks_before_returning_error() {
        struct SlowAndFailStreamer {
            slow_started: tokio::sync::Notify,
            calls: AtomicU64,
        }

        impl Streamer for SlowAndFailStreamer {
            #[allow(clippy::manual_async_fn)]
            fn stream<'a>(
                &'a self,
                url: &'a Url,
                _from: u64,
                sink: &'a mut dyn StreamSink,
            ) -> impl std::future::Future<Output = Result<(), FetchError>> + Send + 'a {
                async move {
                    self.calls.fetch_add(1, Ordering::SeqCst);
                    if url.as_str().ends_with("slow.jar") {
                        sink.begin(&StreamHead {
                            status: 200,
                            content_length: Some(1024),
                        })
                        .map_err(|e| FetchError::Local(e.to_string()))?;
                        sink.chunk(&[1u8; 64])
                            .map_err(|e| FetchError::Local(e.to_string()))?;
                        self.slow_started.notify_one();
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        Ok(())
                    } else if url.as_str().ends_with("fail.jar") {
                        self.slow_started.notified().await;
                        Err(FetchError::Status(404))
                    } else {
                        Ok(())
                    }
                }
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let streamer = SlowAndFailStreamer {
            slow_started: tokio::sync::Notify::new(),
            calls: AtomicU64::new(0),
        };
        let cancel = Cancel::new();
        let dl = Downloader::new(&streamer, cancel.clone(), Progress::new())
            .with_backoff(Duration::ZERO)
            .with_root(dir.path());

        let slow_dest = dir.path().join("a/slow.jar");
        let jobs = vec![
            FileJob {
                url: Url::parse("https://libraries.minecraft.net/slow.jar").unwrap(),
                dest: slow_dest.clone(),
                expected: sha1_of(&[1u8; 1024]),
                size: Some(1024),
            },
            FileJob {
                url: Url::parse("https://libraries.minecraft.net/fail.jar").unwrap(),
                dest: dir.path().join("b/fail.jar"),
                expected: sha1_of(b"x"),
                size: Some(1),
            },
            FileJob {
                url: Url::parse("https://libraries.minecraft.net/queued.jar").unwrap(),
                dest: dir.path().join("c/queued.jar"),
                expected: sha1_of(b"q"),
                size: Some(1),
            },
        ];

        let res = tokio::time::timeout(Duration::from_secs(2), dl.fetch_all(jobs, 2))
            .await
            .expect("fetch_all must cancel slow sibling and drain promptly");
        assert!(matches!(res, Err(DownloadError::Status(404))));
        assert!(
            !cancel.is_cancelled(),
            "batch failure must not poison caller's outer Cancel token"
        );
        assert_eq!(
            streamer.calls.load(Ordering::SeqCst),
            2,
            "queued jobs must not start after batch cancellation"
        );
        // Because `fetch_all` drained all in-flight tasks, `slow.jar.part`'s file handle is closed
        // and can be removed cleanly on Windows immediately upon return.
        let slow_part = part_path(&slow_dest);
        assert!(slow_part.exists());
        fs::remove_file(&slow_part).expect("part handle must be closed before fetch_all returns");
    }
}
