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

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use url::Url;

use crate::net::{FetchError, StreamHead, StreamSink, Streamer};
use crate::portable::fsutil::{
    is_link, reject_links_between, remove_file_with_retry, rename_with_retry,
    strip_prefix_portable, verify_file, Expected,
};

/// Failed attempts tolerated per file before giving up on the connection.
const MAX_TRANSIENT: u32 = 3;
/// Corrupt results tolerated per file before reporting a verification failure.
const MAX_CORRUPT: u32 = 2;
/// Hard ceiling when a job carries no declared metadata size (`job.size` is `None`).
const MAX_UNDECLARED_BYTES: u64 = 512 * 1024 * 1024;
/// Write buffer size per active download stream to coalesce small network chunks on USB flash drives.
const PART_BUF_CAPACITY: usize = 256 * 1024;
/// Maximum queued 256 KiB buffers per active download stream before applying backpressure.
const MAX_WRITER_QUEUE: usize = 8;

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

struct WriterState {
    queue: VecDeque<Vec<u8>>,
    finish: bool,
    abort: bool,
    error: Option<(io::ErrorKind, String)>,
}

struct WriterShared {
    state: Mutex<WriterState>,
    work_available: Condvar,
    space_available: Condvar,
}

struct WriterHandle {
    shared: Arc<WriterShared>,
    done_rx: Option<tokio::sync::oneshot::Receiver<io::Result<()>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WriterHandle {
    fn spawn(part: &Path, resuming: bool, from: u64) -> io::Result<Self> {
        let shared = Arc::new(WriterShared {
            state: Mutex::new(WriterState {
                queue: VecDeque::new(),
                finish: false,
                abort: false,
                error: None,
            }),
            work_available: Condvar::new(),
            space_available: Condvar::new(),
        });
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let part_owned = part.to_path_buf();
        let shared_thread = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("clade-part-writer".into())
            .spawn(move || {
                let res = write_part_file(&part_owned, resuming, from, &shared_thread);
                if let Err(ref e) = res {
                    let mut g = shared_thread
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    g.error = Some((e.kind(), e.to_string()));
                    shared_thread.space_available.notify_all();
                }
                drop(shared_thread);
                let _ = done_tx.send(res);
            })?;
        Ok(Self {
            shared,
            done_rx: Some(done_rx),
            thread: Some(thread),
        })
    }

    fn check_error(&self) -> io::Result<()> {
        let g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((kind, ref msg)) = g.error {
            return Err(io::Error::new(kind, msg.clone()));
        }
        Ok(())
    }

    fn push_buffer(&self, buf: Vec<u8>, cancel: &CancelScope<'_>) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let mut g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some((kind, ref msg)) = g.error {
                return Err(io::Error::new(kind, msg.clone()));
            }
            if g.abort || cancel.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            if g.queue.len() < MAX_WRITER_QUEUE {
                g.queue.push_back(buf);
                self.shared.work_available.notify_one();
                return Ok(());
            }
            let (next_g, _) = self
                .shared
                .space_available
                .wait_timeout(g, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner());
            g = next_g;
        }
    }

    async fn finish(mut self, tail: Vec<u8>) -> io::Result<()> {
        {
            let mut g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if !tail.is_empty() {
                g.queue.push_back(tail);
            }
            g.finish = true;
            self.shared.work_available.notify_one();
        }
        let done_rx = self
            .done_rx
            .take()
            .ok_or_else(|| io::Error::other("part writer completion channel missing"))?;
        match done_rx.await {
            Ok(res) => res,
            Err(_) => Err(io::Error::other("part writer terminated unexpectedly")),
        }
    }

    async fn abort(mut self, flush_pending: bool, tail: Vec<u8>) {
        {
            let mut g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if flush_pending {
                if !tail.is_empty() {
                    g.queue.push_back(tail);
                }
                g.finish = true;
            } else {
                g.queue.clear();
                g.abort = true;
            }
            self.shared.work_available.notify_one();
        }
        if let Some(done_rx) = self.done_rx.take() {
            let _ = done_rx.await;
        }
    }
}

impl Drop for WriterHandle {
    fn drop(&mut self) {
        if self.done_rx.take().is_some() {
            {
                let mut g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
                g.queue.clear();
                g.abort = true;
                self.shared.work_available.notify_one();
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

fn write_part_file(
    part: &Path,
    resuming: bool,
    from: u64,
    shared: &WriterShared,
) -> io::Result<()> {
    let mut file = if resuming {
        let mut f = OpenOptions::new().write(true).open(part)?;
        f.seek(SeekFrom::Start(from))?;
        f.set_len(from)?;
        f
    } else {
        File::create(part)?
    };

    loop {
        let next = {
            let mut g = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if g.abort {
                    return Ok(());
                }
                if let Some(buf) = g.queue.pop_front() {
                    shared.space_available.notify_one();
                    break Some(buf);
                }
                if g.finish {
                    break None;
                }
                g = shared
                    .work_available
                    .wait(g)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some(buf) = next else {
            break;
        };
        file.write_all(&buf)?;
    }
    file.flush()?;
    drop(file);
    Ok(())
}

/// Writes the stream into the `.part` file via a dedicated blocking writer thread, resuming or
/// restarting as the server answered. When the file's size is known it is a hard ceiling for the
/// whole part (bytes already kept from an earlier attempt count towards it); otherwise
/// [`MAX_UNDECLARED_BYTES`] caps the stream.
struct PartWriter<'a> {
    part: &'a Path,
    from: u64,
    limit: Option<u64>,
    expected: &'a Expected,
    /// Bytes the part holds (kept from before plus written now).
    len: u64,
    pending: Vec<u8>,
    worker: Option<WriterHandle>,
    hasher: Option<InlineHasher>,
    progress: &'a Progress,
    credited: &'a mut u64,
    cancel: &'a CancelScope<'a>,
}

impl<'a> PartWriter<'a> {
    fn new(
        part: &'a Path,
        from: u64,
        limit: Option<u64>,
        expected: &'a Expected,
        progress: &'a Progress,
        credited: &'a mut u64,
        cancel: &'a CancelScope<'a>,
    ) -> Self {
        Self {
            part,
            from,
            limit,
            expected,
            len: from,
            pending: Vec::new(),
            worker: None,
            hasher: None,
            progress,
            credited,
            cancel,
        }
    }

    fn flush_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let Some(worker) = self.worker.as_ref() else {
            return Err(io::Error::other("data before headers"));
        };
        let buf = std::mem::replace(&mut self.pending, Vec::with_capacity(PART_BUF_CAPACITY));
        worker.push_buffer(buf, self.cancel)
    }

    fn enqueue_bytes(&mut self, data: &[u8]) -> io::Result<()> {
        let mut rest = data;
        while !rest.is_empty() {
            let space = PART_BUF_CAPACITY.saturating_sub(self.pending.len());
            if space == 0 {
                self.flush_pending()?;
                continue;
            }
            let take = rest.len().min(space);
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() >= PART_BUF_CAPACITY {
                self.flush_pending()?;
            }
        }
        if let Some(worker) = self.worker.as_ref() {
            worker.check_error()?;
        }
        Ok(())
    }

    /// Flushes buffered bytes on the background writer thread, closes the underlying file handle,
    /// and returns the inline `(hex_digest, len)` when streamed from byte zero (`None` when resumed
    /// from `start > 0`).
    async fn finish(mut self) -> io::Result<Option<(String, u64)>> {
        let Some(worker) = self.worker.take() else {
            return Err(io::Error::other("response completed without headers"));
        };
        let tail = std::mem::take(&mut self.pending);
        worker.finish(tail).await?;
        Ok(self.hasher.take().map(|h| (h.finalize_hex(), self.len)))
    }

    /// Signals the writer thread to stop and awaits handle closure without blocking Tokio workers.
    async fn abort(mut self, flush_pending: bool) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let tail = if flush_pending {
            std::mem::take(&mut self.pending)
        } else {
            self.pending.clear();
            Vec::new()
        };
        worker.abort(flush_pending, tail).await;
    }
}

impl StreamSink for PartWriter<'_> {
    fn begin(&mut self, head: &StreamHead) -> io::Result<()> {
        if self.worker.is_some() {
            return Err(io::Error::other("response headers received twice"));
        }
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
            self.hasher = None;
            self.progress.set_credit(self.credited, self.from);
        } else {
            // Fresh download or the server ignored the range and sent 200: start over and hash inline.
            self.hasher = Some(InlineHasher::for_expected(self.expected));
            self.progress.set_credit(self.credited, 0);
        }
        self.worker = Some(WriterHandle::spawn(self.part, resuming, self.from)?);
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
        if self.worker.is_none() {
            return Err(io::Error::other("data before headers"));
        }
        self.enqueue_bytes(data)?;
        if let Some(h) = self.hasher.as_mut() {
            h.update(data);
        }
        self.len += data.len() as u64;
        self.progress.add_done(data.len() as u64);
        *self.credited += data.len() as u64;
        Ok(())
    }
}

fn leaf_label(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "path".to_owned())
}

fn sanitize_link_error(root: Option<&Path>, err: io::Error) -> io::Error {
    let msg = err.to_string();
    let Some(raw) = msg.strip_prefix("refusing to follow link at ") else {
        return err;
    };
    let p = Path::new(raw);
    let label = match root.and_then(|base| strip_prefix_portable(p, base)) {
        Some(rel) if rel.as_os_str().is_empty() => "root".to_owned(),
        Some(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        None => leaf_label(p),
    };
    io::Error::new(err.kind(), format!("refusing to follow link at {label}"))
}

fn reject_if_link(p: &Path) -> io::Result<()> {
    match is_link(p) {
        Ok(true) => Err(io::Error::other(format!(
            "refusing to follow link at {}",
            leaf_label(p)
        ))),
        Ok(false) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn ends_with_dot_segment(p: &Path) -> bool {
    let bytes = p.as_os_str().as_encoded_bytes();
    bytes == b"." || bytes.ends_with(b"/.") || bytes.ends_with(b"\\.")
}

fn validate_job_dest_syntax(root: Option<&Path>, dest: &Path) -> io::Result<()> {
    if dest.as_os_str().is_empty() || dest.file_name().is_none() || ends_with_dot_segment(dest) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination path has no file name",
        ));
    }
    if let Some(base) = root {
        let rest = strip_prefix_portable(dest, base).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination is outside the base directory",
            )
        })?;
        if rest.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination cannot be the base directory",
            ));
        }
        for comp in rest.components() {
            if !matches!(comp, Component::Normal(_)) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination contains unexpected path components",
                ));
            }
        }
    }
    Ok(())
}

fn path_collision_key(path: &Path) -> String {
    let clean = crate::portable::root::normalize_verbatim_path(path.to_path_buf());
    let mut parts: Vec<String> = Vec::new();
    for comp in clean.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if parts
                    .last()
                    .is_some_and(|p| p != "/" && p != ".." && !p.starts_with("prefix:"))
                {
                    parts.pop();
                } else {
                    parts.push("..".to_owned());
                }
            }
            Component::RootDir => parts.push("/".to_owned()),
            Component::Prefix(p) => {
                let s = p.as_os_str().to_string_lossy();
                let folded = if cfg!(windows) {
                    s.to_lowercase()
                } else {
                    s.into_owned()
                };
                parts.push(format!("prefix:{folded}"));
            }
            Component::Normal(c) => {
                let s = c.to_string_lossy();
                let folded = if cfg!(windows) {
                    s.to_lowercase()
                } else {
                    s.into_owned()
                };
                parts.push(folded);
            }
        }
    }
    parts.join("\0")
}

/// Groups jobs into sequential per-destination lanes (case-insensitively on Windows) and rejects
/// conflicting destinations or `dest`/`part_path` collisions before any concurrent stream runs.
fn group_jobs_for_fetch(jobs: Vec<FileJob>) -> Result<Vec<Vec<FileJob>>, DownloadError> {
    let mut lanes_by_dest: HashMap<String, usize> = HashMap::with_capacity(jobs.len());
    let mut part_keys: HashSet<String> = HashSet::with_capacity(jobs.len());
    let mut lanes: Vec<Vec<FileJob>> = Vec::with_capacity(jobs.len());

    for job in jobs {
        let dest_key = path_collision_key(&job.dest);
        let part_key = path_collision_key(&part_path(&job.dest));
        if let Some(&idx) = lanes_by_dest.get(&dest_key) {
            let head = &lanes[idx][0];
            if head.expected != job.expected || head.size != job.size {
                return Err(DownloadError::Local(
                    "conflicting download jobs target the same destination".into(),
                ));
            }
            lanes[idx].push(job);
        } else {
            let idx = lanes.len();
            lanes_by_dest.insert(dest_key, idx);
            part_keys.insert(part_key);
            lanes.push(vec![job]);
        }
    }

    if part_keys.iter().any(|pk| lanes_by_dest.contains_key(pk)) {
        return Err(DownloadError::Local(
            "conflicting download jobs collide with a partial download path".into(),
        ));
    }

    Ok(lanes)
}

fn ensure_parent_safe(root: Option<&Path>, parent: &Path) -> io::Result<()> {
    if let Some(base) = root {
        reject_links_between(base, parent).map_err(|e| sanitize_link_error(Some(base), e))?;
        fs::create_dir_all(parent)?;
        reject_links_between(base, parent).map_err(|e| sanitize_link_error(Some(base), e))?;
    } else {
        let mut existing = parent;
        while fs::symlink_metadata(existing).is_err() {
            match existing.parent() {
                Some(p) if !p.as_os_str().is_empty() => existing = p,
                _ => break,
            }
        }
        reject_links_between(existing, parent).map_err(|e| sanitize_link_error(None, e))?;
        fs::create_dir_all(parent)?;
        reject_links_between(existing, parent).map_err(|e| sanitize_link_error(None, e))?;
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

    async fn promote_part(
        &self,
        part: &Path,
        dest: &Path,
        known_len: Option<u64>,
        size: Option<u64>,
        credited: &mut u64,
    ) -> Result<Outcome, DownloadError> {
        let part_p = part.to_path_buf();
        let dest_p = dest.to_path_buf();
        let root_p = self.root.clone();
        let len = tokio::task::spawn_blocking(move || -> io::Result<u64> {
            if let Some(base) = root_p.as_deref() {
                reject_links_between(base, &dest_p)
                    .map_err(|e| sanitize_link_error(Some(base), e))?;
            }
            reject_if_link(&dest_p)?;
            reject_if_link(&part_p)?;
            let len = match known_len {
                Some(n) => n,
                None => fs::metadata(&part_p)?.len(),
            };
            rename_with_retry(&part_p, &dest_p)?;
            Ok(len)
        })
        .await
        .map_err(|e| DownloadError::Local(e.to_string()))?
        .map_err(|e| DownloadError::Local(e.to_string()))?;
        self.progress.set_credit(credited, size.unwrap_or(len));
        self.progress.item_done();
        Ok(Outcome::Downloaded)
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
        validate_job_dest_syntax(self.root.as_deref(), &job.dest).map_err(local)?;
        let mut credited = 0u64;
        let part = part_path(&job.dest);

        let dest_init = job.dest.clone();
        let part_init = part.clone();
        let expected_init = job.expected.clone();
        let size_init = job.size;
        let root_init = self.root.clone();
        let reuse_len = tokio::task::spawn_blocking(move || -> io::Result<Option<u64>> {
            if !parents_ready {
                if let Some(parent) = dest_init.parent().filter(|p| !p.as_os_str().is_empty()) {
                    ensure_parent_safe(root_init.as_deref(), parent)?;
                }
            }
            if let Some(base) = root_init.as_deref() {
                reject_links_between(base, &dest_init)
                    .map_err(|e| sanitize_link_error(Some(base), e))?;
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
                let mut sink = PartWriter::new(
                    &part,
                    from,
                    job.size,
                    &job.expected,
                    &self.progress,
                    &mut credited,
                    scope,
                );
                let result = tokio::select! {
                    r = self.streamer.stream(&job.url, from, &mut sink) => r,
                    () = scope.cancelled() => {
                        sink.abort(true).await;
                        return Err(DownloadError::Cancelled);
                    }
                };
                match result {
                    Ok(()) => {
                        inline_verified = sink.finish().await.map_err(local)?;
                    }
                    Err(FetchError::Local(m)) if m.contains(OVERSIZE) => {
                        sink.abort(false).await;
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
                        let cancelled = scope.is_cancelled();
                        sink.abort(cancelled).await;
                        return Err(if cancelled {
                            DownloadError::Cancelled
                        } else {
                            DownloadError::Local(m)
                        });
                    }
                    Err(FetchError::Policy(e)) => {
                        sink.abort(false).await;
                        return Err(DownloadError::Policy(e.to_string()));
                    }
                    Err(FetchError::TooLarge(_)) => {
                        sink.abort(false).await;
                        return Err(DownloadError::Local("unexpected size limit".into()));
                    }
                    Err(FetchError::Status(416)) => {
                        sink.abort(false).await;
                        if from > 0
                            && Self::verifies(part.clone(), job.expected.clone(), job.size).await
                        {
                            return self
                                .promote_part(&part, &job.dest, None, job.size, &mut credited)
                                .await;
                        }
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
                        let cancelled = scope.is_cancelled();
                        sink.abort(true).await;
                        if cancelled {
                            return Err(DownloadError::Cancelled);
                        }
                        transient += 1;
                        if transient > MAX_TRANSIENT {
                            return Err(DownloadError::Status(code));
                        }
                        self.pause(transient, scope).await?;
                        continue;
                    }
                    Err(FetchError::Status(code)) => {
                        sink.abort(false).await;
                        return Err(DownloadError::Status(code));
                    }
                    Err(FetchError::Network(m)) => {
                        let cancelled = scope.is_cancelled();
                        sink.abort(true).await;
                        if cancelled {
                            return Err(DownloadError::Cancelled);
                        }
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
                    return self
                        .promote_part(&part, &job.dest, Some(len), job.size, &mut credited)
                        .await;
                }
            } else if Self::verifies(part.clone(), job.expected.clone(), job.size).await {
                return self
                    .promote_part(&part, &job.dest, None, job.size, &mut credited)
                    .await;
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

        for job in &jobs {
            validate_job_dest_syntax(self.root.as_deref(), &job.dest)
                .map_err(|e| DownloadError::Local(e.to_string()))?;
        }
        let lanes = group_jobs_for_fetch(jobs)?;

        let mut parents = HashSet::new();
        for lane in &lanes {
            if let Some(parent) = lane[0].dest.parent().filter(|p| !p.as_os_str().is_empty()) {
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
        let mut results = stream::iter(lanes)
            .map(|lane| {
                let scope = CancelScope {
                    outer: &self.cancel,
                    batch: Some(&batch_cancel),
                };
                async move {
                    let mut outcomes = Vec::with_capacity(lane.len());
                    for job in lane {
                        outcomes.push(self.fetch_with_scope(&job, &scope, true).await?);
                    }
                    Ok::<Vec<Outcome>, DownloadError>(outcomes)
                }
            })
            .buffer_unordered(concurrency.clamp(1, 32));
        let mut first_err: Option<DownloadError> = None;
        while let Some(r) = results.next().await {
            match r {
                Ok(outcomes) => {
                    for outcome in outcomes {
                        match outcome {
                            Outcome::Reused => report.reused += 1,
                            Outcome::Downloaded => report.downloaded += 1,
                        }
                    }
                }
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

        let mut waiter = tokio::spawn(async move {
            let fut = cancel_wait.cancelled();
            tokio::pin!(fut);
            // Poll once so the waiter is parked on Notify before `cancel()` fires.
            let _ = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(std::future::Future::poll(fut.as_mut(), cx))
            })
            .await;
            started_wait.notify_one();
            fut.await;
        });

        started.notified().await;
        assert!(std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(
                std::pin::Pin::new(&mut waiter),
                cx,
            ))
        })
        .await
        .is_pending());
        cancel.cancel();
        // On the single-threaded test runtime, a Notify wakeup is ready after one cooperative
        // yield without advancing any timer wheel.
        tokio::task::yield_now().await;
        assert!(std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(
                std::pin::Pin::new(&mut waiter),
                cx,
            ))
        })
        .await
        .is_ready());
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
        let mut writer =
            PartWriter::new(&part, 0, None, &expected, &progress, &mut credited, &scope);
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
        let mut pw_fresh = PartWriter::new(
            &p200,
            0,
            Some(data.len() as u64),
            &exp,
            &e.progress,
            &mut cred,
            &scope,
        );
        pw_fresh
            .begin(&StreamHead {
                status: 200,
                content_length: Some(data.len() as u64),
            })
            .unwrap();
        pw_fresh.chunk(&data).unwrap();
        let inline = pw_fresh.finish().await.unwrap();
        assert!(inline.is_some());

        let mut cred2 = 0u64;
        fs::write(&p200, &data[..1000]).unwrap();
        let mut pw_resume = PartWriter::new(
            &p200,
            1000,
            Some(data.len() as u64),
            &exp,
            &e.progress,
            &mut cred2,
            &scope,
        );
        pw_resume
            .begin(&StreamHead {
                status: 206,
                content_length: Some((data.len() - 1000) as u64),
            })
            .unwrap();
        pw_resume.chunk(&data[1000..]).unwrap();
        assert!(
            pw_resume.finish().await.unwrap().is_none(),
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
