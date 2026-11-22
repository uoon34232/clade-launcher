//! Launching a prepared profile: the checks that come before, the process, and the commands.
//!
//! [`plan_launch`] is the whole pre-launch pipeline and never starts anything, so every way it
//! can refuse is tested without a process. In order, it needs:
//! 1. a profile that is Ready (otherwise "set it up first");
//! 2. a selected, launch-ready Minecraft account (no account, or one that must sign in again,
//!    stops here with a structured reason; there is **no offline or guest mode**);
//! 3. the lightweight integrity check of the prepared files (clearly missing or wrong-sized files
//!    refuse the launch and ask for a repair through the preparation system; same-size corruption
//!    is what a full verification run is for, so thousands of assets are not re-hashed on every
//!    launch);
//! 4. the profile's own, still verified Java runtime (never `PATH`, `JAVA_HOME` or the registry);
//! 5. fresh credentials, through the existing refresh hook (`auth::ensure_fresh`): a refresh the
//!    person must fix means "Sign in again"; a locked vault means "unlock"; Mojang's pending
//!    approval is reported as its own reason. Account and instance are independent: nothing about
//!    the instance is read from or written for the account.
//!
//! Then the process is started directly (no shell) in the instance's `game/` folder, and a
//! supervisor task (`game_session`) watches it, keeps the playtime session, writes the session
//! log and reports how it ended. The task does not depend on the window: closing Clade while the
//! game runs hides the window and keeps Clade alive, without a tray icon or service, until the
//! game exits (see `lib.rs`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
use crate::accounts_api::AccountsState;
use crate::auth::{ensure_fresh, AuthState};
use crate::coded_error::CodedError;
use crate::game_session::{logs_dir, supervise, ExitKind, ExitSummary, Watch, GRACE};
use crate::instances::{
    Instance, InstanceError, InstanceId, InstanceStore, JavaChoice, SetupState,
};
use crate::java_install::find_installed;
use crate::launch_args::{
    build, resolve, BuildInput, GameCommand, Identity, LaunchError, Resolved,
};
use crate::loader_install::SystemEnv;
use crate::ms_auth::AuthError;
use crate::net::Transport;
use crate::playtime::{ActiveSession, LiveProcess, PlaytimeStore, SessionTracker};
use crate::portable::{fsutil, PortableRoot};
use crate::prepare::check_installed;
use crate::profile_lock::{Activity, ProfileLocks};
use crate::profiles::now_unix;
use crate::runtimes::{resolve_java, ManagedJava};
use crate::ui_settings::SettingsState;
use crate::vault::Vault;

pub const EVENT: &str = "launch-state";
const HEARTBEAT: Duration = Duration::from_secs(60);

// ------------------------------------------------------------------------------ failures

/// Why a launch did not happen: a stable code, a plain message, and for sign-in problems the
/// account concerned.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchFailure {
    pub code: &'static str,
    /// English detail for logs; the UI words the failure from `code`.
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

fn fail(code: &'static str, message: impl Into<String>) -> LaunchFailure {
    LaunchFailure {
        code,
        message: message.into(),
        account_id: None,
    }
}

fn needs_repair(what: &str) -> LaunchFailure {
    fail("needs_repair", format!("Part of this profile is missing or damaged ({what}). It needs to be repaired before it can start."))
}

fn from_auth(e: &AuthError, account: &str) -> LaunchFailure {
    let mut f = match e {
        AuthError::VaultLocked => fail("vault_locked", "Unlock your accounts to play."),
        AuthError::NeedsSignIn(_) => fail("needs_sign_in", "This account needs to sign in again before it can play."),
        AuthError::Offline => fail("offline", "Clade can't refresh your Minecraft sign-in without a network. Connect and try again."),
        AuthError::ApprovalRequired => fail("approval_pending", "Mojang has not approved this launcher for Minecraft sign-in yet, so a fresh sign-in can't be completed."),
        AuthError::NoJavaProfile { .. } => fail("no_java_profile", e.to_string()),
        other => fail("auth", other.to_string()),
    };
    f.account_id = Some(account.to_owned());
    f
}

// ------------------------------------------------------------------------------ state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Claimed: checks and the token refresh are running, or the process has just started.
    Launching,
    Playing,
}

struct Entry {
    phase: Phase,
    /// Set once the process exists.
    pid: Option<u32>,
}

struct StateInner {
    sessions: HashMap<String, Entry>,
    exit_when_idle: bool,
    closing: bool,
}

/// The running games, managed by Tauri.
pub struct LaunchState {
    pub root: PortableRoot,
    sessions: Mutex<StateInner>,
    locks: Arc<ProfileLocks>,
}

impl LaunchState {
    pub fn new(root: PortableRoot) -> Self {
        Self {
            root,
            sessions: Mutex::new(StateInner {
                sessions: HashMap::new(),
                exit_when_idle: false,
                closing: false,
            }),
            locks: Arc::new(ProfileLocks::default()),
        }
    }

    /// Shares the per-profile lock registry with preparation (see `profile_lock`).
    pub fn with_locks(mut self, locks: Arc<ProfileLocks>) -> Self {
        self.locks = locks;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StateInner> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reserves the instance so a second Play click cannot start a second game. `false` if taken.
    pub fn claim(&self, id: &InstanceId) -> bool {
        self.claim_checked(id).is_ok()
    }

    /// [`Self::claim`] that also says what the profile is busy with when it is refused: the game is
    /// already starting or running (`Launching`), or its files are being prepared (`Preparing`).
    pub fn claim_checked(&self, id: &InstanceId) -> Result<(), Activity> {
        let mut inner = self.lock();
        if inner.closing {
            return Err(Activity::Launching);
        }
        if inner.sessions.contains_key(id.as_str()) {
            return Err(Activity::Launching);
        }
        self.locks.claim(id.as_str(), Activity::Launching)?;
        inner.sessions.insert(
            id.to_string(),
            Entry {
                phase: Phase::Launching,
                pid: None,
            },
        );
        Ok(())
    }

    fn set_pid(&self, id: &InstanceId, pid: u32) {
        if let Some(e) = self.lock().sessions.get_mut(id.as_str()) {
            e.pid = Some(pid);
        }
    }

    pub(crate) fn mark_playing(&self, id: &InstanceId) {
        if let Some(e) = self.lock().sessions.get_mut(id.as_str()) {
            e.phase = Phase::Playing;
        }
    }

    pub fn release(&self, id: &InstanceId) {
        let mut inner = self.lock();
        inner.sessions.remove(id.as_str());
        self.locks.release(id.as_str());
    }

    /// Atomically removes `id`, releases its profile lock, and returns `(exit_when_idle, another_running)`
    /// under the same mutex as [`Self::close_requested`] so closing the window cannot race game exit.
    pub fn finish_session(&self, id: &InstanceId) -> (bool, bool) {
        let mut inner = self.lock();
        inner.sessions.remove(id.as_str());
        self.locks.release(id.as_str());
        let closed = inner.exit_when_idle;
        let another_running = inner
            .sessions
            .values()
            .any(|e| e.pid.is_some() || (closed && e.phase == Phase::Launching));
        (closed, another_running)
    }

    /// Called when the window is asked to close: under one lock, if any game process is alive, sets
    /// `exit_when_idle = true` and returns `true` (hide instead of quitting); otherwise latches
    /// `closing = true` and returns `false` so pre-spawn launches cannot spawn after close starts.
    pub fn close_requested(&self) -> bool {
        let mut inner = self.lock();
        if inner.sessions.values().any(|e| e.pid.is_some()) {
            inner.exit_when_idle = true;
            true
        } else {
            inner.exit_when_idle = false;
            inner.closing = true;
            false
        }
    }

    /// True once [`Self::close_requested`] has allowed the window to exit.
    pub fn is_closing(&self) -> bool {
        self.lock().closing
    }

    /// Is a game process alive (started and not yet exited)? Closing the window while this is
    /// true must not end Clade.
    pub fn has_live_game(&self) -> bool {
        self.lock().sessions.values().any(|e| e.pid.is_some())
    }

    /// Is a game either live or currently starting (`Phase::Launching`)?
    pub fn is_busy(&self) -> bool {
        let g = self.lock();
        g.sessions
            .values()
            .any(|e| e.pid.is_some() || e.phase == Phase::Launching)
    }

    /// Is some game still *starting* (its credentials are being fetched or its process is coming up)?
    /// A game that is already running is not: that is `Phase::Playing`. "Start fresh" waits only for
    /// the first kind (see `vault_recovery`).
    pub fn any_starting(&self) -> bool {
        self.lock()
            .sessions
            .values()
            .any(|e| e.phase == Phase::Launching)
    }

    pub fn set_exit_when_idle(&self, on: bool) {
        self.lock().exit_when_idle = on;
    }

    pub fn exit_when_idle(&self) -> bool {
        self.lock().exit_when_idle
    }

    fn snapshot(&self) -> Vec<(String, &'static str)> {
        self.lock()
            .sessions
            .iter()
            .map(|(id, e)| {
                (
                    id.clone(),
                    if e.phase == Phase::Playing {
                        "playing"
                    } else {
                        "launching"
                    },
                )
            })
            .collect()
    }
}

// ------------------------------------------------------------------------------ planning

pub struct Planned {
    pub instance: Instance,
    pub command: GameCommand,
    pub account_id: String,
}

fn java_for(
    root: &PortableRoot,
    inst: &Instance,
    component: &str,
) -> Result<ManagedJava, LaunchFailure> {
    match &inst.java {
        JavaChoice::Runtime { id } => {
            resolve_java(root, id).map_err(|_| needs_repair("the chosen Java runtime"))
        }
        JavaChoice::Auto => {
            find_installed(root, component).ok_or_else(|| needs_repair("the Java runtime"))
        }
    }
}

/// Facts about the moment and the machine, passed in so planning is deterministic in tests.
pub struct LaunchEnv<'a> {
    pub now: i64,
    pub sys: &'a SystemEnv,
    pub total_memory_mb: Option<u64>,
    /// The global game memory chosen in Settings (`None` = automatic). A profile's own setting wins.
    pub default_memory_mb: Option<u32>,
    /// Live physical + commit memory availability (`None` in purely static planning tests).
    pub memory_snapshot: Option<crate::system::MemorySnapshot>,
    /// Framed stage resolution override when launching inside Clade (`None` in standalone mode).
    pub embed_resolution: Option<crate::instances::Resolution>,
}

/// Everything that must be true before the game may start, ending in the ready-to-run command.
pub async fn plan_launch<T: Transport>(
    root: &PortableRoot,
    transport: &T,
    vault: &Mutex<Option<Vault>>,
    accounts: &AccountsState,
    id: &InstanceId,
    env: &LaunchEnv<'_>,
) -> Result<Planned, LaunchFailure> {
    let now = env.now;
    let total_memory_mb = env.total_memory_mb;
    let default_memory_mb = env.default_memory_mb;
    let memory_snapshot = env.memory_snapshot;
    let embed_resolution = env.embed_resolution;
    let sys = env.sys.clone();

    let root_pre = root.clone();
    let id_pre = id.clone();
    let accounts_path = accounts.path().to_path_buf();

    let (inst, meta, resolved, java) = tokio::task::spawn_blocking(
        move || -> Result<(Instance, AccountMeta, Resolved, ManagedJava), LaunchFailure> {
            let store = InstanceStore::new(&root_pre);
            let inst = store
                .get(&id_pre)
                .map_err(|_| fail("not_found", "That profile no longer exists."))?;
            let SetupState::Ready { version_id, .. } = &inst.setup else {
                return Err(fail(
                    "not_ready",
                    "This profile has to be set up before it can start.",
                ));
            };

            // The selected account (global convenience state; the instance is never touched for it).
            let file = AccountsFile::load(&accounts_path).map_err(|e| {
                fail(
                    "storage",
                    format!("The account list could not be read: {e}"),
                )
            })?;
            let meta = file
                .effective_active()
                .and_then(|a| file.accounts.iter().find(|m| m.id == a))
                .ok_or_else(|| fail("no_account", "Sign in with a Microsoft account to play."))?
                .clone();
            if matches!(meta.status, AccountStatus::NeedsReauth { .. }) {
                let mut f = fail(
                    "needs_sign_in",
                    format!(
                        "{} needs to sign in again before it can play.",
                        meta.username
                    ),
                );
                f.account_id = Some(meta.id.clone());
                return Err(f);
            }

            // Lightweight integrity: existence and sizes only.
            check_installed(&root_pre, &inst).map_err(|what| needs_repair(&what))?;

            let resolved = resolve(&root_pre, version_id.as_str()).map_err(|e| match e {
                LaunchError::MissingVersion(_) | LaunchError::MissingFile(_) => {
                    needs_repair("the version information")
                }
                LaunchError::UnsupportedFolderPath => {
                    fail("unsupported_folder_path", e.to_string())
                }
                other => fail(
                    "unsupported",
                    format!("This profile can't be started: {other}."),
                ),
            })?;
            let java = java_for(&root_pre, &inst, &resolved.java.component)?;
            Ok((inst, meta, resolved, java))
        },
    )
    .await
    .map_err(|e| fail("storage", format!("Launch planning task failed: {e}")))??;

    // Credentials, through the same hook preparation of tokens uses everywhere.
    let token = ensure_fresh(transport, vault, accounts, &meta.id, now)
        .await
        .map_err(|e| from_auth(&e, &meta.id))?;

    let root_post = root.clone();
    let id_post = id.clone();
    let account_id = meta.id.clone();

    tokio::task::spawn_blocking(move || -> Result<Planned, LaunchFailure> {
        let store = InstanceStore::new(&root_post);
        let dirs = store.dirs(&id_post);
        let temp = root_post.game_tmp_dir().join(id_post.as_str());
        for d in [
            dirs.game_dir(),
            temp.clone(),
            dirs.home_dir().join("AppData").join("Roaming"),
            dirs.home_dir().join("AppData").join("Local"),
        ] {
            fsutil::reject_links_between(root_post.path(), &d).map_err(|e| {
                fail(
                    "storage",
                    format!("A folder for the game could not be created: {e}"),
                )
            })?;
            std::fs::create_dir_all(&d).map_err(|e| {
                fail(
                    "storage",
                    format!("A folder for the game could not be created: {e}"),
                )
            })?;
            fsutil::reject_links_between(root_post.path(), &d).map_err(|e| {
                fail(
                    "storage",
                    format!("A folder for the game could not be created: {e}"),
                )
            })?;
        }
        fsutil::reject_links_between(root_post.path(), &dirs.natives_dir()).map_err(|e| {
            fail(
                "storage",
                format!("A folder for the game could not be created: {e}"),
            )
        })?;
        let effective_mb = crate::system::effective_memory_mb(
            inst.memory_mb,
            default_memory_mb,
            total_memory_mb.or_else(|| memory_snapshot.map(|s| s.total_mb)),
        );
        let memory_mb = if memory_snapshot.is_some() {
            let clamped =
                crate::system::launch_memory_mb(inst.memory_mb, default_memory_mb, memory_snapshot);
            if clamped < effective_mb {
                tracing::info!(
                    effective_mb,
                    clamped_mb = clamped,
                    "clamped game heap to fit currently available memory"
                );
            }
            clamped
        } else {
            effective_mb
        };
        let command = build(&BuildInput {
            root: &root_post,
            dirs: &dirs,
            resolved: &resolved,
            java: &java,
            identity: &Identity {
                uuid: meta.minecraft_uuid.clone(),
                name: meta.username.clone(),
                access_token: token,
            },
            memory_mb,
            resolution: embed_resolution.or(inst.resolution),
            extra_jvm_args: &inst.extra_jvm_args,
            sys: &sys,
            temp_dir: &temp,
        })
        .map_err(|e| match e {
            LaunchError::MissingFile(what) => needs_repair(&what),
            LaunchError::UnsupportedFolderPath => fail("unsupported_folder_path", e.to_string()),
            other => fail(
                "unsupported",
                format!("This profile can't be started: {other}."),
            ),
        })?;
        Ok(Planned {
            instance: inst,
            command,
            account_id,
        })
    })
    .await
    .map_err(|e| fail("storage", format!("Launch planning task failed: {e}")))?
}

// ------------------------------------------------------------------------------ events

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitDto {
    pub code: Option<i32>,
    pub kind: ExitKind,
    /// Stable reason code the UI words in the selected language.
    pub reason: &'static str,
    /// English detail for logs.
    pub message: String,
    pub seconds: u64,
}

impl From<&ExitSummary> for ExitDto {
    fn from(e: &ExitSummary) -> Self {
        Self {
            code: e.code,
            kind: e.kind,
            reason: e.reason,
            message: e.message.clone(),
            seconds: e.seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchEvent {
    pub instance_id: String,
    /// "launching", "playing" or "stopped".
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<ExitDto>,
}

fn emit(app: &AppHandle, id: &InstanceId, state: &'static str, exit: Option<ExitDto>) {
    let _ = app.emit(
        EVENT,
        LaunchEvent {
            instance_id: id.to_string(),
            state,
            exit,
        },
    );
}

// ------------------------------------------------------------------------------ commands

#[tauri::command]
pub async fn launch_profile(
    app: AppHandle,
    state: State<'_, LaunchState>,
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    id: String,
) -> Result<(), LaunchFailure> {
    let iid =
        InstanceId::new(&id).map_err(|_| fail("not_found", "That profile no longer exists."))?;
    if state.is_closing() {
        return Err(fail("spawn_failed", "Clade is closing."));
    }
    match state.claim_checked(&iid) {
        Ok(()) => {}
        Err(Activity::Launching) => {
            return Err(fail(
                "already_running",
                "This profile is already starting or running.",
            ))
        }
        Err(Activity::Preparing) => {
            return Err(fail(
                "already_preparing",
                "This profile is being set up. Wait for it to finish.",
            ))
        }
    }
    emit(&app, &iid, "launching", None);
    let result = start(&app, &state, &auth, &accounts, &iid).await;
    if let Err(e) = &result {
        tracing::info!(code = e.code, "launch did not start");
        let (closed, another_running) = state.finish_session(&iid);
        emit(&app, &iid, "stopped", None);
        if closed {
            if !another_running {
                app.exit(0);
            }
        } else if !state.is_closing() {
            // A launch that could not start is always shown, even if the window was hidden.
            reveal(&app);
        }
    }
    result
}

async fn start(
    app: &AppHandle,
    state: &LaunchState,
    auth: &AuthState,
    accounts: &AccountsState,
    id: &InstanceId,
) -> Result<(), LaunchFailure> {
    let transport = auth
        .http
        .as_ref()
        .map_err(|_| fail("offline", "Networking is unavailable."))?;
    let behavior = app.state::<SettingsState>().behavior();
    let wants_embed = behavior.embeds_on_launch() && crate::game_embed::can_embed(app);
    let planned = plan_launch(
        &state.root,
        transport,
        &auth.vault,
        accounts,
        id,
        &LaunchEnv {
            now: now_unix(),
            sys: &SystemEnv::from_process(),
            total_memory_mb: crate::system::total_memory_mb(),
            default_memory_mb: app.state::<SettingsState>().default_memory_mb(),
            memory_snapshot: crate::system::memory_snapshot(),
            embed_resolution: wants_embed
                .then(|| crate::game_embed::stage_resolution(app))
                .flatten(),
        },
    )
    .await?;

    let account_id = planned.account_id.clone();
    let header = planned.command.redacted(&state.root);
    let secrets = zeroize::Zeroizing::new(planned.command.secrets().to_vec());

    // Direct process creation (no shell). The child is not tied to this process: if Clade ends,
    // the game does not. Its console output goes to a launcher-owned file, not to pipes: a pipe's
    // reader dies with Clade and the game would then stop on its own a little later. If the file cannot
    // be created, the old piped output is used (the game then lives only as long as Clade does).
    let mut process = tokio::process::Command::from(planned.command.to_process());
    // Zeroize our Rust-side copy of the access token in RAM immediately now that `Command` holds the OS argv.
    drop(planned);
    process.stdin(std::process::Stdio::null());
    let output = match crate::game_session::open_output(&state.root, id) {
        Ok((file, path)) => match file.try_clone() {
            Ok(second) => {
                process
                    .stdout(std::process::Stdio::from(file))
                    .stderr(std::process::Stdio::from(second));
                Some(path)
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not share the game output file, using pipes");
                None
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "could not create the game output file, using pipes");
            None
        }
    };
    if output.is_none() {
        process
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }
    let (child, pid) = {
        let mut inner = state.lock();
        if inner.closing {
            return Err(fail("spawn_failed", "Clade is closing."));
        }
        let child = process
            .spawn()
            .map_err(|e| fail("spawn_failed", format!("Java could not be started: {e}")))?;
        let pid = child.id().unwrap_or(0);
        if let Some(e) = inner.sessions.get_mut(id.as_str()) {
            e.pid = Some(pid);
        }
        tracing::info!(pid, "game process started");
        (child, pid)
    };

    // The playtime session starts only now, after a successful spawn.
    let tracker =
        match PlaytimeStore::new(&state.root).begin_session(id, pid, now_unix(), Instant::now()) {
            Ok(t) => Some(t.with_unbiased_clock()),
            Err(e) => {
                tracing::warn!(error = %e, "could not start the playtime session");
                None
            }
        };
    let _ = accounts.update(|f| {
        if let Some(a) = f.accounts.iter_mut().find(|a| a.id == account_id) {
            a.last_used_at = Some(now_unix());
        }
    });

    // Attach the game window inside Clade's frame when "Inside Clade" is enabled, or hide
    // Clade completely when "Hide Clade" is selected.
    if wants_embed {
        crate::game_embed::start_session(app.clone(), id.as_str().to_owned(), pid);
    } else if behavior.hides_on_launch() {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
        crate::glass::suspend(app);
        crate::webview::suspend_hidden_window(app);
    }

    let (app, id, root) = (app.clone(), id.clone(), state.root.clone());
    tokio::spawn(async move {
        let playing_app = app.clone();
        let playing_id = id.clone();
        let outcome = supervise(
            child,
            Watch {
                root: &root,
                instance: &id,
                secrets: secrets.as_slice(),
                header: &header,
                output: output.as_deref(),
                tracker,
                heartbeat: HEARTBEAT,
                grace: GRACE,
                now_unix,
            },
            move || {
                playing_app.state::<LaunchState>().mark_playing(&playing_id);
                emit(&playing_app, &playing_id, "playing", None);
                if let Some(w) = playing_app.get_webview_window("main") {
                    if !w.is_visible().unwrap_or(true) {
                        crate::glass::suspend(&playing_app);
                        crate::webview::suspend_hidden_window(&playing_app);
                    } else if !w.is_focused().unwrap_or(true) {
                        crate::webview::throttle_unfocused_window(&playing_app);
                    }
                }
            },
        )
        .await;
        drop(secrets);
        let tmp_dir = root.game_tmp_dir().join(id.as_str());
        if fsutil::reject_links_between(root.path(), &tmp_dir).is_ok() {
            let _ = fsutil::remove_dir_all_with_retry(&tmp_dir);
        }
        if let Some(path) = &output {
            if fsutil::reject_links_between(root.path(), path).is_ok() {
                let _ = fsutil::remove_file_with_retry(path);
            }
        }
        let ls = app.state::<LaunchState>();
        let (closed, another_running) = ls.finish_session(&id);
        tracing::info!(code = ?outcome.exit.code, kind = ?outcome.exit.kind, "game process ended");
        crate::game_embed::end_session(&app, id.as_str());
        crate::webview::resume_window(&app);
        emit(&app, &id, "stopped", Some((&outcome.exit).into()));
        // What happens to the window now that the game is over.
        apply_window_plan(&app, outcome.exit.kind, closed, another_running);
    });
    Ok(())
}

fn read_output_tail(path: &std::path::Path) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let skip = len.saturating_sub(64 * 1024);
    if skip > 0 && f.seek(SeekFrom::Start(skip)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.take(64 * 1024).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    String::from_utf8_lossy(&buf)
        .lines()
        .rev()
        .take(250)
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

const MAX_ACTIVE_JOURNAL_BYTES: u64 = 256 * 1024;

/// Recovers sessions whose game process exited since the early startup sweep, opens verified
/// handles for every still-live session in a single pass, and banks any session that exited in
/// the brief window between recovery and handle acquisition.
fn prepare_reattach_sessions(
    root: &PortableRoot,
) -> std::io::Result<Vec<(PathBuf, ActiveSession, LiveProcess)>> {
    let store = PlaytimeStore::new(root);
    match store.recover() {
        Ok(recovered) if !recovered.is_empty() => {
            tracing::info!(
                count = recovered.len(),
                "banked playtime sessions that ended during launcher startup"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not bank finished playtime sessions before reattach"
            );
        }
    }

    let live = store.live_sessions_with_handles()?;
    let live_paths: HashSet<PathBuf> = live.iter().map(|(path, _, _)| path.clone()).collect();
    bank_orphaned_dead_sessions(root, &store, &live_paths);
    Ok(live)
}

fn bank_orphaned_dead_sessions(
    root: &PortableRoot,
    store: &PlaytimeStore<'_>,
    live_paths: &HashSet<PathBuf>,
) {
    let active_dir = root.data_dir().join("playtime-active");
    if fsutil::reject_links_between(root.path(), &active_dir).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&active_dir) else {
        return;
    };
    let now = Instant::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") || live_paths.contains(&path) {
            continue;
        }
        if fsutil::reject_links_between(root.path(), &path).is_err() {
            continue;
        }
        let Ok(bytes) = fsutil::read_bounded(&path, MAX_ACTIVE_JOURNAL_BYTES) else {
            continue;
        };
        let Ok(session) = serde_json::from_slice::<ActiveSession>(&bytes) else {
            continue;
        };
        if crate::playtime::is_session_process_alive(&session) {
            continue;
        }
        let last_hb = session.last_heartbeat_at;
        if let Err(e) = SessionTracker::resume(path, session, now).finish(store, last_hb, now) {
            tracing::warn!(
                error = %e,
                "could not bank playtime session that ended during reattachment"
            );
        }
    }
}

/// Reattaches any live game processes recorded in `data/playtime-active/` at startup so the UI
/// marks them Playing, their playtime keeps ticking, and their eventual exit is handled cleanly.
pub fn reattach_live_sessions(app: &AppHandle) {
    let state = app.state::<LaunchState>();
    let sessions = match prepare_reattach_sessions(&state.root) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "could not inspect live playtime sessions");
            return;
        }
    };
    for (journal_path, session, proc_handle) in sessions {
        let id = session.instance_id.clone();
        let pid = session.pid;
        if !state.claim(&id) {
            continue;
        }
        state.set_pid(&id, pid);
        state.mark_playing(&id);
        emit(app, &id, "playing", None);
        tracing::info!(pid, instance = %id, "reattached live game session");
        if app.state::<SettingsState>().behavior().embeds_on_launch()
            && crate::game_embed::can_embed(app)
        {
            crate::game_embed::start_session(app.clone(), id.as_str().to_owned(), pid);
        }

        let app = app.clone();
        let root = state.root.clone();
        tauri::async_runtime::spawn(async move {
            let mut tracker =
                SessionTracker::resume(journal_path, session, Instant::now()).with_unbiased_clock();

            let mut wait_task = tokio::task::spawn_blocking(move || proc_handle.wait_blocking());
            let mut heartbeat = tokio::time::interval(HEARTBEAT);
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            heartbeat.tick().await;
            let raw_exit = loop {
                tokio::select! {
                    res = &mut wait_task => break res.ok().flatten(),
                    _ = heartbeat.tick() => {
                        if let Err(e) = tracker.tick(now_unix(), Instant::now()) {
                            tracing::warn!(error = %e, "playtime heartbeat failed");
                        }
                    }
                }
            };

            let root_finish = root.clone();
            let id_finish = id.clone();
            let exit = tokio::task::spawn_blocking(move || {
                let banked_secs = match tracker.finish(
                    &PlaytimeStore::new(&root_finish),
                    now_unix(),
                    Instant::now(),
                ) {
                    Ok(b) => b.seconds,
                    Err(e) => {
                        tracing::warn!(error = %e, "could not bank reattached playtime session");
                        0
                    }
                };

                let out_path =
                    crate::game_session::output_dir(&root_finish).join(format!("{id_finish}.out"));
                let tail_lines = read_output_tail(&out_path);
                let status = raw_exit.map(|c| {
                    use std::os::windows::process::ExitStatusExt;
                    std::process::ExitStatus::from_raw(c)
                });
                let mut exit = crate::game_session::summarize(
                    status,
                    Duration::from_secs(banked_secs.max(15)),
                    &tail_lines,
                );
                exit.seconds = banked_secs;

                let tmp_dir = root_finish.game_tmp_dir().join(id_finish.as_str());
                if fsutil::reject_links_between(root_finish.path(), &tmp_dir).is_ok() {
                    let _ = fsutil::remove_dir_all_with_retry(&tmp_dir);
                }
                if fsutil::reject_links_between(root_finish.path(), &out_path).is_ok() {
                    let _ = fsutil::remove_file_with_retry(&out_path);
                }
                exit
            })
            .await
            .unwrap_or_else(|_| crate::game_session::summarize(None, Duration::from_secs(15), &[]));

            let ls = app.state::<LaunchState>();
            let (closed, another_running) = ls.finish_session(&id);
            tracing::info!(
                code = ?exit.code,
                kind = ?exit.kind,
                "reattached game process ended"
            );
            crate::game_embed::end_session(&app, id.as_str());
            crate::webview::resume_window(&app);
            emit(&app, &id, "stopped", Some((&exit).into()));
            apply_window_plan(&app, exit.kind, closed, another_running);
        });
    }
}

/// What to do with Clade's window when a game has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPlan {
    /// Leave the window as it is.
    Leave,
    /// Bring the window to the front (show, restore, focus).
    Reveal,
    /// Finish quitting: Clade has nothing left to do and no window the person is looking at.
    Quit,
}

/// The rules for the window after a game ended:
/// - another game is still running: nothing changes yet;
/// - Clade's window was closed during the game: the delayed quit finishes (unchanged behaviour);
/// - the game failed to start or crashed: the window **always** comes back, so the explanation can be
///   seen. "Stay hidden" never hides a failure;
/// - a normal exit follows "When Minecraft closes": a window already on screen is left alone, a
///   hidden one is shown again, or, with "Stay hidden", Clade quietly ends with the game (it has no
///   tray and no background service; starting it again brings it up).
pub fn window_plan(
    kind: ExitKind,
    closed_during_game: bool,
    another_game_running: bool,
    window_visible: bool,
    stay_hidden: bool,
) -> WindowPlan {
    if another_game_running {
        return WindowPlan::Leave;
    }
    if closed_during_game {
        return WindowPlan::Quit;
    }
    if kind != ExitKind::Normal {
        return WindowPlan::Reveal;
    }
    if window_visible {
        return WindowPlan::Leave;
    }
    if stay_hidden {
        WindowPlan::Quit
    } else {
        WindowPlan::Reveal
    }
}

fn reveal(app: &AppHandle) {
    crate::webview::resume_window(app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    crate::glass::resume(app);
}

fn apply_window_plan(app: &AppHandle, kind: ExitKind, closed: bool, another_running: bool) {
    let visible = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    let stay_hidden = app
        .state::<SettingsState>()
        .behavior()
        .stays_hidden_on_exit();
    let still_launching = !closed && !another_running && app.state::<LaunchState>().any_starting();
    match window_plan(kind, closed, another_running, visible, stay_hidden) {
        WindowPlan::Leave => {
            if !visible && another_running {
                crate::webview::suspend_hidden_window(app);
            }
        }
        WindowPlan::Reveal => reveal(app),
        WindowPlan::Quit if still_launching => {
            if !visible {
                crate::webview::suspend_hidden_window(app);
            }
        }
        WindowPlan::Quit => app.exit(0),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningDto {
    pub instance_id: String,
    pub state: &'static str,
}

/// Which games are starting or running (so a reloaded window shows the right state).
#[tauri::command]
pub fn launch_status(state: State<'_, LaunchState>) -> Vec<RunningDto> {
    state
        .snapshot()
        .into_iter()
        .map(|(instance_id, state)| RunningDto { instance_id, state })
        .collect()
}

fn resolve_game_logs_dir(root: &PortableRoot, iid: &InstanceId) -> Result<PathBuf, CodedError> {
    InstanceStore::new(root).get(iid).map_err(|e| match e {
        InstanceError::NotFound(_) | InstanceError::BadId(_) => {
            CodedError::new("not_found", e.to_string())
        }
        other => CodedError::storage(other),
    })?;
    let dir = logs_dir(root, iid);
    fsutil::reject_links_between(root.path(), &dir).map_err(CodedError::storage)?;
    std::fs::create_dir_all(&dir).map_err(CodedError::storage)?;
    fsutil::reject_links_between(root.path(), &dir).map_err(CodedError::storage)?;
    Ok(dir)
}

/// Opens the folder that holds this profile's game session logs.
#[tauri::command]
pub async fn open_game_logs(
    app: AppHandle,
    state: State<'_, LaunchState>,
    id: String,
) -> Result<(), CodedError> {
    let iid = InstanceId::new(&id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let root = state.root.clone();
    tokio::task::spawn_blocking(move || {
        let dir = resolve_game_logs_dir(&root, &iid)?;
        app.opener()
            .open_path(dir.to_string_lossy(), None::<&str>)
            .map_err(|e| CodedError::new("open_failed", e.to_string()))
    })
    .await
    .map_err(|e| CodedError::new("open_failed", e.to_string()))?
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::accounts::{AccountMeta, AccountStatus};
    use crate::instances::{InstanceStore, NewInstance};
    use crate::net::testing_transport::FakeTransport;
    use crate::prepare::prepare_instance;
    use crate::prepare::tests::{create, ctx, world, Era, World};
    use crate::vault::{AccountSecrets, KdfParams};
    use std::fs;
    use std::path::Path;

    const FAST_KDF: KdfParams = KdfParams {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        let link_win = link.to_string_lossy().replace('/', "\\");
        let target_win = target.to_string_lossy().replace('/', "\\");
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link_win)
            .arg(target_win)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn seed_ready_account(w: &World) -> (AccountsState, Mutex<Option<Vault>>) {
        let state = AccountsState::new(w.root.accounts_file());
        state
            .update(|f| {
                f.accounts.push(AccountMeta {
                    id: "acc-a".into(),
                    username: "Alex".into(),
                    minecraft_uuid: "00000000-0000-4000-8000-00000000a001".into(),
                    added_at: 1,
                    last_used_at: None,
                    status: AccountStatus::Ready,
                });
                f.active = Some("acc-a".into());
            })
            .unwrap();
        let mut vault =
            Vault::create_with(&w.root.vault_file(), "correct horse battery", FAST_KDF).unwrap();
        vault
            .set_account(
                "acc-a",
                AccountSecrets {
                    ms_refresh_token: Some("fixture-refresh".into()),
                    minecraft_access_token: Some("FIXTURE-TOKEN-A-not-a-credential".into()),
                    minecraft_token_expires_at: Some(101_000),
                },
            )
            .unwrap();
        (state, Mutex::new(Some(vault)))
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn plan_launch_rejects_links_on_game_home_temp_and_appdata() {
        let w = world("1.21.4", Era::Modern);
        let id = create(&w, NewInstance::vanilla("Profile", "1.21.4").unwrap());
        prepare_instance(&ctx(&w), &id).await.unwrap();
        let (accounts, vault) = seed_ready_account(&w);
        let transport = FakeTransport::new();
        let sys = SystemEnv::from_process();
        let env = LaunchEnv {
            now: 1_000,
            sys: &sys,
            total_memory_mb: Some(16_384),
            default_memory_mb: None,
            memory_snapshot: None,
            embed_resolution: None,
        };

        let outside = tempfile::tempdir().unwrap();
        let dirs = InstanceStore::new(&w.root).dirs(&id);

        // Junction at game/
        fs::remove_dir_all(dirs.game_dir()).unwrap();
        create_junction(&dirs.game_dir(), outside.path());
        let err = plan_launch(&w.root, &transport, &vault, &accounts, &id, &env)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "storage");
        fs::remove_dir(dirs.game_dir()).unwrap();
        fs::create_dir_all(dirs.game_dir()).unwrap();

        // Junction at home/ must not create outside/AppData
        create_junction(&dirs.home_dir(), outside.path());
        let err = plan_launch(&w.root, &transport, &vault, &accounts, &id, &env)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "storage");
        assert!(!outside.path().join("AppData").exists());
        fs::remove_dir(dirs.home_dir()).unwrap();

        // Junction at home/AppData/Roaming
        let appdata = dirs.home_dir().join("AppData");
        fs::create_dir_all(&appdata).unwrap();
        create_junction(&appdata.join("Roaming"), outside.path());
        let err = plan_launch(&w.root, &transport, &vault, &accounts, &id, &env)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "storage");
        fs::remove_dir(appdata.join("Roaming")).unwrap();

        // Junction at cache/tmp/game/<id>/ (previous AppData/Roaming failure created `temp_dir`)
        let temp_dir = w.root.game_tmp_dir().join(id.as_str());
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir).unwrap();
        }
        fs::create_dir_all(temp_dir.parent().unwrap()).unwrap();
        create_junction(&temp_dir, outside.path());
        let err = plan_launch(&w.root, &transport, &vault, &accounts, &id, &env)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "storage");
        fs::remove_dir(&temp_dir).unwrap();
    }

    #[test]
    fn finish_session_keeps_exit_when_idle_alive_while_second_profile_is_launching() {
        let d = tempfile::tempdir().unwrap();
        let state = LaunchState::new(PortableRoot::at(d.path()));
        let launching = InstanceId::new("launching-a").unwrap();
        let running = InstanceId::new("running-b").unwrap();

        assert!(state.claim(&launching));
        assert!(state.claim(&running));
        state.set_pid(&running, 2002);

        // User requests close while `running-b` is live: hide and defer quit.
        assert!(state.close_requested());
        assert!(state.exit_when_idle());
        assert!(!state.is_closing());

        // `running-b` exits while `launching-a` is still in Phase::Launching: must NOT quit yet.
        assert_eq!(state.finish_session(&running), (true, true));
        assert!(state.any_starting());

        // Once `launching-a` finishes or fails, no live or launching sessions remain.
        assert_eq!(state.finish_session(&launching), (true, false));
        assert!(!state.any_starting());
    }

    #[test]
    fn resolve_game_logs_dir_requires_existing_profile_and_rejects_links() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("Clade"));
        root.ensure_layout().unwrap();

        let missing = InstanceId::new("never-created").unwrap();
        let err = resolve_game_logs_dir(&root, &missing).unwrap_err();
        assert_eq!(err.code, "not_found");
        assert!(
            !logs_dir(&root, &missing).exists(),
            "missing profile must not create logs directory"
        );

        let inst = InstanceStore::new(&root)
            .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 1)
            .unwrap();

        #[cfg(windows)]
        {
            let outside = tempfile::tempdir().unwrap();
            let games_logs = root.logs_dir().join("games");
            fs::create_dir_all(&games_logs).unwrap();
            let linked = games_logs.join(inst.id.as_str());
            create_junction(&linked, outside.path());
            let err = resolve_game_logs_dir(&root, &inst.id).unwrap_err();
            assert_eq!(err.code, "storage");
            fs::remove_dir(&linked).unwrap();
        }

        let dir = resolve_game_logs_dir(&root, &inst.id).unwrap();
        assert_eq!(dir, logs_dir(&root, &inst.id));
        assert!(dir.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn prepare_reattach_sessions_banks_exited_and_keeps_live_handle_exit_code() {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("Clade"));
        root.ensure_layout().unwrap();
        let store = InstanceStore::new(&root);
        let dead_inst = store
            .create(NewInstance::vanilla("Dead", "1.21.4").unwrap(), 1)
            .unwrap()
            .id;
        let live_inst = store
            .create(NewInstance::vanilla("Live", "1.21.4").unwrap(), 2)
            .unwrap()
            .id;

        let playtime = PlaytimeStore::new(&root);
        let t0 = Instant::now();
        let mut dead_tracker = playtime.begin_session(&dead_inst, 1, 1_000, t0).unwrap();
        dead_tracker
            .tick(1_060, t0 + Duration::from_secs(60))
            .unwrap();
        drop(dead_tracker);

        // Spawn a real process that stays alive long enough to be captured with a live handle,
        // then exits with a non-zero exit code (42).
        let mut child = std::process::Command::new("cmd")
            .args(["/D", "/C", "ping -n 3 127.0.0.1 >nul & exit /b 42"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let live_tracker = playtime
            .begin_session(&live_inst, child.id(), 1_000, t0)
            .unwrap();
        drop(live_tracker);

        let mut sessions = prepare_reattach_sessions(&root).unwrap();
        // Dead session was banked immediately and removed from playtime-active.
        assert_eq!(
            playtime.instance_total(&dead_inst).unwrap().total_seconds,
            60
        );
        assert_eq!(playtime.lifetime_total().unwrap().total_seconds, 60);

        // Live session kept its verified OS handle across process termination and preserves exit code 42.
        assert_eq!(sessions.len(), 1);
        let (_jpath, sess, proc_handle) = sessions.pop().unwrap();
        assert_eq!(sess.instance_id, live_inst);
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(42));
        assert_eq!(proc_handle.wait_blocking(), Some(42));
    }
}
