//! "Start fresh": the way out when the vault passphrase is forgotten (or the vault file is missing or
//! damaged), without touching anything but the saved sign-ins.
//!
//! What it does, in this order, and nothing else:
//! 1. Checks that nothing is using the credentials: the vault is not unlocked, no sign-in or skin
//!    change is running, no game is starting. The checks use the gates that already exist, in the
//!    existing lock order (vault, then the accounts file).
//! 2. Reads `accounts.json`. A damaged file stops everything here, before any file is moved.
//! 3. **Retires** the old credential files by renaming them into
//!    `data/retired-vaults/<unix-time>-<random>/`: `vault.bin.bak`, any leftover
//!    `vault.bin.<12 hex digits>.tmp` (the exact name of an interrupted save), and `vault.bin` last. Nothing is deleted, and the retired vault still
//!    opens with its old passphrase. Files that do not exist are skipped, and no folder is created
//!    when there is nothing to retire.
//! 4. Marks every account "needs to sign in again" (`NeedsReauth(Unknown)`) in `accounts.json`, with
//!    the same safe write the rest of the launcher uses. Names, UUIDs, ids and the active account stay.
//!
//! It is not one atomic step, because several files are involved, so it is built to be repeated:
//! every stage skips what is already done. If stage 3 fails, the active vault is still in place and
//! usable (the files already moved are renamed back, never deleted). If stage 4 fails, the vault is
//! already retired and the error says so (`accounts_not_updated`); running it again only repeats stage
//! 4. A new vault is not created here: that happens at the next sign-in, with a new passphrase.
//!
//! The Microsoft sessions behind the old tokens are not revoked remotely. Profiles, worlds, playtime,
//! settings, the Minecraft files and the skin cache are never touched. This module never deletes a
//! file or a folder tree: the only removal is of the empty retirement folder it just created, when
//! the first rename failed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::accounts::ReauthReason;
use crate::accounts_api::{AccountsDto, AccountsState, ACCOUNTS_CHANGED_EVENT};
use crate::auth::{lock, AuthState, RunGuard};
use crate::coded_error::CodedError;
use crate::launch_cmd::LaunchState;
use crate::portable::fsutil::{
    is_link, move_new_no_replace, reject_links_between, rename_with_retry,
};
use crate::profiles::now_unix;

/// Where retired vaults go, next to `vault.bin`.
pub const RETIRED_DIR: &str = "retired-vaults";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResetError {
    #[error("The vault is unlocked, so there is nothing to start fresh from.")]
    VaultUnlocked,
    #[error("A sign-in is in progress. Finish or cancel it first.")]
    SignInInProgress,
    #[error("A skin or cape change is still running.")]
    AppearanceBusy,
    #[error("A game is starting.")]
    GameStarting,
    /// The old credential files could not be archived. The active vault was left in place.
    #[error("The old sign-in store could not be archived; nothing was changed.")]
    ResetFailed,
    /// The vault is archived, but the account list could not be updated. Run it again.
    #[error("The old sign-in store was archived, but the account list could not be updated.")]
    AccountsNotUpdated,
    /// `accounts.json` could not be read; nothing was changed.
    #[error("The account list could not be read; nothing was changed.")]
    Storage,
}

impl ResetError {
    pub fn code(self) -> &'static str {
        match self {
            Self::VaultUnlocked => "vault_unlocked",
            Self::SignInInProgress => "sign_in_in_progress",
            Self::AppearanceBusy => "appearance_busy",
            Self::GameStarting => "game_starting",
            Self::ResetFailed => "reset_failed",
            Self::AccountsNotUpdated => "accounts_not_updated",
            Self::Storage => "storage",
        }
    }

    pub fn coded(self) -> CodedError {
        CodedError::new(self.code(), self.to_string())
    }
}

/// Is `name` the temporary file of an interrupted vault save? `atomic_write` names it
/// `<file>.<12 lowercase hex digits>.tmp`; nothing broader is accepted, so an unrelated file that merely
/// starts with `vault.bin.` is never moved.
fn is_save_temp(name: &str, vault_name: &str) -> bool {
    name.strip_prefix(vault_name)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|hex| {
            hex.len() == 12
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// The credential files that exist now, in the order they are retired: the backup and any leftover
/// temporary file first, the vault itself last, so a failure before the last rename leaves the
/// active vault where it was.
fn credential_files(vault: &Path) -> Result<Vec<PathBuf>, ResetError> {
    let dir = vault.parent().ok_or(ResetError::ResetFailed)?;
    if is_link(dir).unwrap_or(true) {
        return Err(ResetError::ResetFailed);
    }
    let name = vault
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(ResetError::ResetFailed)?;
    let mut files = Vec::new();
    let mut take = |p: PathBuf| -> Result<(), ResetError> {
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() && !is_link(&p).unwrap_or(true) => {
                files.push(p);
                Ok(())
            }
            // something else sits where a credential file belongs: do not touch it
            Ok(_) => Err(ResetError::ResetFailed),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ResetError::ResetFailed),
        }
    };
    take(dir.join(format!("{name}.bak")))?;
    // leftovers of an interrupted save (`vault.bin.<hex>.tmp`) hold encrypted credentials too
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let n = e.file_name();
            if is_save_temp(&n.to_string_lossy(), name) {
                take(e.path())?;
            }
        }
    }
    take(vault.to_path_buf())?;
    Ok(files)
}

fn unique_dir(root: &Path, now: i64) -> Result<PathBuf, ResetError> {
    let base = root.parent().ok_or(ResetError::ResetFailed)?;
    reject_links_between(base, root).map_err(|_| ResetError::ResetFailed)?;
    fs::create_dir_all(root).map_err(|_| ResetError::ResetFailed)?;
    for _ in 0..8 {
        let mut nonce = [0u8; 6];
        getrandom::fill(&mut nonce).map_err(|_| ResetError::ResetFailed)?;
        let dir = root.join(format!("{now}-{}", hex::encode(nonce)));
        match fs::create_dir(&dir) {
            Ok(()) => {
                reject_links_between(base, &dir).map_err(|_| ResetError::ResetFailed)?;
                return Ok(dir);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(ResetError::ResetFailed),
        }
    }
    Err(ResetError::ResetFailed)
}

/// Moves `files` into one new folder under `root`. Does nothing, and creates nothing, for an empty
/// list. If a rename fails, the files already moved are renamed back (never deleted).
fn retire(files: &[PathBuf], root: &Path, now: i64) -> Result<(), ResetError> {
    retire_with_ops(files, root, now, &move_new_no_replace, &rename_with_retry)
}

/// The rename used to move a file. A parameter so recovery uses non-replacing moves into the
/// archive and retrying moves on rollback, while tests can inject failures.
type Rename<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

fn retire_with_ops(
    files: &[PathBuf],
    root: &Path,
    now: i64,
    move_into: Rename<'_>,
    move_back: Rename<'_>,
) -> Result<(), ResetError> {
    if files.is_empty() {
        return Ok(());
    }
    let dir = unique_dir(root, now)?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for src in files {
        let Some(name) = src.file_name() else {
            return rollback(&moved, &dir, move_back);
        };
        let dst = dir.join(name);
        match move_into(src, &dst) {
            Ok(()) => moved.push((src.clone(), dst)),
            Err(_) => return rollback(&moved, &dir, move_back),
        }
    }
    Ok(())
}

fn rollback(
    moved: &[(PathBuf, PathBuf)],
    dir: &Path,
    rename: Rename<'_>,
) -> Result<(), ResetError> {
    for (src, dst) in moved.iter().rev() {
        let _ = rename(dst, src); // a failure leaves the file safely in the retirement folder
    }
    let _ = fs::remove_dir(dir); // only succeeds when it is empty
    Err(ResetError::ResetFailed)
}

/// The whole recovery. See the module documentation.
pub fn reset(
    auth: &AuthState,
    accounts: &AccountsState,
    game_starting: &dyn Fn() -> bool,
    now: i64,
) -> Result<(), ResetError> {
    // A sign-in cannot start while this runs, and none is running now.
    {
        let mut running = lock(&auth.running);
        if running.is_some() {
            return Err(ResetError::SignInInProgress);
        }
        *running = Some(Arc::new(Notify::new()));
    }
    let _sign_in = RunGuard(&auth.running);
    // Nor can a skin or cape change (they read the vault through the same refresh path).
    let _change = accounts
        .begin_appearance_change()
        .ok_or(ResetError::AppearanceBusy)?;
    // A game that is starting is fetching its token right now. One that is already running is not
    // using the credentials any more, and a profile being prepared never does.
    if game_starting() {
        return Err(ResetError::GameStarting);
    }
    // The vault lock is held to the end: no refresh, sign-in write or account removal can touch the
    // vault meanwhile. The accounts gate is only taken after it (the launcher's one lock order).
    let vault = lock(&auth.vault);
    if vault.is_some() {
        return Err(ResetError::VaultUnlocked);
    }

    // Stage 2: the account list must be readable before anything is moved.
    let list = accounts
        .load_authoritative()
        .map_err(|_| ResetError::Storage)?;

    // Stage 3: retire the old credentials.
    let files = credential_files(&auth.vault_path)?;
    // An unlock or create that is still in flight must not install its vault after this point.
    auth.epoch.fetch_add(1, Ordering::SeqCst);
    let root = auth
        .vault_path
        .parent()
        .ok_or(ResetError::ResetFailed)?
        .join(RETIRED_DIR);
    retire(&files, &root, now)?;

    // Stage 4: every account must sign in again.
    if list.is_some_and(|f| !f.accounts.is_empty()) {
        accounts
            .update(|f| {
                let ids: Vec<String> = f.accounts.iter().map(|a| a.id.clone()).collect();
                for id in ids {
                    f.mark_needs_reauth(&id, ReauthReason::Unknown, now);
                }
            })
            .map_err(|_| ResetError::AccountsNotUpdated)?;
    }
    Ok(())
}

fn should_emit_accounts_changed(result: &Result<(), ResetError>) -> bool {
    matches!(result, Ok(()) | Err(ResetError::AccountsNotUpdated))
}

/// "Start fresh": archives the old vault and makes every account sign in again. Only allowed while
/// the vault is locked, missing or unreadable.
#[tauri::command]
pub async fn reset_vault(
    app: AppHandle,
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    launch: State<'_, LaunchState>,
) -> Result<AccountsDto, CodedError> {
    let _ = (&auth, &accounts, &launch);
    let app_for_work = app.clone();
    let (result, dto) = tokio::task::spawn_blocking(move || {
        let auth = app_for_work.state::<AuthState>();
        let accounts = app_for_work.state::<AccountsState>();
        let launch = app_for_work.state::<LaunchState>();
        let result = reset(&auth, &accounts, &|| launch.any_starting(), now_unix());
        let dto = if result.is_ok() {
            Some(accounts.list().map_err(|e| e.coded()))
        } else {
            None
        };
        (result, dto)
    })
    .await
    .map_err(|_| ResetError::ResetFailed.coded())?;

    match &result {
        Ok(()) => tracing::info!(op = "vault_reset", result = "ok", "recovery"),
        Err(e) => tracing::info!(op = "vault_reset", result = e.code(), "recovery"),
    }
    // Only emit when state actually changed (including when the vault was archived before list save).
    if should_emit_accounts_changed(&result) {
        let _ = app.emit(ACCOUNTS_CHANGED_EVENT, ());
    }
    result.map_err(ResetError::coded)?;
    dto.unwrap_or_else(|| Err(ResetError::Storage.coded()))
}

#[cfg(test)]
fn retire_with(
    files: &[PathBuf],
    root: &Path,
    now: i64,
    rename: Rename<'_>,
) -> Result<(), ResetError> {
    retire_with_ops(files, root, now, rename, rename)
}

#[cfg(test)]
mod tests;
