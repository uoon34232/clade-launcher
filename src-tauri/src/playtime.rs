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
//!   heartbeat. [`PlaytimeStore::recover`] runs at startup and banks it, so at most about one
//!   heartbeat interval is lost.
//! - Banking a finished session is idempotent: each total remembers recently applied session
//!   ids, so a crash between the two file updates cannot double count on recovery.
//!
//! Known limit: if the launcher is closed while the game keeps running, time after that point
//! is not counted (the launcher is what observes the process).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::instances::InstanceId;
use crate::portable::fsutil::{atomic_write, quarantine_corrupt};
use crate::portable::PortableRoot;

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
        })
    }

    /// Adds a finished session to the per-instance and lifetime totals. Idempotent.
    fn bank(&self, s: &ActiveSession) -> io::Result<BankedSession> {
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

    /// Startup recovery: bank every session left open by a crash, power loss or forced kill.
    /// Unreadable journal entries are set aside (`.corrupt`) rather than lost or retried forever.
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
                Some(s) => match self.bank(&s) {
                    Ok(b) => {
                        banked.push(b);
                        if let Err(e) = fs::remove_file(&path) {
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
}

impl SessionTracker {
    /// Records elapsed play time and refreshes the journal. `now_unix` is only for display.
    pub fn tick(&mut self, now_unix: i64, now: Instant) -> io::Result<()> {
        let elapsed = now.saturating_duration_since(self.last_tick);
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
        fs::remove_file(&self.path)?;
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
}
