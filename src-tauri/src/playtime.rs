//! Playtime tracking: how long Minecraft itself actually ran.
//!
//! What counts: only the lifetime of a launched game process. Time spent in the launcher
//! (idle, browsing instances, downloading) never counts, because a session only exists between
//! "game process started" and "game process exited".
//!
//! Storage (all inside the portable folder):
//! - `instances/<id>/playtime.json`: total for that instance. It lives in the instance folder,
//!   so copying an instance to another launcher folder keeps its history, and deleting an
//!   instance deletes its statistics together with it.
//! - `data/playtime.json`: lifetime total across all instances. Kept separately so deleting an
//!   instance does not erase time that was really played.
//! - `data/playtime-active/<session>.json`: the open session, refreshed by a heartbeat. It is the
//!   crash-recovery journal.
//!
//! Accuracy and crash recovery
//! - Durations come from a monotonic clock (`Instant`), never from comparing wall-clock
//!   timestamps, so changing the system clock cannot inflate or erase playtime.
//! - The tracker is ticked every [`HEARTBEAT`]. A tick that arrives after more than
//!   [`MAX_TICK_GAP`] (PC slept or hibernated with the game open, launcher process frozen)
//!   counts only one heartbeat interval instead of the whole gap.
//! - If the launcher or PC dies mid-session, the journal still holds the time up to the last
//!   persisted heartbeat. [`PlaytimeStore::recover`] runs at startup and banks that much.
//! - Banking a finished session is idempotent: each total remembers recently applied session
//!   ids, so a crash between the two file updates cannot double count on recovery.
//!
//! Known limit: if the launcher stops while the game keeps running, time after that point is not
//! counted (the launcher is what observes the process). The loss is therefore not bounded by the
//! heartbeat interval: it can be the whole session if the launcher stopped before its first tick.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::instances::InstanceId;
use crate::portable::fsutil::{atomic_write, quarantine_corrupt, remove_file_with_retry};
use crate::portable::PortableRoot;

static BANK_LOCK: Mutex<()> = Mutex::new(());

fn fallback_monotonic_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Monotonic milliseconds excluding time spent in Windows sleep or hibernation
/// (`QueryUnbiasedInterruptTime`).
pub fn unbiased_ms() -> u64 {
    use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;
    let mut ticks_100ns: u64 = 0;
    // SAFETY: `ticks_100ns` is a valid aligned `u64` out-pointer for the duration of the call.
    if unsafe { QueryUnbiasedInterruptTime(&mut ticks_100ns) } != 0 {
        return ticks_100ns / 10_000;
    }
    fallback_monotonic_ms()
}

/// Handle to a verified OS process matching an [`ActiveSession`]'s PID and creation timestamp.
pub(crate) struct LiveProcess(std::os::windows::io::OwnedHandle);

impl LiveProcess {
    /// Opens `pid` with `PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE` and verifies its
    /// OS creation timestamp is within 120 seconds of `started_at` (defending against PID reuse).
    /// Returns `(handle, still_running)`.
    pub(crate) fn open_matching(pid: u32, started_at: i64) -> Option<(Self, bool)> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{FILETIME, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE,
        };

        if pid == 0 {
            return None;
        }
        // SAFETY: `OpenProcess` takes plain integer arguments and returns a null handle on error.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return None;
        }
        // SAFETY: `raw` is a fresh non-null process handle owned exclusively by `owned`, which
        // calls `CloseHandle` automatically on every exit path.
        let owned = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
        let h = owned.as_raw_handle().cast();

        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut user = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        // SAFETY: `h` is valid with `PROCESS_QUERY_LIMITED_INFORMATION` and all `FILETIME`
        // out-pointers are valid stack locals.
        if unsafe { GetProcessTimes(h, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
            return None;
        }
        const WINDOWS_TO_UNIX_EPOCH_SECS: u64 = 11_644_473_600;
        let ticks_100ns =
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        let created_secs = (ticks_100ns / 10_000_000).checked_sub(WINDOWS_TO_UNIX_EPOCH_SECS)?;
        let created_unix = i64::try_from(created_secs).ok()?;
        if created_unix.abs_diff(started_at) > 120 {
            return None;
        }
        // SAFETY: `h` is valid with `PROCESS_SYNCHRONIZE`; timeout 0 polls without blocking.
        let alive = unsafe { WaitForSingleObject(h, 0) } == WAIT_TIMEOUT;
        Some((Self(owned), alive))
    }

    pub(crate) fn open_alive(pid: u32, started_at: i64) -> Option<Self> {
        Self::open_matching(pid, started_at).and_then(|(p, alive)| alive.then_some(p))
    }

    /// Blocks until the process terminates and returns its raw OS exit code.
    pub(crate) fn wait_blocking(self) -> Option<u32> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, WaitForSingleObject, INFINITE,
        };
        let h = self.0.as_raw_handle().cast();
        // SAFETY: `self.0` owns a valid process handle with `PROCESS_SYNCHRONIZE` and
        // `PROCESS_QUERY_LIMITED_INFORMATION` for the duration of both calls.
        unsafe {
            WaitForSingleObject(h, INFINITE);
            let mut code: u32 = 0;
            (GetExitCodeProcess(h, &mut code) != 0).then_some(code)
        }
    }
}

/// True if the game process recorded in `s` is still running on this machine (matching both PID
/// and creation time to guard against Windows PID reuse).
pub fn is_session_process_alive(s: &ActiveSession) -> bool {
    LiveProcess::open_alive(s.pid, s.started_at).is_some()
}

const SCHEMA: u32 = 1;
/// How often the running session is persisted. Frequent enough to lose little on a crash,
/// rare enough not to wear a USB stick.
pub const HEARTBEAT: Duration = Duration::from_secs(60);
/// A tick later than this since the previous one is treated as a suspend/freeze gap.
pub const MAX_TICK_GAP: Duration = Duration::from_secs(180);
const APPLIED_MEMORY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstancePlaytime {
    pub schema: u32,
    pub total_seconds: u64,
    pub sessions: u32,
    /// Unix seconds of the end of the last session (display only).
    pub last_played_at: Option<i64>,
    applied: Vec<String>,
}

impl Default for InstancePlaytime {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            total_seconds: 0,
            sessions: 0,
            last_played_at: None,
            applied: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifetimePlaytime {
    pub schema: u32,
    pub total_seconds: u64,
    pub sessions: u64,
    applied: Vec<String>,
}

impl Default for LifetimePlaytime {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            total_seconds: 0,
            sessions: 0,
            applied: Vec::new(),
        }
    }
}

/// The on-disk journal entry for a session that has started and not yet been banked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveSession {
    pub schema: u32,
    pub session_id: String,
    pub instance_id: InstanceId,
    pub pid: u32,
    /// Unix seconds when the game started (display/diagnostics only).
    pub started_at: i64,
    /// Unix seconds of the last heartbeat (display/diagnostics only).
    pub last_heartbeat_at: i64,
    /// Milliseconds of confirmed play so far. This is the number that gets banked.
    pub accumulated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankedSession {
    pub session_id: String,
    pub instance_id: InstanceId,
    pub seconds: u64,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> io::Result<T> {
    match fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e),
    }
}

/// Like [`read_json`], but a total file whose *contents* cannot be read (empty, truncated, garbage)
/// does not stop banking: the damaged file is moved aside (`<name>.corrupt`, never overwriting an
/// earlier backup, never deleted), a fresh default total starts, and the recovery is logged. A real
/// I/O failure is still an error.
fn read_total_or_quarantine<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> io::Result<T> {
    match fs::read(path) {
        Ok(b) => match serde_json::from_slice(&b) {
            Ok(v) => Ok(v),
            Err(e) => {
                let moved = quarantine_corrupt(path)?;
                tracing::warn!(
                    line = e.line(),
                    column = e.column(),
                    backup = %moved.file_name().unwrap_or_default().to_string_lossy(),
                    "a playtime file was damaged; it was set aside and a fresh total started"
                );
                Ok(T::default())
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    atomic_write(
        path,
        &serde_json::to_vec_pretty(value).map_err(io::Error::other)?,
    )
}

fn remember(applied: &mut Vec<String>, id: &str) {
    applied.push(id.to_owned());
    if applied.len() > APPLIED_MEMORY {
        applied.remove(0);
    }
}

pub struct PlaytimeStore<'a> {
    root: &'a PortableRoot,
}

impl<'a> PlaytimeStore<'a> {
    pub fn new(root: &'a PortableRoot) -> Self {
        Self { root }
    }

    fn active_dir(&self) -> PathBuf {
        self.root.data_dir().join("playtime-active")
    }
    fn lifetime_file(&self) -> PathBuf {
        self.root.data_dir().join("playtime.json")
    }
    fn instance_dir(&self, id: &InstanceId) -> PathBuf {
        self.root.instances_dir().join(id.as_str())
    }
    fn instance_file(&self, id: &InstanceId) -> PathBuf {
        self.instance_dir(id).join("playtime.json")
    }

    pub fn instance_total(&self, id: &InstanceId) -> io::Result<InstancePlaytime> {
        read_json(&self.instance_file(id))
    }

    /// Reads `playtime.json` from an arbitrary instance folder (such as a retained `.removed/` folder).
    pub fn dir_total(dir: &Path) -> io::Result<InstancePlaytime> {
        read_json(&dir.join("playtime.json"))
    }

    pub fn lifetime_total(&self) -> io::Result<LifetimePlaytime> {
        read_json(&self.lifetime_file())
    }

    /// Call right after the game process was spawned. `now` is the monotonic start time.
    pub fn begin_session(
        &self,
        instance: &InstanceId,
        pid: u32,
        now_unix: i64,
        now: Instant,
    ) -> io::Result<SessionTracker> {
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).map_err(|e| io::Error::other(e.to_string()))?;
        let session = ActiveSession {
            schema: SCHEMA,
            session_id: format!("{now_unix}-{}", hex::encode(id)),
            instance_id: instance.clone(),
            pid,
            started_at: now_unix,
            last_heartbeat_at: now_unix,
            accumulated_ms: 0,
        };
        let path = self
            .active_dir()
            .join(format!("{}.json", session.session_id));
        write_json(&path, &session)?;
        Ok(SessionTracker {
            path,
            session,
            last_tick: now,
            last_unbiased_ms: None,
        })
    }

    /// Adds a finished session to the per-instance and lifetime totals. Idempotent.
    fn bank(&self, s: &ActiveSession) -> io::Result<BankedSession> {
        let _guard = BANK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let seconds = (s.accumulated_ms + 500) / 1000;

        // Per instance (skipped if the instance was deleted meanwhile; lifetime still counts it).
        if self.instance_dir(&s.instance_id).is_dir() {
            let path = self.instance_file(&s.instance_id);
            let mut t: InstancePlaytime = read_total_or_quarantine(&path)?;
            if !t.applied.contains(&s.session_id) {
                t.total_seconds += seconds;
                t.sessions += 1;
                t.last_played_at = Some(s.last_heartbeat_at);
                remember(&mut t.applied, &s.session_id);
                write_json(&path, &t)?;
            }
        }

        let path = self.lifetime_file();
        let mut l: LifetimePlaytime = read_total_or_quarantine(&path)?;
        if !l.applied.contains(&s.session_id) {
            l.total_seconds += seconds;
            l.sessions += 1;
            remember(&mut l.applied, &s.session_id);
            write_json(&path, &l)?;
        }
        Ok(BankedSession {
            session_id: s.session_id.clone(),
            instance_id: s.instance_id.clone(),
            seconds,
        })
    }

    /// Returns valid open journal entries whose game process is still running on this machine, so
    /// startup can reattach to them rather than banking them early or truncating their output.
    pub fn live_sessions(&self) -> io::Result<Vec<(PathBuf, ActiveSession)>> {
        let dir = self.active_dir();
        let mut live = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(live),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(s) = fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<ActiveSession>(&b).ok())
                .filter(is_session_process_alive)
            {
                live.push((path, s));
            }
        }
        Ok(live)
    }

    /// Startup recovery: bank every session left open by a crash, power loss or forced kill,
    /// skipping any session whose game process is still running. Unreadable journal entries are
    /// set aside (`.corrupt`) rather than lost or retried forever.
    pub fn recover(&self) -> io::Result<Vec<BankedSession>> {
        let dir = self.active_dir();
        let mut banked = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(banked),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<ActiveSession>(&b).ok())
            {
                Some(s) if is_session_process_alive(&s) => {
                    // The game is still running: leave the journal entry for `reattach_live_sessions`.
                }
                Some(s) => match self.bank(&s) {
                    Ok(b) => {
                        banked.push(b);
                        if let Err(e) = remove_file_with_retry(&path) {
                            tracing::warn!(error = %e, "could not remove a banked playtime journal entry");
                        }
                    }
                    // One session that cannot be banked now (a real I/O problem) must not stop the
                    // others; its journal entry stays and is tried again at the next start.
                    Err(e) => {
                        tracing::warn!(error = %e, "could not bank a playtime session; it stays in the journal")
                    }
                },
                None => match quarantine_corrupt(&path) {
                    Ok(_) => {
                        tracing::warn!("a playtime journal entry was damaged and was set aside")
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "could not set aside a damaged journal entry")
                    }
                },
            }
        }
        Ok(banked)
    }
}

/// A running session. Tick it periodically while the process is alive and call `finish` when it exits.
pub struct SessionTracker {
    path: PathBuf,
    session: ActiveSession,
    last_tick: Instant,
    last_unbiased_ms: Option<u64>,
}

impl SessionTracker {
    /// Reconstructs a tracker for an already-open journal entry whose game process is still alive.
    pub fn resume(path: PathBuf, session: ActiveSession, now: Instant) -> Self {
        Self {
            path,
            session,
            last_tick: now,
            last_unbiased_ms: None,
        }
    }

    /// Switches elapsed-time measurement to [`unbiased_ms`] (`QueryUnbiasedInterruptTime`) so
    /// Windows sleep/hibernation intervals never inflate playtime even when shorter than [`MAX_TICK_GAP`].
    pub fn with_unbiased_clock(mut self) -> Self {
        self.last_unbiased_ms = Some(unbiased_ms());
        self
    }

    /// Records elapsed play time and refreshes the journal. `now_unix` is only for display.
    pub fn tick(&mut self, now_unix: i64, now: Instant) -> io::Result<()> {
        let elapsed = if let Some(prev) = self.last_unbiased_ms {
            let cur = unbiased_ms();
            self.last_unbiased_ms = Some(cur);
            Duration::from_millis(cur.saturating_sub(prev))
        } else {
            now.saturating_duration_since(self.last_tick)
        };
        let counted = if elapsed <= MAX_TICK_GAP {
            elapsed
        } else {
            HEARTBEAT
        };
        self.session.accumulated_ms += counted.as_millis() as u64;
        self.session.last_heartbeat_at = now_unix;
        self.last_tick = now;
        write_json(&self.path, &self.session)
    }

    /// Game process exited: take a last measurement, bank it, and close the journal entry.
    pub fn finish(
        mut self,
        store: &PlaytimeStore<'_>,
        now_unix: i64,
        now: Instant,
    ) -> io::Result<BankedSession> {
        self.tick(now_unix, now)?;
        let banked = store.bank(&self.session)?;
        remove_file_with_retry(&self.path)?;
        Ok(banked)
    }

    pub fn accumulated(&self) -> Duration {
        Duration::from_millis(self.session.accumulated_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::{InstanceStore, NewInstance};

    fn setup() -> (tempfile::TempDir, PortableRoot, InstanceId) {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        fs::create_dir_all(root.data_dir().join("playtime-active")).unwrap();
        let id = InstanceStore::new(&root)
            .create(NewInstance::vanilla("Vanilla", "1.21.4").unwrap(), 1)
            .unwrap()
            .id;
        (d, root, id)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn counts_only_the_running_game_and_banks_per_instance_and_lifetime() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        // Launcher idle time before and after is irrelevant: nothing is recorded without a session.
        assert_eq!(store.instance_total(&id).unwrap().total_seconds, 0);

        let t0 = Instant::now();
        let mut s = store.begin_session(&id, 4242, 1000, t0).unwrap();
        s.tick(1060, t0 + secs(60)).unwrap();
        s.tick(1120, t0 + secs(120)).unwrap();
        let banked = s.finish(&store, 1150, t0 + secs(150)).unwrap();
        assert_eq!(banked.seconds, 150);

        let inst = store.instance_total(&id).unwrap();
        assert_eq!(
            (inst.total_seconds, inst.sessions, inst.last_played_at),
            (150, 1, Some(1150))
        );
        let life = store.lifetime_total().unwrap();
        assert_eq!((life.total_seconds, life.sessions), (150, 1));
        assert_eq!(
            fs::read_dir(root.data_dir().join("playtime-active"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn instances_keep_separate_totals_and_lifetime_is_the_sum() {
        let (_d, root, a) = setup();
        let b = InstanceStore::new(&root)
            .create(NewInstance::vanilla("Other", "1.20.1").unwrap(), 2)
            .unwrap()
            .id;
        let store = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        store
            .begin_session(&a, 1, 0, t0)
            .unwrap()
            .finish(&store, 0, t0 + secs(100))
            .unwrap();
        store
            .begin_session(&b, 2, 0, t0)
            .unwrap()
            .finish(&store, 0, t0 + secs(40))
            .unwrap();
        store
            .begin_session(&a, 3, 0, t0)
            .unwrap()
            .finish(&store, 0, t0 + secs(10))
            .unwrap();
        assert_eq!(store.instance_total(&a).unwrap().total_seconds, 110);
        assert_eq!(store.instance_total(&b).unwrap().total_seconds, 40);
        assert_eq!(store.lifetime_total().unwrap().total_seconds, 150);
    }

    #[test]
    fn a_suspend_gap_is_not_counted_as_play_time() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        let mut s = store.begin_session(&id, 1, 0, t0).unwrap();
        s.tick(60, t0 + secs(60)).unwrap();
        // The PC sleeps for 8 hours; the next tick arrives far later than MAX_TICK_GAP.
        s.tick(0, t0 + secs(60 + 8 * 3600)).unwrap();
        assert_eq!(s.accumulated(), secs(60) + HEARTBEAT);
    }

    #[test]
    fn wall_clock_changes_cannot_change_the_result() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        let mut s = store.begin_session(&id, 1, 1_000_000, t0).unwrap();
        // The user sets the system clock back a year mid-session.
        s.tick(1_000_000 - 31_536_000, t0 + secs(60)).unwrap();
        let banked = s.finish(&store, 5, t0 + secs(90)).unwrap();
        assert_eq!(banked.seconds, 90);
    }

    #[test]
    fn a_crashed_session_is_recovered_at_next_start_up_to_the_last_heartbeat() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        let mut s = store.begin_session(&id, 1, 1000, t0).unwrap();
        s.tick(1060, t0 + secs(60)).unwrap();
        s.tick(1120, t0 + secs(120)).unwrap();
        drop(s); // launcher killed / power cut: no finish(), journal entry stays on disk
        assert_eq!(store.instance_total(&id).unwrap().total_seconds, 0);

        let recovered = store.recover().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].seconds, 120);
        assert_eq!(store.instance_total(&id).unwrap().total_seconds, 120);
        assert_eq!(store.lifetime_total().unwrap().total_seconds, 120);
        assert!(
            store.recover().unwrap().is_empty(),
            "recovery must not bank twice"
        );
    }

    #[test]
    fn crash_between_the_two_file_updates_does_not_double_count() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        let mut s = store.begin_session(&id, 1, 1000, t0).unwrap();
        s.tick(1060, t0 + secs(60)).unwrap();
        let session = s.session.clone();
        drop(s);
        // Simulate dying after the per-instance file was written but before lifetime/cleanup.
        let inst = InstancePlaytime {
            total_seconds: 60,
            sessions: 1,
            applied: vec![session.session_id.clone()],
            ..Default::default()
        };
        write_json(&store.instance_file(&id), &inst).unwrap();

        store.recover().unwrap();
        assert_eq!(
            store.instance_total(&id).unwrap().total_seconds,
            60,
            "instance total must not double"
        );
        assert_eq!(
            store.lifetime_total().unwrap().total_seconds,
            60,
            "lifetime must still be completed"
        );
    }

    #[test]
    fn deleting_an_instance_keeps_lifetime_but_removes_its_own_stats() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        // Tick once a minute, like the real heartbeat; one 300 s jump would count as a suspend gap.
        let mut s = store.begin_session(&id, 1, 0, t0).unwrap();
        for minute in 1..=4 {
            s.tick(0, t0 + secs(60 * minute)).unwrap();
        }
        s.finish(&store, 0, t0 + secs(300)).unwrap();
        InstanceStore::new(&root).delete(&id).unwrap();
        assert_eq!(store.instance_total(&id).unwrap().total_seconds, 0);
        assert_eq!(store.lifetime_total().unwrap().total_seconds, 300);

        // A session that was still open when its instance vanished is still counted in lifetime
        // and must not recreate the instance folder.
        let id2 = InstanceStore::new(&root)
            .create(NewInstance::vanilla("Temp", "1.21.4").unwrap(), 5)
            .unwrap()
            .id;
        let mut s = store.begin_session(&id2, 9, 0, t0).unwrap();
        s.tick(0, t0 + secs(30)).unwrap();
        drop(s);
        InstanceStore::new(&root).delete(&id2).unwrap();
        store.recover().unwrap();
        assert_eq!(store.lifetime_total().unwrap().total_seconds, 330);
        assert!(!root.instances_dir().join(id2.as_str()).exists());
    }

    #[test]
    fn corrupt_journal_entries_are_set_aside_not_fatal() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        fs::write(
            root.data_dir().join("playtime-active").join("junk.json"),
            b"{ not json",
        )
        .unwrap();
        let t0 = Instant::now();
        let mut s = store.begin_session(&id, 1, 0, t0).unwrap();
        s.tick(0, t0 + secs(60)).unwrap();
        drop(s);
        let recovered = store.recover().unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(root
            .data_dir()
            .join("playtime-active")
            .join("junk.json.corrupt")
            .exists());
    }

    #[test]
    fn playtime_files_contain_no_paths_or_secrets() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        store
            .begin_session(&id, 1, 0, t0)
            .unwrap()
            .finish(&store, 0, t0 + secs(5))
            .unwrap();
        for f in [store.instance_file(&id), store.lifetime_file()] {
            let text = fs::read_to_string(f).unwrap();
            assert!(!text.contains(":\\") && !text.contains(":/") && !text.contains("token"));
        }
    }

    #[test]
    fn one_damaged_total_neither_blocks_banking_nor_recovery_of_the_others() {
        let (_d, root, a) = setup();
        let b = InstanceStore::new(&root)
            .create(NewInstance::vanilla("Other", "1.21.4").unwrap(), 2)
            .unwrap()
            .id;
        let store = PlaytimeStore::new(&root);
        // instance A's total is damaged; instance B and the lifetime total are fine
        let a_file = root.instances_dir().join(a.as_str()).join("playtime.json");
        fs::write(&a_file, b"{ truncated").unwrap();
        let t0 = Instant::now();
        for (inst, pid) in [(&a, 1u32), (&b, 2u32)] {
            let mut s = store.begin_session(inst, pid, 0, t0).unwrap();
            s.tick(0, t0 + secs(60)).unwrap();
            drop(s); // crashed: only the journal remains
        }
        let recovered = store.recover().unwrap();
        assert_eq!(recovered.len(), 2, "both sessions were banked");
        // A: the damaged file was set aside untouched and a fresh total started
        assert_eq!(
            fs::read(a_file.with_file_name("playtime.json.corrupt")).unwrap(),
            b"{ truncated"
        );
        assert_eq!(store.instance_total(&a).unwrap().total_seconds, 60);
        // B and the lifetime total are right
        assert_eq!(store.instance_total(&b).unwrap().total_seconds, 60);
        assert_eq!(store.lifetime_total().unwrap().total_seconds, 120);
        // the journal is empty: nothing will be banked twice
        assert_eq!(
            fs::read_dir(root.data_dir().join("playtime-active"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn a_damaged_lifetime_total_is_set_aside_too_and_a_second_damage_keeps_both() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);
        let lifetime = root.data_dir().join("playtime.json");
        let t0 = Instant::now();
        for (n, junk) in [&b"first junk"[..], b"second junk"].into_iter().enumerate() {
            fs::write(&lifetime, junk).unwrap();
            let mut s = store.begin_session(&id, 1, 0, t0).unwrap();
            s.tick(0, t0 + secs(30)).unwrap();
            s.finish(&store, 0, t0 + secs(30)).unwrap();
            assert!(lifetime.is_file(), "round {n}: a fresh total was written");
        }
        let kept: Vec<_> = fs::read_dir(root.data_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("playtime.json.corrupt"))
            .collect();
        assert_eq!(kept.len(), 2, "both damaged copies were kept: {kept:?}");
    }

    #[test]
    fn unbiased_clock_advances_monotonically_and_ignores_synthetic_instant_jumps() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);

        let m1 = unbiased_ms();
        std::thread::sleep(Duration::from_millis(35));
        let m2 = unbiased_ms();
        assert!(m2 >= m1, "{m2} >= {m1}");
        assert!(m2 - m1 >= 15, "unbiased clock advanced: {} ms", m2 - m1);

        let t0 = Instant::now();
        let mut s = store
            .begin_session(&id, 0, 1_000, t0)
            .unwrap()
            .with_unbiased_clock();
        std::thread::sleep(Duration::from_millis(30));
        // Even if `now` jumps by 120 seconds (below MAX_TICK_GAP), `with_unbiased_clock` reads
        // QueryUnbiasedInterruptTime instead, so a short Windows sleep cannot inflate playtime.
        s.tick(1_120, t0 + secs(120)).unwrap();
        assert!(
            s.accumulated() < secs(5),
            "unbiased clock measured real wake time, got {:?}",
            s.accumulated()
        );
    }

    #[test]
    fn concurrent_banking_serialises_lifetime_updates_without_losing_sessions() {
        let (_d, root, _) = setup();
        let mut ids = Vec::new();
        for n in 0..8 {
            ids.push(
                InstanceStore::new(&root)
                    .create(
                        NewInstance::vanilla(&format!("Inst {n}"), "1.21.4").unwrap(),
                        10 + n,
                    )
                    .unwrap()
                    .id,
            );
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(ids.len()));
        std::thread::scope(|scope| {
            for id in &ids {
                let root = &root;
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let store = PlaytimeStore::new(root);
                    let t0 = Instant::now();
                    let mut s = store.begin_session(id, 0, 1_000, t0).unwrap();
                    s.tick(1_010, t0 + secs(10)).unwrap();
                    barrier.wait();
                    s.finish(&store, 1_025, t0 + secs(25)).unwrap();
                });
            }
        });
        let store = PlaytimeStore::new(&root);
        let life = store.lifetime_total().unwrap();
        assert_eq!(life.sessions, 8);
        assert_eq!(life.total_seconds, 8 * 25);
    }

    #[test]
    fn live_process_sessions_are_detected_skipped_by_recover_and_resumable_until_exit() {
        let (_d, root, id) = setup();
        let store = PlaytimeStore::new(&root);

        let mut child = std::process::Command::new("cmd")
            .args(["/D", "/C", "ping -n 4 127.0.0.1 >nul"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let started = crate::profiles::now_unix();
        let t0 = Instant::now();
        let mut s = store.begin_session(&id, pid, started, t0).unwrap();
        s.tick(started + 60, t0 + secs(60)).unwrap();
        // Simulate the launcher closing while the game process stays alive.
        drop(s);

        let live = store.live_sessions().unwrap();
        assert_eq!(live.len(), 1, "running child must be detected as live");
        assert_eq!(live[0].1.pid, pid);

        // PID-reuse guard: the same live PID with a stale `started_at` (> 120s away) is rejected.
        let mut stale = live[0].1.clone();
        stale.started_at = started - 3_600;
        assert!(!is_session_process_alive(&stale));
        assert!(!is_session_process_alive(&ActiveSession {
            pid: 0,
            ..stale
        }));

        // `recover` must leave the live session untouched rather than banking or deleting it early.
        assert!(store.recover().unwrap().is_empty());
        assert_eq!(store.instance_total(&id).unwrap().total_seconds, 0);
        assert!(live[0].0.is_file());

        // Once the process actually exits, `live_sessions` is empty and `recover` banks it.
        let _ = child.wait().unwrap();
        assert!(store.live_sessions().unwrap().is_empty());
        let banked = store.recover().unwrap();
        assert_eq!(banked.len(), 1);
        assert_eq!(banked[0].seconds, 60);
        assert_eq!(store.instance_total(&id).unwrap().total_seconds, 60);
        assert!(!live[0].0.exists());

        // Also verify `SessionTracker::resume` banks accumulated + post-resume time cleanly.
        let mut s2 = store.begin_session(&id, 0, started, t0).unwrap();
        s2.tick(started + 30, t0 + secs(30)).unwrap();
        let (jpath, active) = (s2.path.clone(), s2.session.clone());
        drop(s2);
        let resumed = SessionTracker::resume(jpath, active, t0 + secs(30));
        let b2 = resumed.finish(&store, started + 50, t0 + secs(50)).unwrap();
        assert_eq!(b2.seconds, 50);
        assert_eq!(store.instance_total(&id).unwrap().total_seconds, 110);
    }
}
