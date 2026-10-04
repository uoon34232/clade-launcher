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

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use url::Url;

use crate::net::{FetchError, StreamHead, StreamSink, Streamer};
use crate::portable::fsutil::{verify_file, Expected};

/// Failed attempts tolerated per file before giving up on the connection.
const MAX_TRANSIENT: u32 = 3;
/// Corrupt results tolerated per file before reporting a verification failure.
const MAX_CORRUPT: u32 = 2;

// ------------------------------------------------------------------------------ cancel

/// A cancellation switch shared between the UI command and every running download.
#[derive(Debug, Default)]
pub struct Cancel {
    flag: AtomicBool,
}

impl Cancel {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    /// Resolves once cancelled.
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(40)).await;
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

/// Writes the stream into the `.part` file, resuming or restarting as the server answered.
struct PartWriter<'a> {
    part: &'a Path,
    from: u64,
    file: Option<File>,
    progress: &'a Progress,
    credited: &'a mut u64,
    cancel: &'a Cancel,
}

impl StreamSink for PartWriter<'_> {
    fn begin(&mut self, head: &StreamHead) -> io::Result<()> {
        if head.status == 206 {
            let mut f = OpenOptions::new().write(true).open(self.part)?;
            f.seek(SeekFrom::Start(self.from))?;
            f.set_len(self.from)?;
            self.file = Some(f);
            self.progress.set_credit(self.credited, self.from);
        } else {
            // The server sent the whole file (it ignored the range): start the part over.
            self.file = Some(File::create(self.part)?);
            self.progress.set_credit(self.credited, 0);
        }
        Ok(())
    }

    fn chunk(&mut self, data: &[u8]) -> io::Result<()> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let f = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("data before headers"))?;
        f.write_all(data)?;
        self.progress.add_done(data.len() as u64);
        *self.credited += data.len() as u64;
        Ok(())
    }
}

// ------------------------------------------------------------------------------ downloader

pub struct Downloader<'a, S: Streamer> {
    streamer: &'a S,
    cancel: Arc<Cancel>,
    progress: Arc<Progress>,
    backoff: Duration,
}

impl<'a, S: Streamer> Downloader<'a, S> {
    pub fn new(streamer: &'a S, cancel: Arc<Cancel>, progress: Arc<Progress>) -> Self {
        Self {
            streamer,
            cancel,
            progress,
            backoff: Duration::from_millis(400),
        }
    }

    /// Base delay between retries (it triples per attempt). Tests use zero.
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
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

    async fn pause(&self, attempt: u32) -> Result<(), DownloadError> {
        let wait = self.backoff * 3u32.saturating_pow(attempt.saturating_sub(1));
        if wait.is_zero() {
            return Ok(());
        }
        tokio::select! {
            () = tokio::time::sleep(wait) => Ok(()),
            () = self.cancel.cancelled() => Err(DownloadError::Cancelled),
        }
    }

    /// Makes `job.dest` exist and verify, downloading only what is needed.
    pub async fn fetch(&self, job: &FileJob) -> Result<Outcome, DownloadError> {
        let local = |e: io::Error| DownloadError::Local(e.to_string());
        let mut credited = 0u64;

        if job.dest.is_file()
            && Self::verifies(job.dest.clone(), job.expected.clone(), job.size).await
        {
            let len = fs::metadata(&job.dest).map(|m| m.len()).unwrap_or(0);
            self.progress
                .set_credit(&mut credited, job.size.unwrap_or(len));
            self.progress.item_done();
            return Ok(Outcome::Reused);
        }

        if let Some(parent) = job.dest.parent() {
            fs::create_dir_all(parent).map_err(local)?;
        }
        let part = part_path(&job.dest);
        let (mut transient, mut corrupt) = (0u32, 0u32);

        loop {
            if self.cancel.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            let mut from = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if job.size.is_some_and(|s| from > s) {
                let _ = fs::remove_file(&part); // longer than the file can be: garbage
                from = 0;
            }
            let already_whole = from > 0 && job.size == Some(from);
            if !already_whole {
                if from == 0 {
                    let _ = fs::remove_file(&part);
                    File::create(&part).map_err(local)?;
                }
                let mut sink = PartWriter {
                    part: &part,
                    from,
                    file: None,
                    progress: &self.progress,
                    credited: &mut credited,
                    cancel: &self.cancel,
                };
                let result = tokio::select! {
                    r = self.streamer.stream(&job.url, from, &mut sink) => r,
                    () = self.cancel.cancelled() => return Err(DownloadError::Cancelled),
                };
                drop(sink);
                match result {
                    Ok(()) => {}
                    Err(FetchError::Local(m)) => {
                        return Err(if self.cancel.is_cancelled() {
                            DownloadError::Cancelled
                        } else {
                            DownloadError::Local(m)
                        })
                    }
                    Err(FetchError::Policy(e)) => return Err(DownloadError::Policy(e.to_string())),
                    Err(FetchError::TooLarge(_)) => {
                        return Err(DownloadError::Local("unexpected size limit".into()))
                    }
                    Err(FetchError::Status(416)) => {
                        // The part is not a prefix of the file any more: start over.
                        let _ = fs::remove_file(&part);
                        self.progress.set_credit(&mut credited, 0);
                        corrupt += 1;
                        if corrupt > MAX_CORRUPT {
                            return Err(DownloadError::Status(416));
                        }
                        continue;
                    }
                    Err(FetchError::Status(code)) if code == 408 || code == 429 || code >= 500 => {
                        transient += 1;
                        if transient > MAX_TRANSIENT {
                            return Err(DownloadError::Status(code));
                        }
                        self.pause(transient).await?;
                        continue;
                    }
                    Err(FetchError::Status(code)) => return Err(DownloadError::Status(code)),
                    Err(FetchError::Network(m)) => {
                        transient += 1;
                        if transient > MAX_TRANSIENT {
                            return Err(DownloadError::Offline(m));
                        }
                        self.pause(transient).await?;
                        continue;
                    }
                }
            }

            // A part is only ever promoted after it verifies in full.
            if Self::verifies(part.clone(), job.expected.clone(), job.size).await {
                let len = fs::metadata(&part).map_err(local)?.len();
                fs::rename(&part, &job.dest).map_err(local)?;
                self.progress
                    .set_credit(&mut credited, job.size.unwrap_or(len));
                self.progress.item_done();
                return Ok(Outcome::Downloaded);
            }
            let _ = fs::remove_file(&part);
            self.progress.set_credit(&mut credited, 0);
            corrupt += 1;
            if corrupt > MAX_CORRUPT {
                let name = job.dest.file_name().unwrap_or_default().to_string_lossy();
                return Err(DownloadError::Verification(name.into_owned()));
            }
        }
    }

    /// Fetches many files with at most `concurrency` in flight; stops at the first failure
    /// (in-flight parts are kept for resuming).
    pub async fn fetch_all(
        &self,
        jobs: Vec<FileJob>,
        concurrency: usize,
    ) -> Result<Report, DownloadError> {
        let mut report = Report::default();
        let mut results = stream::iter(jobs)
            .map(|job| async move { self.fetch(&job).await })
            .buffer_unordered(concurrency.clamp(1, 32));
        while let Some(r) = results.next().await {
            match r? {
                Outcome::Reused => report.reused += 1,
                Outcome::Downloaded => report.downloaded += 1,
            }
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
}
