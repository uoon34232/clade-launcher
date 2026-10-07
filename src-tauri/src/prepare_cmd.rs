//! The Tauri side of preparing a profile: one preparation at a time, progress events, cancel,
//! and the internal Idle / Preparing / Failed state.
//!
//! Lifecycle of a profile as the UI sees it: Pending -> Preparing -> Ready. `Preparing` and
//! `Failed` are runtime states only (the manifest keeps saying Pending until the final check has
//! passed), so a crash can never leave a half-prepared profile marked Ready. A failure keeps its
//! structured error until the next attempt; everything verified so far stays on disk for the retry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::accounts::AccountsFile;
use crate::accounts_api::AccountsState;
use crate::download::{Cancel, Progress};
use crate::instances::InstanceId;
use crate::loader_install::SystemEnv;
use crate::loader_prepare::ProcessRunner;
use crate::net::HttpFetcher;
use crate::portable::PortableRoot;
use crate::prepare::{prepare_instance, Ctx, PrepareError, Tracker};
use crate::profile_lock::{Activity, ProfileLocks};
use crate::profiles::{now_unix, snapshot, ProfilesDto};
use crate::ui_settings::SettingsState;

pub const PROGRESS_EVENT: &str = "prepare-progress";
const TICK: Duration = Duration::from_millis(150);

/// Everything the prepare commands need. Four clients: documents and large files, each for the
/// global allow-list and for the loader hosts only.
pub struct PrepareState {
    pub root: PortableRoot,
    pub meta: Result<HttpFetcher, String>,
    pub loader_meta: Result<HttpFetcher, String>,
    pub downloads: Result<HttpFetcher, String>,
    pub loader_downloads: Result<HttpFetcher, String>,
    running: Mutex<Option<(InstanceId, Arc<Cancel>)>>,
    failed: Mutex<HashMap<String, PrepareFailure>>,
    locks: Arc<ProfileLocks>,
}

impl PrepareState {
    pub fn new(
        root: PortableRoot,
        meta: Result<HttpFetcher, String>,
        loader_meta: Result<HttpFetcher, String>,
        downloads: Result<HttpFetcher, String>,
        loader_downloads: Result<HttpFetcher, String>,
    ) -> Self {
        Self {
            root,
            meta,
            loader_meta,
            downloads,
            loader_downloads,
            running: Mutex::new(None),
            failed: Mutex::new(HashMap::new()),
            locks: Arc::new(ProfileLocks::default()),
        }
    }

    /// Shares the per-profile lock registry with the launcher, so a profile is never prepared while
    /// its game runs, nor launched while it is prepared.
    pub fn with_locks(mut self, locks: Arc<ProfileLocks>) -> Self {
        self.locks = locks;
        self
    }

    /// Starts a preparation of `id`, or says why not. One preparation at a time overall, and never
    /// for a profile whose game is starting or running. The checks and the claims happen under one
    /// lock, so two commands cannot both get through.
    pub(crate) fn begin(&self, id: &InstanceId, cancel: &Arc<Cancel>) -> Result<(), PrepareError> {
        let mut running = lock(&self.running);
        if running.is_some() {
            return Err(PrepareError::Busy);
        }
        match self.locks.claim(id.as_str(), Activity::Preparing) {
            Ok(()) => {}
            Err(Activity::Launching) => return Err(PrepareError::GameRunning),
            Err(Activity::Preparing) => return Err(PrepareError::Busy),
        }
        *running = Some((id.clone(), cancel.clone()));
        Ok(())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A structured failure for the UI: a stable code and a plain message.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PrepareFailure {
    pub code: &'static str,
    /// Facts the frontend needs to word the message (byte counts); never secrets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// English detail for logs; the UI words the failure from `code` and `params`.
    pub message: String,
}

impl From<&PrepareError> for PrepareFailure {
    fn from(e: &PrepareError) -> Self {
        let params = match e {
            PrepareError::DiskSpace { needed, available } => {
                Some(serde_json::json!({ "requiredBytes": needed, "availableBytes": available }))
            }
            _ => None,
        };
        Self {
            code: e.code(),
            params,
            message: e.to_string(),
        }
    }
}

/// Clears scratch space a crashed run may have left (`cache/tmp`). Verified files and `.part`
/// files are kept on purpose: they are what makes the next attempt resume.
pub fn sweep_scratch(root: &PortableRoot) -> std::io::Result<usize> {
    let mut removed = 0;
    let Ok(rd) = std::fs::read_dir(root.tmp_dir()) else {
        return Ok(0);
    };
    for e in rd.flatten() {
        let p = e.path();
        let ok = if e.file_type().is_ok_and(|t| t.is_dir()) {
            std::fs::remove_dir_all(&p).is_ok()
        } else {
            std::fs::remove_file(&p).is_ok()
        };
        removed += usize::from(ok);
    }
    Ok(removed)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareStatusDto {
    /// "idle", "preparing" or "failed".
    pub state: &'static str,
    pub error: Option<PrepareFailure>,
}

impl PrepareState {
    pub fn status(&self, id: &str) -> PrepareStatusDto {
        if lock(&self.running)
            .as_ref()
            .is_some_and(|(r, _)| r.as_str() == id)
        {
            return PrepareStatusDto {
                state: "preparing",
                error: None,
            };
        }
        match lock(&self.failed).get(id) {
            Some(f) => PrepareStatusDto {
                state: "failed",
                error: Some(f.clone()),
            },
            None => PrepareStatusDto {
                state: "idle",
                error: None,
            },
        }
    }
}

struct Running<'a>(&'a PrepareState, String);
impl Drop for Running<'_> {
    fn drop(&mut self) {
        *lock(&self.0.running) = None;
        self.0.locks.release(&self.1);
    }
}

fn client(r: &Result<HttpFetcher, String>) -> Result<&HttpFetcher, PrepareError> {
    r.as_ref()
        .map_err(|_| PrepareError::Unsupported("networking is unavailable".into()))
}

/// Preparing, repairing and verifying a profile download or re-check game files, so they need an
/// account that can play: the active one, `Ready`. Checked here, in the backend, before anything
/// is claimed or fetched; the dock button is only a convenience on top of this.
pub(crate) fn require_ready_account(accounts: &AccountsState) -> Result<(), PrepareError> {
    let file = AccountsFile::load(accounts.path())
        .map_err(|e| PrepareError::Storage(format!("the account list could not be read: {e}")))?;
    file.ready_active()
        .map(|_| ())
        .ok_or(PrepareError::AccountRequired)
}

#[tauri::command]
pub async fn prepare_profile(
    app: AppHandle,
    state: State<'_, PrepareState>,
    settings: State<'_, SettingsState>,
    accounts: State<'_, AccountsState>,
    id: String,
    verify: Option<bool>,
) -> Result<ProfilesDto, PrepareFailure> {
    let instance =
        InstanceId::new(&id).map_err(|_| PrepareFailure::from(&PrepareError::NotFound))?;
    // Not a failed preparation: nothing is claimed, nothing is fetched, no failure is remembered.
    require_ready_account(&accounts).map_err(|e| PrepareFailure::from(&e))?;
    let cancel = Cancel::new();
    if let Err(refused) = state.begin(&instance, &cancel) {
        return Err((&refused).into());
    }
    let _guard = Running(&state, id.clone());
    lock(&state.failed).remove(&id);

    let outcome = run(
        &app,
        &state,
        &instance,
        cancel,
        &settings,
        verify.unwrap_or(false),
    )
    .await;
    match outcome {
        Ok(()) => snapshot(&state.root, settings.last_instance())
            .map_err(|e| PrepareFailure::from(&PrepareError::Storage(e.to_string()))),
        Err(e) => {
            let failure = PrepareFailure::from(&e);
            tracing::info!(code = e.code(), "profile preparation did not complete");
            if !matches!(e, PrepareError::Cancelled) {
                lock(&state.failed).insert(id, failure.clone());
            }
            Err(failure)
        }
    }
}

async fn run(
    app: &AppHandle,
    state: &PrepareState,
    id: &InstanceId,
    cancel: Arc<Cancel>,
    settings: &SettingsState,
    verify: bool,
) -> Result<(), PrepareError> {
    let sink_app = app.clone();
    let tracker = Arc::new(Tracker::new(
        id,
        Progress::new(),
        Box::new(move |p| {
            let _ = sink_app.emit(PROGRESS_EVENT, p);
        }),
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let (t, stop) = (tracker.clone(), stop.clone());
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(TICK).await;
                t.emit();
            }
        })
    };

    let result = async {
        let ctx = Ctx {
            root: &state.root,
            meta: client(&state.meta)?,
            loader_meta: client(&state.loader_meta)?,
            streamer: client(&state.downloads)?,
            loader_streamer: client(&state.loader_downloads)?,
            runner: &ProcessRunner,
            sys: SystemEnv::from_process(),
            concurrency: settings.max_concurrent_downloads(),
            now: now_unix(),
            backoff: Duration::from_millis(400),
            cancel,
            tracker: &tracker,
            verify_all: verify,
            free_space: Arc::new(crate::system::free_disk_bytes),
        };
        prepare_instance(&ctx, id).await.map(|_| ())
    }
    .await;

    stop.store(true, Ordering::Relaxed);
    ticker.abort();
    result
}

#[tauri::command]
pub fn cancel_prepare(state: State<'_, PrepareState>) {
    if let Some((_, cancel)) = lock(&state.running).as_ref() {
        cancel.cancel();
    }
}

#[tauri::command]
pub fn prepare_status(state: State<'_, PrepareState>, id: String) -> PrepareStatusDto {
    state.status(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(root: PortableRoot) -> PrepareState {
        let no = || Err("tests do not use the network".to_owned());
        PrepareState::new(root, no(), no(), no(), no())
    }

    #[test]
    fn preparing_or_verifying_needs_a_ready_active_account_in_the_backend() {
        use crate::accounts::{AccountMeta, AccountStatus, ReauthReason};
        let d = tempfile::tempdir().unwrap();
        let accounts = AccountsState::new(d.path().join("accounts.json"));
        let meta = |id: &str| AccountMeta {
            id: id.into(),
            username: format!("Fixture{id}"),
            minecraft_uuid: "00000000-0000-4000-8000-00000000a001".into(),
            added_at: 1,
            last_used_at: None,
            status: AccountStatus::Ready,
        };
        let code = |a: &AccountsState| {
            require_ready_account(a)
                .err()
                .map(|e| PrepareFailure::from(&e).code)
        };

        // No account at all (and no accounts.json): rejected with the stable code.
        assert_eq!(code(&accounts), Some("account_required"));
        accounts
            .update(|f| {
                f.accounts.push(meta("a"));
                f.accounts.push(meta("b"));
                f.active = Some("a".into());
            })
            .unwrap();
        assert_eq!(code(&accounts), None, "a ready active account may prepare");
        // The active account needs to sign in again; the other one being ready changes nothing.
        accounts
            .update(|f| f.mark_needs_reauth("a", ReauthReason::Revoked, 5))
            .unwrap();
        assert_eq!(code(&accounts), Some("account_required"));
        // The failure is a structured code with no parameters, and the accounts stay untouched.
        let f = PrepareFailure::from(&PrepareError::AccountRequired);
        assert_eq!((f.code, f.params.is_none()), ("account_required", true));
        assert_eq!(accounts.update(|f| f.accounts.len()).unwrap(), 2);
        // An unreadable list is a storage problem, not "sign in".
        std::fs::write(accounts.path(), b"{ not json").unwrap();
        assert_eq!(code(&accounts), Some("storage"));
    }

    #[test]
    fn the_command_checks_the_account_before_it_claims_or_fetches_anything() {
        // `prepare_profile` must refuse before `begin` (which claims the profile) and before any
        // download client is touched; keep the order from regressing.
        let src = include_str!("prepare_cmd.rs");
        let body = &src[src.find("pub async fn prepare_profile").unwrap()..];
        let gate = body.find("require_ready_account(&accounts)").unwrap();
        assert!(gate < body.find("state.begin(").unwrap());
        assert!(gate < body.find("run(").unwrap());
    }

    #[test]
    fn a_full_disk_is_reported_as_a_code_with_byte_counts_not_prose() {
        let e = PrepareError::DiskSpace {
            needed: 3 * 1024 * 1024 * 1024,
            available: 200 * 1024 * 1024,
        };
        let f = PrepareFailure::from(&e);
        assert_eq!(f.code, "disk_space");
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["params"]["requiredBytes"], 3u64 * 1024 * 1024 * 1024);
        assert_eq!(json["params"]["availableBytes"], 200u64 * 1024 * 1024);
        // Other failures carry no parameters.
        let plain = serde_json::to_value(PrepareFailure::from(&PrepareError::Busy)).unwrap();
        assert!(plain.get("params").is_none());
    }

    #[test]
    fn launching_and_preparing_the_same_profile_exclude_each_other_but_other_profiles_do_not() {
        use crate::launch_cmd::LaunchState;
        use crate::profile_lock::Activity;
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        let locks = Arc::new(ProfileLocks::default());
        let prep = state(root.clone()).with_locks(locks.clone());
        let launch = LaunchState::new(root).with_locks(locks);
        let (a, b) = (InstanceId::new("a").unwrap(), InstanceId::new("b").unwrap());
        let cancel = Cancel::new();

        // a running game: its profile cannot be prepared or verified, another one can
        assert!(launch.claim(&a));
        assert!(matches!(
            prep.begin(&a, &cancel),
            Err(PrepareError::GameRunning)
        ));
        assert_eq!(
            PrepareFailure::from(&PrepareError::GameRunning).code,
            "already_running"
        );
        assert!(prep.begin(&b, &cancel).is_ok(), "profile B is independent");

        // one preparation at a time overall (unchanged), and its own profile cannot be launched
        assert!(matches!(prep.begin(&b, &cancel), Err(PrepareError::Busy)));
        assert_eq!(launch.claim_checked(&b), Err(Activity::Preparing));
        assert!(launch.claim_checked(&InstanceId::new("c").unwrap()).is_ok());

        // ending each operation frees its profile again
        launch.release(&a);
        *lock(&prep.running) = None;
        prep.locks.release("b");
        assert!(prep.begin(&a, &cancel).is_ok());
        assert_eq!(launch.claim_checked(&a), Err(Activity::Preparing));
        *lock(&prep.running) = None;
        prep.locks.release("a");
        assert!(launch.claim(&a));
        assert!(launch.claim(&b), "b is free again too");
    }

    #[test]
    fn the_internal_state_moves_idle_preparing_failed_idle() {
        let d = tempfile::tempdir().unwrap();
        let s = state(PortableRoot::at(d.path()));
        assert_eq!(s.status("a").state, "idle");
        *lock(&s.running) = Some((InstanceId::new("a").unwrap(), Cancel::new()));
        assert_eq!(s.status("a").state, "preparing");
        assert_eq!(s.status("b").state, "idle", "other profiles are unaffected");
        *lock(&s.running) = None;
        lock(&s.failed).insert("a".into(), (&PrepareError::Offline("x".into())).into());
        let st = s.status("a");
        assert_eq!(
            (st.state, st.error.as_ref().map(|e| e.code)),
            ("failed", Some("offline"))
        );
        lock(&s.failed).remove("a");
        assert_eq!(s.status("a").state, "idle");
    }

    #[test]
    fn startup_sweeps_scratch_but_keeps_verified_and_partial_downloads() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        std::fs::create_dir_all(root.tmp_dir().join("natives-abc/out")).unwrap();
        std::fs::write(root.tmp_dir().join("junk.bin"), b"x").unwrap();
        std::fs::create_dir_all(root.libraries_dir()).unwrap();
        std::fs::write(root.libraries_dir().join("kept.jar"), b"ok").unwrap();
        std::fs::write(root.libraries_dir().join("half.jar.part"), b"par").unwrap();
        assert_eq!(sweep_scratch(&root).unwrap(), 2);
        assert_eq!(std::fs::read_dir(root.tmp_dir()).unwrap().count(), 0);
        assert!(root.libraries_dir().join("kept.jar").is_file());
        assert!(root.libraries_dir().join("half.jar.part").is_file());
    }

    #[test]
    fn failures_carry_stable_codes_and_plain_messages() {
        for (e, code) in [
            (PrepareError::Cancelled, "cancelled"),
            (PrepareError::Offline("x".into()), "offline"),
            (PrepareError::Verification("a.jar".into()), "verification"),
            (PrepareError::Busy, "busy"),
        ] {
            let f = PrepareFailure::from(&e);
            assert_eq!(f.code, code);
            assert!(!f.message.is_empty() && !f.message.contains("Err("));
        }
        let json = serde_json::to_string(&PrepareFailure::from(&PrepareError::NotFound)).unwrap();
        assert!(json.contains("not_found"));
    }
}
