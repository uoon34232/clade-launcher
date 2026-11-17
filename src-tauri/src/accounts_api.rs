//! The account list as the frontend sees it, and the commands that change which account is active.
//!
//! Accounts are independent from instances: nothing here reads or writes an instance, its files
//! or its playtime. The active account is one global convenience value (`accounts.json`).
//! Metadata only; tokens live in the encrypted vault and never reach this module.
//!
//! Signing in lives in `auth.rs`; skin and cape changes live in `appearance_cmd.rs` on top of
//! `profile_service.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
use crate::auth::AuthState;
use crate::coded_error::CodedError;
use crate::ms_auth::AuthError;
use crate::net::HttpFetcher;
use crate::skin::SkinCache;

/// Sent when avatars were fetched in the background: the interface lists the accounts again.
pub const ACCOUNTS_CHANGED_EVENT: &str = "accounts-changed";
/// Overall time one background round of skin downloads may take, however many accounts there are.
const BACKGROUND_SKIN_BUDGET: Duration = Duration::from_secs(10);

/// Serialises read-modify-write of `accounts.json`.
pub struct AccountsState {
    path: PathBuf,
    gate: Mutex<()>,
    /// The avatar cache and the client that fills it; absent in tests that do not need skins.
    skins: Option<(SkinCache, Result<HttpFetcher, String>)>,
    /// A background round of skin downloads is running.
    syncing: AtomicBool,
    /// A skin or cape change is being sent. Changes are sent one at a time and never repeated.
    appearance_busy: AtomicBool,
}

impl AccountsState {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            gate: Mutex::new(()),
            skins: None,
            syncing: AtomicBool::new(false),
            appearance_busy: AtomicBool::new(false),
        }
    }

    /// Claims the right to send one skin or cape change; `None` while another one is running.
    pub fn begin_appearance_change(&self) -> Option<AppearanceChange<'_>> {
        (!self.appearance_busy.swap(true, Ordering::AcqRel)).then_some(AppearanceChange(self))
    }

    /// Enables account avatars: skins are cached in `cache` and fetched with `http` (the global
    /// allow-list; the texture host is a Mojang host).
    pub fn with_skins(mut self, cache: SkinCache, http: Result<HttpFetcher, String>) -> Self {
        self.skins = Some((cache, http));
        self
    }

    /// Notes the skin an account's profile just reported (`None`: the default skin). Local only.
    pub fn record_skin(&self, uuid: &str, reported: Option<&str>) {
        if let Some((cache, _)) = &self.skins {
            cache.record(uuid, reported);
        }
    }

    fn known_uuids(&self) -> Vec<String> {
        let _g = self.lock();
        AccountsFile::load(&self.path)
            .map(|f| f.accounts.into_iter().map(|a| a.minecraft_uuid).collect())
            .unwrap_or_default()
    }

    /// Downloads the skins that are noted but not cached yet, all together and within one overall
    /// `budget` (never a wait per account). Returns whether anything was waiting. Cosmetic: it
    /// cannot fail.
    pub async fn sync_pending_skins(&self, budget: Duration) -> bool {
        let Some((cache, Ok(http))) = &self.skins else {
            return false;
        };
        cache.sync_all(http, &self.known_uuids(), budget).await
    }

    /// Whether some account has a skin waiting to be downloaded (local files only).
    fn skins_pending(&self) -> bool {
        match &self.skins {
            Some((cache, Ok(_))) => self.known_uuids().iter().any(|u| cache.pending(u)),
            _ => false,
        }
    }

    /// Starts fetching the waiting skins in the background, at most one such round at a time, and
    /// tells the interface (`accounts-changed`) when it is done so it can list again. The account
    /// list itself never waits for the network: avatars are decorative.
    pub fn sync_skins_in_background(&self, app: &AppHandle) {
        if !self.skins_pending() || self.syncing.swap(true, Ordering::AcqRel) {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AccountsState>();
            let ran = state.sync_pending_skins(BACKGROUND_SKIN_BUDGET).await;
            state.syncing.store(false, Ordering::Release);
            if ran {
                let _ = app.emit(ACCOUNTS_CHANGED_EVENT, ());
            }
        });
    }

    /// Fills in each account's avatar from the cache (no network).
    pub fn attach(&self, mut list: AccountsDto) -> AccountsDto {
        if let Some((cache, _)) = &self.skins {
            for a in &mut list.accounts {
                a.skin = cache.data_uri(&a.uuid);
            }
        }
        list
    }

    /// The accounts as the frontend sees them, avatars included.
    pub fn list(&self) -> Result<AccountsDto, AccountError> {
        let _g = self.lock();
        Ok(self.attach(list(&self.path)?))
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> std::io::Result<AccountsFile> {
        let _g = self.lock();
        AccountsFile::load(&self.path)
    }

    pub fn load_authoritative(&self) -> std::io::Result<Option<AccountsFile>> {
        let _g = self.lock();
        AccountsFile::load_authoritative(&self.path)
    }

    /// Whether nobody holds the gate right now (tests use it to prove the vault is never touched
    /// while the gate is held).
    #[cfg(test)]
    fn is_free(&self) -> bool {
        self.gate.try_lock().is_ok()
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

/// Held while a skin or cape change is in flight; dropping it frees the next one.
pub struct AppearanceChange<'a>(&'a AccountsState);

impl Drop for AppearanceChange<'_> {
    fn drop(&mut self) {
        self.0.appearance_busy.store(false, Ordering::Release);
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

/// Skin and cape changes are real: `appearance_cmd.rs` talks to Minecraft services through
/// `profile_service::MinecraftServices`.
pub const CAPABILITIES: Capabilities = Capabilities {
    sign_in: true,
    skin: true,
    cape: true,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: String,
    /// The Minecraft Java profile name.
    pub username: String,
    pub uuid: String,
    pub needs_sign_in: bool,
    /// The account's current skin texture as an inline PNG (`data:image/png;base64,...`), cached by
    /// the backend from the account's own profile. The UI cuts the head and the hat layer out of
    /// it. `None` when there is none (the UI draws a neutral placeholder).
    pub skin: Option<String>,
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
        skin: None,
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
pub fn list_accounts(
    app: AppHandle,
    state: State<'_, AccountsState>,
) -> Result<AccountsDto, CodedError> {
    // Answers at once from local files; waiting avatars are fetched in the background.
    state.sync_skins_in_background(&app);
    state.list().map_err(|e| e.coded())
}

#[tauri::command]
pub fn select_account(
    state: State<'_, AccountsState>,
    id: String,
) -> Result<AccountsDto, CodedError> {
    let _g = state.lock();
    select(&state.path, &id)
        .map(|d| state.attach(d))
        .map_err(|e| e.coded())
}

#[tauri::command]
pub fn remove_account(
    state: State<'_, AccountsState>,
    auth: State<'_, AuthState>,
    id: String,
) -> Result<AccountsDto, CodedError> {
    remove_and_forget(&state, &id, |id| auth.forget(id))
        .map(|d| state.attach(d))
        .map_err(|e| e.coded())
}

/// Removes the account's metadata, then its vault secrets, and never holds the accounts gate while
/// touching the vault. Sign-in persistence takes the vault first and the gate second, so the other
/// order here would let the two deadlock. One order only: vault, then gate.
pub fn remove_and_forget(
    state: &AccountsState,
    id: &str,
    forget: impl FnOnce(&str),
) -> Result<AccountsDto, AccountError> {
    let dto = {
        let _g = state.lock();
        let uuid = load(&state.path).ok().and_then(|f| {
            f.accounts
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.minecraft_uuid.clone())
        });
        let dto = remove(&state.path, id)?;
        if let (Some(uuid), Some((cache, _))) = (uuid, &state.skins) {
            cache.forget(&uuid); // the avatar of a removed account goes with it
        }
        dto
    };
    forget(id); // sign out: its secrets leave the vault too (when it is unlocked)
    Ok(dto)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, name: &str) -> AccountMeta {
        AccountMeta {
            id: id.into(),
            username: name.into(),
            minecraft_uuid: format!(
                "00000000-0000-4000-8000-00000000{}",
                if id == "a" { "a001" } else { "b002" }
            ),
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
    fn the_vault_is_never_touched_while_the_accounts_gate_is_held() {
        let (_d, p) = seeded();
        let state = AccountsState::new(p);
        let mut forgotten = None;
        let dto = remove_and_forget(&state, "a", |id| {
            // the vault step runs here: the gate must already be free, so the order stays vault, then gate
            assert!(
                state.is_free(),
                "the gate was still held during the vault step"
            );
            forgotten = Some(id.to_owned());
        })
        .unwrap();
        assert_eq!(forgotten.as_deref(), Some("a"));
        assert_eq!(dto.accounts.len(), 1);
        // an unknown account never reaches the vault step
        let mut called = false;
        assert!(remove_and_forget(&state, "nope", |_| called = true).is_err());
        assert!(!called);
    }

    #[test]
    fn each_account_gets_its_own_avatar_and_removing_one_clears_only_its_cache() {
        use crate::net::HttpFetcher;
        let d = tempfile::tempdir().unwrap();
        let (_d2, p) = seeded();
        let cache = SkinCache::new(d.path().join("cache").join("skins"));
        let no_net: Result<HttpFetcher, String> = Err("tests do not use the network".into());
        let state = AccountsState::new(p).with_skins(cache.clone(), no_net);
        let uuids: Vec<String> = state
            .list()
            .unwrap()
            .accounts
            .iter()
            .map(|a| a.uuid.clone())
            .collect();
        assert!(state
            .list()
            .unwrap()
            .accounts
            .iter()
            .all(|a| a.skin.is_none()));

        // Seed the cache the way a finished sync leaves it: a different texture per account.
        for (i, uuid) in uuids.iter().enumerate() {
            let id = format!("{:0>64}", i + 1);
            state.record_skin(
                uuid,
                Some(&format!("https://textures.minecraft.net/texture/{id}")),
            );
            let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
            png.extend_from_slice(b"IHDR");
            png.extend_from_slice(&64u32.to_be_bytes());
            png.extend_from_slice(&64u32.to_be_bytes());
            png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0, i as u8]);
            let stem = crate::ms_auth::dashed_uuid(uuid).unwrap();
            let dir = d.path().join("cache").join("skins");
            std::fs::write(dir.join(format!("{stem}.png")), &png).unwrap();
            std::fs::write(dir.join(format!("{stem}.have")), &id).unwrap();
        }
        let skins: Vec<Option<String>> = state
            .list()
            .unwrap()
            .accounts
            .into_iter()
            .map(|a| a.skin)
            .collect();
        assert!(skins.iter().all(|s| s
            .as_deref()
            .is_some_and(|s| s.starts_with("data:image/png;base64,"))));
        assert_ne!(skins[0], skins[1], "every account shows its own skin");
        // Selecting another account never mixes avatars up.
        let dto = state.attach(select(state.path(), "b").unwrap());
        assert_eq!(dto.accounts[0].skin, skins[0]);
        assert_eq!(dto.accounts[1].skin, skins[1]);

        remove_and_forget(&state, "a", |_| ()).unwrap();
        assert!(cache.data_uri(&uuids[0]).is_none());
        assert!(
            cache.data_uri(&uuids[1]).is_some(),
            "the other account keeps its avatar"
        );
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
        assert!(json.contains("\"capabilities\":{\"signIn\":true,\"skin\":true,\"cape\":true}"));
        assert!(!json.to_lowercase().contains("token"));
    }

    #[test]
    fn state_load_and_load_authoritative_serialize_under_gate_and_enforce_limits() {
        use std::sync::{Arc, Barrier};

        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("accounts.json");
        let state = Arc::new(AccountsState::new(p.clone()));

        // Missing file behavior.
        assert!(state.load().unwrap().accounts.is_empty());
        assert!(state.load_authoritative().unwrap().is_none());

        // Seeded file behavior and serialization under `gate`.
        let mut initial = AccountsFile::default();
        initial.accounts.push(meta("a", "Alex"));
        initial.save(&p).unwrap();

        let guard = state.lock();
        let done_load = Arc::new(AtomicBool::new(false));
        let done_auth = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(Barrier::new(3));

        let t1 = {
            let state = Arc::clone(&state);
            let done = Arc::clone(&done_load);
            let ready = Arc::clone(&ready);
            std::thread::spawn(move || {
                ready.wait();
                let res = state.load().unwrap();
                done.store(true, Ordering::Release);
                res
            })
        };
        let t2 = {
            let state = Arc::clone(&state);
            let done = Arc::clone(&done_auth);
            let ready = Arc::clone(&ready);
            std::thread::spawn(move || {
                ready.wait();
                let res = state.load_authoritative().unwrap().unwrap();
                done.store(true, Ordering::Release);
                res
            })
        };

        ready.wait();
        std::thread::sleep(Duration::from_millis(25));
        assert!(!done_load.load(Ordering::Acquire));
        assert!(!done_auth.load(Ordering::Acquire));

        let mut updated = AccountsFile::default();
        updated.accounts.push(meta("b", "Blake"));
        updated.save(&p).unwrap();
        drop(guard);

        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();
        assert_eq!(r1.accounts[0].id, "b");
        assert_eq!(r2.accounts[0].id, "b");

        // Schema != 1 rejected.
        std::fs::write(&p, r#"{"schema":2,"active":null,"accounts":[]}"#).unwrap();
        assert_eq!(
            state.load().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(
            state.load_authoritative().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );

        // > 1 MiB rejected.
        std::fs::write(&p, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert_eq!(
            state.load().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(
            state.load_authoritative().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
