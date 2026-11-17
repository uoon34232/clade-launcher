//! Watching one running game: its log, its playtime and how it ended.
//!
//! [`supervise`] owns the child process from the moment it was spawned until it exits:
//! - every line of stdout/stderr goes to a session log under `logs/games/<instance>/`, with the
//!   secrets of the session replaced by `<redacted>` (a [`Redactor`]); a header records the
//!   redacted command, a footer the exit code and duration; logs are capped in size and only the
//!   newest few per instance are kept;
//! - the playtime session (begun right after a successful spawn by the caller) is ticked by a
//!   heartbeat while the process is alive and banked when it exits, so a crash still counts and
//!   launcher idle time never does;
//! - after a short grace period alive, `on_playing` is called once (the UI goes from "Launching"
//!   to "Playing"); a process that dies inside the grace period is reported as failing to start;
//! - when it exits, the last lines are classified into a short, human-readable [`ExitSummary`]
//!   (Java failed to start, wrong Java, missing class or library, native library failure, out
//!   of memory, a loader failure, a plain non-zero exit). The full detail stays in the log.
//!
//! The supervisor does not know about accounts, the UI or Tauri: it is driven in tests with
//! ordinary controlled processes.

use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc;

use crate::instances::InstanceId;
use crate::playtime::{PlaytimeStore, SessionTracker};
use crate::portable::PortableRoot;

/// A process that is still alive after this long counts as having started.
pub const GRACE: Duration = Duration::from_millis(2500);
/// A non-zero exit sooner than this after the start is "failed to start" rather than a crash.
const EARLY_EXIT: Duration = Duration::from_secs(10);
const MAX_LOG_BYTES: u64 = 24 * 1024 * 1024;
const KEEP_LOGS: usize = 10;
const TAIL_LINES: usize = 250;
const MAX_LINE_CHARS: usize = 4000;
const LINE_CHANNEL_CAPACITY: usize = 512;

// ------------------------------------------------------------------------------ redaction

/// Replaces secret values in text. Built from the session's secrets (the access token).
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: &[String]) -> Self {
        Self {
            // Longest first so a secret that contains another is hidden whole.
            secrets: {
                let mut s: Vec<String> = secrets.iter().filter(|s| s.len() >= 6).cloned().collect();
                s.sort_by_key(|s| std::cmp::Reverse(s.len()));
                s
            },
        }
    }

    pub fn apply<'a>(&self, line: &'a str) -> Cow<'a, str> {
        if !self.secrets.iter().any(|s| line.contains(s.as_str())) {
            return Cow::Borrowed(line);
        }
        let mut out = line.to_owned();
        for s in &self.secrets {
            if out.contains(s.as_str()) {
                out = out.replace(s.as_str(), "<redacted>");
            }
        }
        Cow::Owned(out)
    }
}

// ------------------------------------------------------------------------------ exit summary

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    /// Exit code 0.
    Normal,
    /// Java or the game stopped with an error after it had started.
    Crashed,
    /// It stopped almost at once: Java or the game never really started.
    FailedToStart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitSummary {
    pub code: Option<i32>,
    pub kind: ExitKind,
    /// One short English sentence for logs. The UI words the outcome from `reason` and `code`.
    pub message: String,
    /// Stable reason code (`closed`, `wrong_java`, `out_of_memory`, ..., `unexpected`).
    pub reason: &'static str,
    pub seconds: u64,
}

/// Known failure signatures, most specific first: (text to look for, reason code, English
/// sentence for the log). The UI words each reason in the selected language.
const SIGNATURES: &[(&str, &str, &str)] = &[
    ("UnsupportedClassVersionError", "wrong_java", "Minecraft or a mod needs a different Java version than the one Clade prepared. Repairing the profile may help."),
    ("Could not find or load main class", "incomplete_files", "Minecraft's files are incomplete. Repair the profile to download them again."),
    ("Could not reserve enough space", "no_memory", "There isn't enough free memory to start Minecraft. Close other programs and try again."),
    ("OutOfMemoryError", "out_of_memory", "Minecraft ran out of memory."),
    ("UnsatisfiedLinkError", "native_library", "A native library (graphics or sound) could not be loaded."),
    ("Failed to locate library", "native_library", "A native library (graphics or sound) could not be loaded."),
    ("EXCEPTION_ACCESS_VIOLATION", "java_crash", "The Java runtime crashed, often because of a graphics driver."),
    ("hs_err_pid", "java_crash", "The Java runtime crashed, often because of a graphics driver."),
    ("ModLoadingException", "mod_loader", "The mod loader reported an error while starting."),
    ("Mixin apply failed", "mixin", "A mod could not be applied. Check the mods in this profile."),
    ("Incompatible mod set", "incompatible_mods", "The mods in this profile don't work together."),
    ("NoClassDefFoundError", "missing_class", "A library or mod could not be loaded."),
    ("ClassNotFoundException", "missing_class", "A library or mod could not be loaded."),
    ("Error: Could not create the Java Virtual Machine", "jvm_start", "Java could not start with the chosen settings."),
];

pub fn summarize(status: Option<ExitStatus>, ran: Duration, tail: &[String]) -> ExitSummary {
    let code = status.and_then(|s| s.code());
    let seconds = ran.as_secs();
    if status.is_some_and(|s| s.success()) {
        return ExitSummary {
            code,
            kind: ExitKind::Normal,
            message: "Minecraft closed.".into(),
            reason: "closed",
            seconds,
        };
    }
    let text = tail.join(
        "
",
    );
    let known = SIGNATURES
        .iter()
        .find(|(needle, _, _)| text.contains(needle))
        .map(|(_, reason, m)| (*reason, (*m).to_owned()));
    let kind = if ran < EARLY_EXIT {
        ExitKind::FailedToStart
    } else {
        ExitKind::Crashed
    };
    let (reason, message) = known.unwrap_or_else(|| match (kind, code) {
        (ExitKind::FailedToStart, Some(c)) => (
            "early_exit",
            format!("Minecraft stopped right after starting (exit code {c})."),
        ),
        (_, Some(c)) => (
            "unexpected",
            format!("Minecraft stopped unexpectedly (exit code {c})."),
        ),
        (_, None) => ("stopped", "Minecraft was stopped.".to_owned()),
    });
    ExitSummary {
        code,
        kind,
        message,
        reason,
        seconds,
    }
}

// ------------------------------------------------------------------------------ logs

/// `logs/games/<instance>/`
pub fn logs_dir(root: &PortableRoot, id: &InstanceId) -> PathBuf {
    root.logs_dir().join("games").join(id.as_str())
}

fn prune(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    files.sort();
    let excess = files.len().saturating_sub(KEEP_LOGS);
    for old in files.into_iter().take(excess) {
        let _ = fs::remove_file(old);
    }
}

struct SessionLog {
    file: BufWriter<File>,
    written: u64,
    capped: bool,
}

impl SessionLog {
    fn open(dir: &Path, name: &str) -> std::io::Result<(Self, PathBuf)> {
        fs::create_dir_all(dir)?;
        prune(dir);
        let path = dir.join(name);
        Ok((
            Self {
                file: BufWriter::with_capacity(64 * 1024, File::create(&path)?),
                written: 0,
                capped: false,
            },
            path,
        ))
    }

    /// The command line header: written whole (it can be long), already redacted by caller.
    fn header(&mut self, clean: &str) {
        if writeln!(self.file, "{clean}").is_ok() {
            self.written += clean.len() as u64 + 1;
        }
    }

    fn line(&mut self, clean: &str) {
        if self.capped {
            return;
        }
        let clipped = if clean.len() <= MAX_LINE_CHARS {
            clean
        } else {
            match clean.char_indices().nth(MAX_LINE_CHARS) {
                Some((idx, _)) => &clean[..idx],
                None => clean,
            }
        };
        if self.written + clipped.len() as u64 > MAX_LOG_BYTES {
            let _ = writeln!(self.file, "[log truncated: size limit reached]");
            self.capped = true;
            return;
        }
        if writeln!(self.file, "{clipped}").is_ok() {
            self.written += clipped.len() as u64 + 1;
        }
    }

    fn flush(&mut self) {
        let _ = self.file.flush();
    }
}

// ------------------------------------------------------------------------------ supervising

pub struct Supervised {
    pub exit: ExitSummary,
    /// Seconds added to the playtime totals (`None` if no session was running).
    pub banked_seconds: Option<u64>,
    pub log_path: Option<PathBuf>,
}

async fn pump<R: AsyncRead + Unpin>(r: R, tx: mpsc::Sender<String>) {
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(l)) = lines.next_line().await {
        if tx.send(l).await.is_err() {
            break;
        }
    }
}

// ----------------------------------------------------------------------- the game's output file

/// Where the running games' console output is collected: launcher-owned files, never pipes.
///
/// A pipe ties the game to its reader: when Clade is killed the read end closes, the game's next write
/// fails, and the game stops by itself a little later. A file has no reader to lose, so the game keeps
/// running (and keeps its output) whatever happens to Clade. While Clade is alive it follows the file
/// ([`Watch::output`]) and turns it into the redacted session log.
pub fn output_dir(root: &PortableRoot) -> PathBuf {
    root.cache_dir().join("game-out")
}

/// How much game output may pile up in the output file while Clade is watching: once this much has been
/// read and the reader has caught up to EOF (or a runaway writer reaches twice this bound), the file is
/// emptied again (see [`tail_file`]). (The session log that people read has its own, larger cap.)
pub const OUTPUT_LIMIT_BYTES: u64 = 8 * 1024 * 1024;

/// Empties a file in place. Safe while a game has it open for appending: its next write simply goes to
/// the new end of the file.
fn empty_file(path: &Path) -> std::io::Result<()> {
    fs::OpenOptions::new().write(true).open(path)?.set_len(0)
}

/// Creates (empty) the output file for one instance's game and returns it with its path. The file is
/// handed to the game as both stdout and stderr.
///
/// It is opened for **appending**: every write of the game lands at the current end of the file,
/// wherever its own file position was. That is what makes it possible for Clade to empty the file
/// while the game keeps running (the bound above) without the game's next write leaving a gap.
pub fn open_output(root: &PortableRoot, id: &InstanceId) -> std::io::Result<(File, PathBuf)> {
    let dir = output_dir(root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.out"));
    // Start empty even if a previous (orphaned) game left output in the file.
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    let file = fs::OpenOptions::new().append(true).open(&path)?;
    Ok((file, path))
}

/// Empties output files left by games that ended while Clade was not watching, skipping any game
/// whose process is still alive (see [`PlaytimeStore::live_sessions`]).
pub fn sweep_output(root: &PortableRoot) {
    let live_ids: HashSet<String> = PlaytimeStore::new(root)
        .live_sessions()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, s)| s.instance_id.to_string())
        .collect();
    let Ok(entries) = fs::read_dir(output_dir(root)) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let is_live = path.extension().and_then(|x| x.to_str()) == Some("out")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|id| live_ids.contains(id));
        if !is_live {
            let _ = empty_file(&path);
        }
    }
}

/// Longest line kept whole; anything longer without a newline is cut here.
const MAX_PENDING_BYTES: usize = 64 * 1024;

/// Turns the bytes in `pending` into whole lines (lossy UTF-8, `\n` or `\r\n` ended); a partial last
/// line stays in `pending` unless it has grown too long.
async fn drain_lines(pending: &mut Vec<u8>, tx: &mpsc::Sender<String>) {
    let mut start = 0;
    while let Some(pos) = pending[start..].iter().position(|b| *b == b'\n') {
        let mut line = &pending[start..start + pos];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let text = String::from_utf8_lossy(line).into_owned();
        start += pos + 1;
        if tx.send(text).await.is_err() {
            pending.clear();
            return;
        }
    }
    pending.drain(..start);
    if pending.len() > MAX_PENDING_BYTES {
        let text = String::from_utf8_lossy(pending).into_owned();
        pending.clear();
        let _ = tx.send(text).await;
    }
}

/// Follows the game's output file and sends its lines until `done` says the game is gone, then reads
/// what is left (including a last line without a newline).
async fn tail_file(
    path: PathBuf,
    tx: mpsc::Sender<String>,
    mut done: tokio::sync::watch::Receiver<bool>,
) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return;
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    // Bytes read since the file was last emptied.
    let mut read_total: u64 = 0;
    let mut idle = Duration::from_millis(150);
    loop {
        let finished = *done.borrow();
        let n = file.read(&mut buf).await.unwrap_or(0);
        if n > 0 {
            idle = Duration::from_millis(150);
            read_total += n as u64;
            pending.extend_from_slice(&buf[..n]);
            drain_lines(&mut pending, &tx).await;
            // Bound the file: once enough has been read and we are caught up to exact EOF (or a
            // runaway writer hit twice the bound), empty it and read from the start again.
            let maybe_len = if read_total >= OUTPUT_LIMIT_BYTES {
                file.metadata().await.ok().map(|m| m.len())
            } else {
                None
            };
            if let Some(len) = maybe_len {
                let reset = if len < read_total {
                    file.seek(std::io::SeekFrom::Start(0)).await.is_ok()
                } else if len == read_total || len >= OUTPUT_LIMIT_BYTES * 2 {
                    empty_file(&path).is_ok()
                        && file.seek(std::io::SeekFrom::Start(0)).await.is_ok()
                } else {
                    false
                };
                if reset {
                    read_total = 0;
                }
            }
            continue;
        }
        if read_total > 0
            && file.metadata().await.is_ok_and(|m| m.len() < read_total)
            && file.seek(std::io::SeekFrom::Start(0)).await.is_ok()
        {
            read_total = 0;
            continue;
        }
        if finished {
            break;
        }
        tokio::select! {
            () = tokio::time::sleep(idle) => {
                idle = (idle * 2).min(Duration::from_millis(1500));
            }
            _ = done.changed() => {
                idle = Duration::from_millis(150);
            }
        }
    }
    if !pending.is_empty() {
        let _ = tx
            .send(String::from_utf8_lossy(&pending).into_owned())
            .await;
    }
}

pub struct Watch<'a> {
    pub root: &'a PortableRoot,
    pub instance: &'a InstanceId,
    /// Secret values to hide in the log (never written anywhere).
    pub secrets: &'a [String],
    /// The command line as already-redacted text, written at the top of the log.
    pub header: &'a str,
    /// The file the game writes its console output to (see `open_output`); followed while Clade runs.
    /// `None` for a child whose stdout/stderr are pipes.
    pub output: Option<&'a Path>,
    /// The playtime session, begun right after the successful spawn.
    pub tracker: Option<SessionTracker>,
    pub heartbeat: Duration,
    pub grace: Duration,
    pub now_unix: fn() -> i64,
}

/// Runs until the child exits. `on_playing` fires once if it is still alive after the grace period.
pub async fn supervise(mut child: Child, watch: Watch<'_>, on_playing: impl Fn()) -> Supervised {
    let started_ms = crate::playtime::unbiased_ms();
    let pid = child.id().unwrap_or(0);
    let (tx, mut rx) = mpsc::channel::<String>(LINE_CHANNEL_CAPACITY);
    let mut readers = Vec::new();
    if let Some(o) = child.stdout.take() {
        readers.push(tokio::spawn(pump(o, tx.clone())));
    }
    if let Some(e) = child.stderr.take() {
        readers.push(tokio::spawn(pump(e, tx.clone())));
    }
    let (done_tx, done_rx) = tokio::sync::watch::channel(false);
    if let Some(path) = watch.output {
        readers.push(tokio::spawn(tail_file(
            path.to_path_buf(),
            tx.clone(),
            done_rx,
        )));
    }
    drop(tx);

    let stamp = (watch.now_unix)();
    let redactor = Redactor::new(watch.secrets);
    let (mut log, log_path) = match SessionLog::open(
        &logs_dir(watch.root, watch.instance),
        &format!("{stamp}-{pid}.log"),
    ) {
        Ok((l, p)) => (Some(l), Some(p)),
        Err(e) => {
            tracing::warn!(error = %e, "could not open the game session log");
            (None, None)
        }
    };
    if let Some(l) = log.as_mut() {
        l.line(&format!(
            "Clade game session, instance {}, started at unix time {stamp}",
            watch.instance
        ));
        let clean_header = redactor.apply(watch.header);
        l.header(&format!("Command: {clean_header}"));
        l.line("---");
        l.flush();
    }

    let mut tracker = watch.tracker;
    let mut tail: VecDeque<String> = VecDeque::with_capacity(TAIL_LINES);
    let mut heartbeat = tokio::time::interval(watch.heartbeat);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await; // the first tick fires at once
    let grace = tokio::time::sleep(watch.grace);
    tokio::pin!(grace);
    let mut playing = false;
    let feed = |line: String, log: &mut Option<SessionLog>, tail: &mut VecDeque<String>| {
        let tail_line = match redactor.apply(&line) {
            Cow::Borrowed(_) => {
                if let Some(l) = log.as_mut() {
                    l.line(&line);
                }
                line
            }
            Cow::Owned(redacted) => {
                if let Some(l) = log.as_mut() {
                    l.line(&redacted);
                }
                redacted
            }
        };
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(tail_line);
    };

    let status = loop {
        tokio::select! {
            status = child.wait() => break status.ok(),
            Some(line) = rx.recv() => feed(line, &mut log, &mut tail),
            _ = heartbeat.tick() => {
                if let Some(l) = log.as_mut() {
                    l.flush();
                }
                if let Some(t) = tracker.as_mut() {
                    if let Err(e) = t.tick((watch.now_unix)(), Instant::now()) {
                        tracing::warn!(error = %e, "playtime heartbeat failed");
                    }
                }
            }
            () = &mut grace, if !playing => {
                playing = true;
                on_playing();
            }
        }
    };

    // The process is gone: drain what it printed last while readers finish, bounded by a timeout
    // in case a grandchild process kept a pipe handle open.
    let _ = done_tx.send(true);
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while let Ok(Some(line)) = tokio::time::timeout_at(drain_deadline, rx.recv()).await {
        feed(line, &mut log, &mut tail);
    }
    for r in readers {
        r.abort();
    }

    let ran = Duration::from_millis(crate::playtime::unbiased_ms().saturating_sub(started_ms));
    let banked_seconds = tracker.take().and_then(|t| {
        match t.finish(
            &PlaytimeStore::new(watch.root),
            (watch.now_unix)(),
            Instant::now(),
        ) {
            Ok(b) => Some(b.seconds),
            Err(e) => {
                tracing::warn!(error = %e, "could not bank the playtime session");
                None
            }
        }
    });

    let tail_vec: Vec<String> = tail.into_iter().collect();
    let exit = summarize(status, ran, &tail_vec);
    if let Some(l) = log.as_mut() {
        l.line("---");
        l.line(&format!(
            "Exited with code {} after {} s ({:?}). {}",
            exit.code.map_or("unknown".to_owned(), |c| c.to_string()),
            exit.seconds,
            exit.kind,
            exit.message
        ));
        l.flush();
    }
    Supervised {
        exit,
        banked_seconds,
        log_path,
    }
}

#[cfg(test)]
mod tests;
