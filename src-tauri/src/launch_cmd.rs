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

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::accounts::{AccountStatus, AccountsFile};
use crate::accounts_api::AccountsState;
use crate::auth::{ensure_fresh, AuthState};
use crate::coded_error::CodedError;
use crate::game_session::{logs_dir, supervise, ExitKind, ExitSummary, Watch, GRACE};
use crate::instances::{Instance, InstanceId, InstanceStore, JavaChoice, SetupState};
use crate::java_install::find_installed;
use crate::launch_args::{build, resolve, BuildInput, GameCommand, Identity, LaunchError};
use crate::loader_install::SystemEnv;
use crate::ms_auth::AuthError;
use crate::net::Transport;
use crate::playtime::PlaytimeStore;
use crate::portable::PortableRoot;
use crate::prepare::check_installed;
use crate::profiles::now_unix;
use crate::runtimes::{resolve_java, ManagedJava};
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

/// The running games, managed by Tauri.
pub struct LaunchState {
    pub root: PortableRoot,
    sessions: Mutex<HashMap<String, Entry>>,
    exit_when_idle: AtomicBool,
}

impl LaunchState {
    pub fn new(root: PortableRoot) -> Self {
        Self {
            root,
            sessions: Mutex::new(HashMap::new()),
            exit_when_idle: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reserves the instance so a second Play click cannot start a second game. `false` if taken.
    pub fn claim(&self, id: &InstanceId) -> bool {
        let mut s = self.lock();
        if s.contains_key(id.as_str()) {
            return false;
        }
        s.insert(
            id.to_string(),
            Entry {
                phase: Phase::Launching,
                pid: None,
            },
        );
        true
    }

    fn set_pid(&self, id: &InstanceId, pid: u32) {
        if let Some(e) = self.lock().get_mut(id.as_str()) {
            e.pid = Some(pid);
        }
    }

    fn mark_playing(&self, id: &InstanceId) {
        if let Some(e) = self.lock().get_mut(id.as_str()) {
            e.phase = Phase::Playing;
        }
    }

    pub fn release(&self, id: &InstanceId) {
        self.lock().remove(id.as_str());
    }

    /// Is a game process alive (started and not yet exited)? Closing the window while this is
    /// true must not end Clade.
    pub fn has_live_game(&self) -> bool {
        self.lock().values().any(|e| e.pid.is_some())
    }

    pub fn set_exit_when_idle(&self, on: bool) {
        self.exit_when_idle.store(on, Ordering::SeqCst);
    }

    pub fn exit_when_idle(&self) -> bool {
        self.exit_when_idle.load(Ordering::SeqCst)
    }

    fn snapshot(&self) -> Vec<(String, &'static str)> {
        self.lock()
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
    let (now, sys, total_memory_mb) = (env.now, env.sys, env.total_memory_mb);
    let store = InstanceStore::new(root);
    let inst = store
        .get(id)
        .map_err(|_| fail("not_found", "That profile no longer exists."))?;
    let SetupState::Ready { version_id, .. } = &inst.setup else {
        return Err(fail(
            "not_ready",
            "This profile has to be set up before it can start.",
        ));
    };

    // The selected account (global convenience state; the instance is never touched for it).
    let file = AccountsFile::load(accounts.path()).map_err(|e| {
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
    check_installed(root, &inst).map_err(|what| needs_repair(&what))?;

    let resolved = resolve(root, version_id.as_str()).map_err(|e| match e {
        LaunchError::MissingVersion(_) | LaunchError::MissingFile(_) => {
            needs_repair("the version information")
        }
        other => fail(
            "unsupported",
            format!("This profile can't be started: {other}."),
        ),
    })?;
    let java = java_for(root, &inst, &resolved.java.component)?;

    // Credentials, through the same hook preparation of tokens uses everywhere.
    let token = ensure_fresh(transport, vault, accounts, &meta.id, now)
        .await
        .map_err(|e| from_auth(&e, &meta.id))?;

    let dirs = store.dirs(id);
    let temp = root.cache_dir().join("game-tmp").join(id.as_str());
    for d in [
        dirs.game_dir(),
        temp.clone(),
        dirs.home_dir().join("AppData").join("Roaming"),
        dirs.home_dir().join("AppData").join("Local"),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| {
            fail(
                "storage",
                format!("A folder for the game could not be created: {e}"),
            )
        })?;
    }
    let command = build(&BuildInput {
        root,
        dirs: &dirs,
        resolved: &resolved,
        java: &java,
        identity: &Identity {
            uuid: meta.minecraft_uuid.clone(),
            name: meta.username.clone(),
            access_token: token,
        },
        memory_mb: crate::system::memory_for(inst.memory_mb, total_memory_mb),
        extra_jvm_args: &inst.extra_jvm_args,
        sys,
        temp_dir: &temp,
    })
    .map_err(|e| match e {
        LaunchError::MissingFile(what) => needs_repair(&what),
        other => fail(
            "unsupported",
            format!("This profile can't be started: {other}."),
        ),
    })?;
    Ok(Planned {
        instance: inst,
        command,
        account_id: meta.id,
    })
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
    if !state.claim(&iid) {
        return Err(fail(
            "already_running",
            "This profile is already starting or running.",
        ));
    }
    emit(&app, &iid, "launching", None);
    let result = start(&app, &state, &auth, &accounts, &iid).await;
    if let Err(e) = &result {
        tracing::info!(code = e.code, "launch did not start");
        state.release(&iid);
        emit(&app, &iid, "stopped", None);
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
        },
    )
    .await?;

    // Direct process creation (no shell). The child is not tied to this process: if Clade ends,
    // the game does not.
    let mut process = tokio::process::Command::from(planned.command.to_process());
    process
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = process
        .spawn()
        .map_err(|e| fail("spawn_failed", format!("Java could not be started: {e}")))?;
    let pid = child.id().unwrap_or(0);
    state.set_pid(id, pid);
    tracing::info!(pid, "game process started");

    // The playtime session starts only now, after a successful spawn.
    let tracker =
        match PlaytimeStore::new(&state.root).begin_session(id, pid, now_unix(), Instant::now()) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = %e, "could not start the playtime session");
                None
            }
        };
    let _ = accounts.update(|f| {
        if let Some(a) = f.accounts.iter_mut().find(|a| a.id == planned.account_id) {
            a.last_used_at = Some(now_unix());
        }
    });

    let header = planned.command.redacted(&state.root);
    let secrets = planned.command.secrets().to_vec();
    let (app, id, root) = (app.clone(), id.clone(), state.root.clone());
    tokio::spawn(async move {
        let playing_app = app.clone();
        let playing_id = id.clone();
        let outcome = supervise(
            child,
            Watch {
                root: &root,
                instance: &id,
                secrets: &secrets,
                header: &header,
                tracker,
                heartbeat: HEARTBEAT,
                grace: GRACE,
                now_unix,
            },
            move || {
                playing_app.state::<LaunchState>().mark_playing(&playing_id);
                emit(&playing_app, &playing_id, "playing", None);
            },
        )
        .await;
        let _ = std::fs::remove_dir_all(root.cache_dir().join("game-tmp").join(id.as_str()));
        let ls = app.state::<LaunchState>();
        ls.release(&id);
        tracing::info!(code = ?outcome.exit.code, kind = ?outcome.exit.kind, "game process ended");
        emit(&app, &id, "stopped", Some((&outcome.exit).into()));
        // Clade was closed while the game ran: now that it is over, finish quitting.
        if ls.exit_when_idle() && !ls.has_live_game() {
            app.exit(0);
        }
    });
    Ok(())
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

/// Opens the folder that holds this profile's game session logs.
#[tauri::command]
pub fn open_game_logs(
    app: AppHandle,
    state: State<'_, LaunchState>,
    id: String,
) -> Result<(), CodedError> {
    let iid = InstanceId::new(&id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let dir = logs_dir(&state.root, &iid);
    std::fs::create_dir_all(&dir).map_err(CodedError::storage)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| CodedError::new("open_failed", e.to_string()))
}

#[cfg(test)]
mod tests;
