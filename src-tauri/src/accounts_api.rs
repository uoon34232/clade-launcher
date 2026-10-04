//! The account list as the frontend sees it, and the commands that change which account is active.
//!
//! Accounts are independent from instances: nothing here reads or writes an instance, its files
//! or its playtime. The active account is one global convenience value (`accounts.json`).
//! Metadata only; tokens live in the encrypted vault and never reach this module.
//!
//! Signing in lives in `auth.rs`; skin and cape changes are not implemented yet, and the
//! capabilities say so.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::State;

use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
use crate::auth::AuthState;
use crate::coded_error::CodedError;
use crate::ms_auth::AuthError;

/// Serialises read-modify-write of `accounts.json`.
pub struct AccountsState {
    path: PathBuf,
    gate: Mutex<()>,
}

impl AccountsState {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            gate: Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read-modify-write of `accounts.json` under the lock (metadata only, atomic save).
    pub fn update<R>(&self, f: impl FnOnce(&mut AccountsFile) -> R) -> Result<R, AuthError> {
        let _g = self.lock();
        let mut file =
            AccountsFile::load(&self.path).map_err(|e| AuthError::Storage(e.to_string()))?;
        let out = f(&mut file);
        file.save(&self.path)
            .map_err(|e| AuthError::Storage(e.to_string()))?;
        Ok(out)
    }
}

/// What this build can do, so the UI never offers what would fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub sign_in: bool,
    pub skin: bool,
    pub cape: bool,
}

pub const CAPABILITIES: Capabilities = Capabilities {
    sign_in: true,
    skin: false,
    cape: false,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: String,
    /// The Minecraft Java profile name.
    pub username: String,
    pub uuid: String,
    pub needs_sign_in: bool,
    /// A data URI of the player's head, rendered and cached by the backend once skins are
    /// fetched. `None` until then (the UI draws a neutral placeholder).
    pub head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsDto {
    pub accounts: Vec<AccountDto>,
    pub active: Option<String>,
    pub capabilities: Capabilities,
}

fn dto(a: &AccountMeta) -> AccountDto {
    AccountDto {
        id: a.id.clone(),
        username: a.username.clone(),
        uuid: a.minecraft_uuid.clone(),
        needs_sign_in: matches!(a.status, AccountStatus::NeedsReauth { .. }),
        head: None,
    }
}

fn snapshot(file: &AccountsFile) -> AccountsDto {
    AccountsDto {
        accounts: file.accounts.iter().map(dto).collect(),
        active: file.effective_active().map(str::to_owned),
        capabilities: CAPABILITIES,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("That account no longer exists.")]
    Unknown,
    #[error("The account list could not be read: {0}")]
    Storage(String),
    #[error("Microsoft sign-in isn't available in this build yet.")]
    SignInUnavailable,
}

impl AccountError {
    pub fn coded(&self) -> CodedError {
        let message = self.to_string();
        match self {
            Self::Unknown => CodedError::new("unknown_account", message),
            Self::Storage(_) => CodedError::new("storage", message),
            Self::SignInUnavailable => CodedError::new("sign_in_unavailable", message),
        }
    }
}

fn load(path: &Path) -> Result<AccountsFile, AccountError> {
    AccountsFile::load(path).map_err(|e| AccountError::Storage(e.to_string()))
}

fn save(file: &AccountsFile, path: &Path) -> Result<(), AccountError> {
    file.save(path)
        .map_err(|e| AccountError::Storage(e.to_string()))
}

pub fn list(path: &Path) -> Result<AccountsDto, AccountError> {
    Ok(snapshot(&load(path)?))
}

pub fn select(path: &Path, id: &str) -> Result<AccountsDto, AccountError> {
    let mut f = load(path)?;
    if !f.select(id) {
        return Err(AccountError::Unknown);
    }
    save(&f, path)?;
    Ok(snapshot(&f))
}

/// Removes the account's metadata. The command also deletes the account's vault secrets
/// (sign out = remove tokens).
pub fn remove(path: &Path, id: &str) -> Result<AccountsDto, AccountError> {
    let mut f = load(path)?;
    if !f.remove(id) {
        return Err(AccountError::Unknown);
    }
    save(&f, path)?;
    Ok(snapshot(&f))
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AccountsState>) -> Result<AccountsDto, CodedError> {
    let _g = state.lock();
    list(&state.path).map_err(|e| e.coded())
}

#[tauri::command]
pub fn select_account(
    state: State<'_, AccountsState>,
    id: String,
) -> Result<AccountsDto, CodedError> {
    let _g = state.lock();
    select(&state.path, &id).map_err(|e| e.coded())
}

#[tauri::command]
pub fn remove_account(
    state: State<'_, AccountsState>,
    auth: State<'_, AuthState>,
    id: String,
) -> Result<AccountsDto, CodedError> {
    let _g = state.lock();
    let dto = remove(&state.path, &id).map_err(|e| e.coded())?;
    auth.forget(&id); // sign out: its secrets leave the vault too (when it is unlocked)
    Ok(dto)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, name: &str) -> AccountMeta {
        AccountMeta {
            id: id.into(),
            username: name.into(),
            minecraft_uuid: "00000000-0000-4000-8000-00000000a001".into(),
            added_at: 1,
            last_used_at: None,
            status: AccountStatus::Ready,
        }
    }

    fn seeded() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("accounts.json");
        let mut f = AccountsFile::default();
        f.accounts.push(meta("a", "Alex"));
        f.accounts.push(meta("b", "Blake"));
        f.save(&p).unwrap();
        (d, p)
    }

    #[test]
    fn no_file_means_no_accounts_and_nothing_is_invented() {
        let d = tempfile::tempdir().unwrap();
        let s = list(&d.path().join("accounts.json")).unwrap();
        assert!(s.accounts.is_empty() && s.active.is_none());
    }

    #[test]
    fn selection_is_saved_and_survives_a_reload() {
        let (_d, p) = seeded();
        assert_eq!(list(&p).unwrap().active.as_deref(), Some("a"));
        assert_eq!(select(&p, "b").unwrap().active.as_deref(), Some("b"));
        assert_eq!(list(&p).unwrap().active.as_deref(), Some("b"));
        assert!(matches!(select(&p, "nope"), Err(AccountError::Unknown)));
    }

    #[test]
    fn selecting_or_removing_an_account_touches_no_instance() {
        let (d, p) = seeded();
        let inst = d.path().join("instances/x");
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::write(inst.join("instance.json"), "{}").unwrap();
        select(&p, "b").unwrap();
        remove(&p, "a").unwrap();
        assert_eq!(std::fs::read(inst.join("instance.json")).unwrap(), b"{}");
    }

    #[test]
    fn removing_the_active_account_moves_to_another() {
        let (_d, p) = seeded();
        select(&p, "b").unwrap();
        let s = remove(&p, "b").unwrap();
        assert_eq!(s.active.as_deref(), Some("a"));
        assert_eq!(remove(&p, "a").unwrap().active, None);
    }

    #[test]
    fn the_wire_format_has_no_tokens_and_reports_current_capabilities() {
        let (_d, p) = seeded();
        let json = serde_json::to_string(&list(&p).unwrap()).unwrap();
        assert!(json.contains("\"needsSignIn\":false"));
        assert!(json.contains("\"capabilities\":{\"signIn\":true,\"skin\":false,\"cape\":false}"));
        assert!(!json.to_lowercase().contains("token"));
    }
}
