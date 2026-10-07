//! Signing in: the loopback listener for the browser redirect, the sign-in orchestration, token
//! storage in the encrypted vault, token refresh, and the Tauri commands that drive it.
//!
//! Protocol details live in [`crate::ms_auth`]. This module owns the moving parts around it:
//! - **System browser only.** The launcher opens Microsoft's own page in the user's browser and
//!   listens on `127.0.0.1` (random port, one connection at a time, short deadline) for the
//!   redirect. It never sees or asks for a password and shows no login form of its own.
//! - **Callback validation:** exact method and path, `Host` header must be this listener
//!   (DNS-rebinding guard), `state` must match in constant time; anything else is ignored.
//! - **Secrets** go only into the vault (`data/vault.bin`, Argon2id + XChaCha20-Poly1305). The
//!   vault must be unlocked before a sign-in starts so a finished sign-in can always be saved.
//!   `accounts.json` receives metadata only (id, UUID, Java profile name, state).
//! - **Never deleting** an account because its credentials expired: refresh failure only marks it
//!   "needs sign in again".

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::time::timeout;
use url::Url;

use crate::accounts::{AccountMeta, AccountStatus, ReauthReason};
use crate::accounts_api::{AccountsDto, AccountsState};
use crate::coded_error::CodedError;
use crate::ms_auth::{
    authorize_url, complete_chain, exchange_code, random_token, refresh_ms, AuthError, JavaProfile,
    McSession, Pkce, Secret,
};
use crate::net::{HttpFetcher, Transport};
use crate::profiles::now_unix;
use crate::vault::{AccountSecrets, Vault, VaultError};

/// How long the browser sign-in may take before the launcher gives up listening.
pub const SIGN_IN_WAIT: Duration = Duration::from_secs(5 * 60);
/// A cached Minecraft token this close to expiry is refreshed instead of used.
const TOKEN_MARGIN_SECS: i64 = 300;
/// How long sign-in or a refresh waits for a decorative avatar download before answering without it.
const SKIN_WAIT: Duration = Duration::from_secs(3);
pub const PROGRESS_EVENT: &str = "sign-in-progress";

// ------------------------------------------------------------------------------ callback

#[derive(Debug)]
pub enum Callback {
    Code(Secret),
    /// The user (or Microsoft) ended the sign-in: the OAuth `error` value.
    Denied(String),
    /// Not the redirect we are waiting for (favicon, a stray tab, a forged request).
    Ignore,
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Validates the first lines of an HTTP request against the sign-in in progress.
pub fn parse_callback(head: &str, port: u16, expected_state: &str) -> Callback {
    let mut lines = head.split("\r\n");
    let request = lines.next().unwrap_or("");
    let mut parts = request.split(' ');
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Ignore;
    };
    let host_ok = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.strip_prefix("host:").is_some_and(|h| {
            let h = h.trim();
            h == format!("localhost:{port}") || h == format!("127.0.0.1:{port}")
        })
    });
    if !host_ok {
        return Callback::Ignore;
    }
    let Ok(url) = Url::parse(&format!("http://localhost:{port}{target}")) else {
        return Callback::Ignore;
    };
    if url.path() != "/" {
        return Callback::Ignore;
    }
    let (mut state, mut code, mut error) = (None, None, None);
    for (k, v) in url.query_pairs() {
        match &*k {
            "state" => state = Some(v.into_owned()),
            "code" => code = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if !state
        .as_deref()
        .is_some_and(|s| constant_time_eq(s, expected_state))
    {
        return Callback::Ignore;
    }
    if let Some(e) = error {
        return Callback::Denied(e);
    }
    match code {
        Some(c) if !c.is_empty() && c.len() <= 4096 => Callback::Code(Secret::new(c)),
        _ => Callback::Ignore,
    }
}

const PAGE_DONE: &str = "<!doctype html><meta charset=utf-8><title>Signed in</title><body style=\"font-family:system-ui;margin:4rem auto;max-width:28rem;color:#2b2622\"><h1 style=\"font-weight:500\">You are signed in.</h1><p>You can close this tab and return to the launcher.</p>";
const PAGE_STOPPED: &str = "<!doctype html><meta charset=utf-8><title>Sign-in stopped</title><body style=\"font-family:system-ui;margin:4rem auto;max-width:28rem;color:#2b2622\"><h1 style=\"font-weight:500\">Sign-in stopped.</h1><p>You can close this tab and return to the launcher.</p>";

async fn respond(stream: &mut TcpStream, status: &str, page: &str) {
    let out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(out.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while buf.len() < 8192 {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// The one-shot listener for the browser's redirect.
pub struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    pub async fn bind() -> Result<Self, AuthError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| AuthError::BrowserUnavailable)?;
        let port = listener
            .local_addr()
            .map_err(|_| AuthError::BrowserUnavailable)?
            .port();
        Ok(Self { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://localhost:<port>`. Microsoft matches the registered `http://localhost` on any port.
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    pub async fn wait_for_code(&self, state: &str, wait: Duration) -> Result<Secret, AuthError> {
        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AuthError::TimedOut);
            }
            let (mut stream, _) = timeout(remaining, self.listener.accept())
                .await
                .map_err(|_| AuthError::TimedOut)?
                .map_err(|_| AuthError::BrowserUnavailable)?;
            let Ok(Some(head)) = timeout(Duration::from_secs(3), read_head(&mut stream)).await
            else {
                continue;
            };
            match parse_callback(&head, self.port, state) {
                Callback::Code(code) => {
                    respond(&mut stream, "200 OK", PAGE_DONE).await;
                    return Ok(code);
                }
                Callback::Denied(error) => {
                    respond(&mut stream, "200 OK", PAGE_STOPPED).await;
                    return Err(if error == "access_denied" {
                        AuthError::Cancelled
                    } else {
                        AuthError::OAuth(error.chars().take(40).collect())
                    });
                }
                Callback::Ignore => respond(&mut stream, "404 Not Found", "Not found").await,
            }
        }
    }
}

// ------------------------------------------------------------------------------ the sign-in

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Opening,
    Waiting,
    Finishing,
}

pub struct SignedIn {
    pub refresh: Secret,
    pub session: McSession,
    pub profile: JavaProfile,
}

/// Runs one browser sign-in end to end. Cancellable at every step.
pub async fn sign_in<T: Transport>(
    t: &T,
    loopback: &Loopback,
    open: impl FnOnce(&Url) -> bool,
    progress: impl Fn(Stage),
    cancel: &Notify,
    wait: Duration,
) -> Result<SignedIn, AuthError> {
    let pkce = Pkce::generate()?;
    let state = random_token()?;
    let redirect = loopback.redirect_uri();
    let url = authorize_url(&redirect, &state, &pkce.challenge);

    progress(Stage::Opening);
    if !open(&url) {
        return Err(AuthError::BrowserUnavailable);
    }
    progress(Stage::Waiting);
    let code = tokio::select! {
        r = loopback.wait_for_code(&state, wait) => r?,
        () = cancel.notified() => return Err(AuthError::Cancelled),
    };

    progress(Stage::Finishing);
    let finish = async {
        let tokens = exchange_code(t, &redirect, &code, &pkce.verifier).await?;
        let refresh = tokens
            .refresh
            .ok_or(AuthError::BadResponse("no refresh token"))?;
        let (session, profile) = complete_chain(t, &tokens.access).await?;
        Ok(SignedIn {
            refresh,
            session,
            profile,
        })
    };
    tokio::select! {
        r = finish => r,
        () = cancel.notified() => Err(AuthError::Cancelled),
    }
}

// ------------------------------------------------------------------------------ storage

fn same_uuid(a: &str, b: &str) -> bool {
    a.replace('-', "").eq_ignore_ascii_case(&b.replace('-', ""))
}

fn new_account_id() -> Result<String, AuthError> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|_| AuthError::BadResponse("no randomness"))?;
    Ok(format!("acc-{}", hex::encode(b)))
}

fn secrets_for(refresh: &Secret, session: &McSession, now: i64) -> AccountSecrets {
    AccountSecrets {
        ms_refresh_token: Some(refresh.expose().to_owned()),
        minecraft_access_token: Some(session.access_token.expose().to_owned()),
        minecraft_token_expires_at: Some(now + i64::try_from(session.expires_in).unwrap_or(3600)),
    }
}

fn storage(e: impl std::fmt::Display) -> AuthError {
    AuthError::Storage(e.to_string())
}

/// Saves a finished sign-in: the secrets to the vault first, then the metadata (a crash between
/// the two leaves only an orphaned secret, purged at the next unlock). A UUID that is already in
/// the list updates that entry instead of adding a duplicate. `reauth_for` insists on one account.
pub fn persist(
    vault: &mut Vault,
    accounts: &AccountsState,
    signed: &SignedIn,
    reauth_for: Option<&str>,
    now: i64,
) -> Result<String, AuthError> {
    let profile = &signed.profile;
    accounts
        .update(|file| {
            let existing = file
                .accounts
                .iter()
                .find(|a| same_uuid(&a.minecraft_uuid, &profile.uuid))
                .map(|a| a.id.clone());
            let id = match reauth_for {
                Some(wanted) => {
                    let acc = file
                        .accounts
                        .iter()
                        .find(|a| a.id == wanted)
                        .ok_or(AuthError::UnknownAccount)?;
                    if !same_uuid(&acc.minecraft_uuid, &profile.uuid) {
                        return Err(AuthError::WrongAccount);
                    }
                    wanted.to_owned()
                }
                None => match existing {
                    Some(id) => id,
                    None => new_account_id()?,
                },
            };
            vault
                .set_account(&id, secrets_for(&signed.refresh, &signed.session, now))
                .map_err(storage)?;
            match file.accounts.iter_mut().find(|a| a.id == id) {
                Some(a) => {
                    a.username = profile.name.clone();
                    a.status = AccountStatus::Ready;
                }
                None => file.accounts.push(AccountMeta {
                    id: id.clone(),
                    username: profile.name.clone(),
                    minecraft_uuid: profile.uuid.clone(),
                    added_at: now,
                    last_used_at: None,
                    status: AccountStatus::Ready,
                }),
            }
            file.active = Some(id.clone());
            Ok(id)
        })
        .and_then(|r| r)
}

/// Makes sure an account has a usable Minecraft access token (for a launch that is about to
/// happen): returns the cached one while it is fresh, otherwise refreshes the Microsoft token
/// and repeats the chain. A refresh the user must fix marks the account "needs sign in again"
/// and keeps it; an outage changes nothing.
pub async fn ensure_fresh<T: Transport>(
    t: &T,
    vault: &Mutex<Option<Vault>>,
    accounts: &AccountsState,
    id: &str,
    now: i64,
) -> Result<Secret, AuthError> {
    // Which vault this refresh belongs to. The network steps below take a while; if the vault is
    // replaced meanwhile (it was archived by "Start fresh" and a new one created), the results of this
    // refresh must not be written into it.
    let (secrets, generation) = {
        let guard = lock(vault);
        let v = guard.as_ref().ok_or(AuthError::VaultLocked)?;
        (v.account(id).cloned(), v.generation())
    };
    let mark = |reason: ReauthReason| {
        let _ = accounts.update(|f| f.mark_needs_reauth(id, reason, now));
    };
    let Some(secrets) = secrets else {
        mark(ReauthReason::Unknown); // e.g. the vault came from another folder
        return Err(AuthError::NeedsSignIn(ReauthReason::Unknown));
    };
    if let (Some(token), Some(expires)) = (
        &secrets.minecraft_access_token,
        secrets.minecraft_token_expires_at,
    ) {
        if expires > now + TOKEN_MARGIN_SECS {
            return Ok(Secret::new(token.clone()));
        }
    }
    let Some(old_refresh) = &secrets.ms_refresh_token else {
        mark(ReauthReason::Unknown);
        return Err(AuthError::NeedsSignIn(ReauthReason::Unknown));
    };
    let tokens = match refresh_ms(t, &Secret::new(old_refresh.clone())).await {
        Ok(tokens) => tokens,
        Err(AuthError::NeedsSignIn(reason)) => {
            mark(reason);
            return Err(AuthError::NeedsSignIn(reason));
        }
        Err(e) => return Err(e),
    };
    // Microsoft rotates refresh tokens, and the old one may stop working once it has been used. So
    // the new one is saved to the vault right now, before the Xbox and Minecraft steps: if one of
    // them fails (an outage, a rate limit), the account still holds a refresh token that works. Only
    // the refresh token is written here; the short-lived Minecraft token and its expiry stay as they
    // were until the whole chain has succeeded.
    if let Some(new_refresh) = &tokens.refresh {
        let mut guard = lock(vault);
        let v = guard.as_mut().ok_or(AuthError::VaultLocked)?;
        if v.generation() != generation {
            return Err(AuthError::VaultLocked);
        }
        let mut updated = secrets.clone();
        updated.ms_refresh_token = Some(new_refresh.expose().to_owned());
        v.set_account(id, updated).map_err(storage)?;
    }
    let (session, profile) = complete_chain(t, &tokens.access).await?;
    // The refresh token to keep: the new one, or the old one if none came back.
    let refresh = tokens
        .refresh
        .unwrap_or_else(|| Secret::new(old_refresh.clone()));
    {
        let mut guard = lock(vault);
        let v = guard.as_mut().ok_or(AuthError::VaultLocked)?;
        if v.generation() != generation {
            return Err(AuthError::VaultLocked); // the vault changed: nothing of this refresh is kept
        }
        v.set_account(id, secrets_for(&refresh, &session, now))
            .map_err(storage)?;
    }
    accounts.update(|f| match f.accounts.iter_mut().find(|a| a.id == id) {
        Some(a) if same_uuid(&a.minecraft_uuid, &profile.uuid) => {
            a.username = profile.name.clone();
            a.status = AccountStatus::Ready;
            Ok(())
        }
        Some(_) => Err(AuthError::WrongAccount),
        None => Err(AuthError::UnknownAccount),
    })??;
    // The profile just reported the current skin: note it (the download happens off this path).
    accounts.record_skin(&profile.uuid, profile.skin_url.as_deref());
    Ok(session.access_token)
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------------------ commands

/// Shared sign-in state: the (possibly locked) vault and the sign-in in progress, if any.
pub struct AuthState {
    pub vault_path: std::path::PathBuf,
    pub vault: Mutex<Option<Vault>>,
    pub(crate) running: Mutex<Option<Arc<Notify>>>,
    /// Bumped by "Start fresh" (`vault_recovery`). An unlock or create that began before it must not
    /// install its vault afterwards: that vault's file has been archived.
    pub(crate) epoch: AtomicU64,
    pub http: Result<HttpFetcher, String>,
}

impl AuthState {
    pub fn new(vault_path: std::path::PathBuf, http: Result<HttpFetcher, String>) -> Self {
        Self {
            vault_path,
            vault: Mutex::new(None),
            running: Mutex::new(None),
            epoch: AtomicU64::new(0),
            http,
        }
    }

    /// Puts an unlocked or freshly created vault into the slot, unless "Start fresh" archived the
    /// vault files since this operation began (`started` is the epoch it saw then). `after` runs under
    /// the vault lock, only when the vault is really installed.
    pub(crate) fn install_vault(
        &self,
        mut vault: Vault,
        started: u64,
        after: impl FnOnce(&mut Vault),
    ) -> Result<(), CodedError> {
        let mut slot = lock(&self.vault);
        if self.epoch.load(Ordering::SeqCst) != started {
            return Err(CodedError::new(
                "vault_missing",
                "the vault was archived while it was being opened",
            ));
        }
        after(&mut vault);
        *slot = Some(vault);
        Ok(())
    }

    /// Removes an account's secrets if the vault is unlocked. A locked vault keeps them until
    /// the next unlock, which purges every entry that has no account.
    pub fn forget(&self, id: &str) {
        if let Some(v) = lock(&self.vault).as_mut() {
            if let Err(e) = v.remove_account(id) {
                tracing::warn!(error = %e, "could not remove an account's secrets");
            }
        }
    }
}

pub(crate) struct RunGuard<'a>(pub(crate) &'a Mutex<Option<Arc<Notify>>>);
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        *lock(self.0) = None;
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatusDto {
    pub exists: bool,
    pub unlocked: bool,
}

/// What the frontend receives when sign-in fails: a stable code and a plain message.
#[derive(Debug, Clone, Serialize)]
pub struct SignInError {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// English detail for logs; the UI words the failure from `code` and `params`.
    pub message: String,
}

impl From<AuthError> for SignInError {
    fn from(e: AuthError) -> Self {
        Self {
            code: e.code(),
            params: e.params(),
            message: e.to_string(),
        }
    }
}

/// The vault's failures as stable codes (the UI words them).
pub(crate) fn vault_error(e: &VaultError) -> CodedError {
    match e {
        VaultError::Authentication => CodedError::new("passphrase_mismatch", "passphrase rejected"),
        VaultError::PassphraseTooShort => CodedError::new("passphrase_too_short", e.to_string()),
        VaultError::AlreadyExists => CodedError::new("vault_exists", "a vault already exists"),
        VaultError::Missing => CodedError::new("vault_missing", "there is no vault file"),
        _ => CodedError::new("vault_unreadable", e.to_string()),
    }
}

#[tauri::command]
pub fn vault_status(auth: State<'_, AuthState>) -> VaultStatusDto {
    VaultStatusDto {
        exists: Vault::exists(&auth.vault_path),
        unlocked: lock(&auth.vault).is_some(),
    }
}

#[tauri::command]
pub async fn create_vault(
    auth: State<'_, AuthState>,
    passphrase: String,
) -> Result<(), CodedError> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    let path = auth.vault_path.clone();
    let started = auth.epoch.load(Ordering::SeqCst);
    let vault = tokio::task::spawn_blocking(move || Vault::create(&path, &passphrase))
        .await
        .map_err(|_| CodedError::new("vault_unreadable", "the vault task failed"))?
        .map_err(|e| vault_error(&e))?;
    auth.install_vault(vault, started, |_| ())
}

/// Drops the vault entries of accounts that were removed while the vault was locked.
///
/// This deletes credentials, so it only runs against an account list that is known to be
/// authoritative: `accounts.json` must exist and parse. A missing file (a copied `vault.bin`, a
/// restored backup, a deleted list) or a damaged one proves nothing about which accounts are gone,
/// so the vault is left exactly as it is. A file that parses and really lists no accounts is a real
/// empty list. Returns how many entries were removed.
pub(crate) fn purge_orphans(vault: &mut Vault, accounts_path: &Path) -> usize {
    let file = match crate::accounts::AccountsFile::load_authoritative(accounts_path) {
        Ok(Some(file)) => file,
        Ok(None) => {
            tracing::info!("accounts.json is missing; the vault was left untouched");
            return 0;
        }
        Err(e) => {
            tracing::warn!(error = %e, "accounts.json could not be read; the vault was left untouched");
            return 0;
        }
    };
    let orphans: Vec<String> = vault
        .account_ids()
        .filter(|id| !file.accounts.iter().any(|a| a.id == *id))
        .map(str::to_owned)
        .collect();
    let mut removed = 0;
    for id in orphans {
        match vault.remove_account(&id) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(error = %e, "could not remove an orphaned account secret"),
        }
    }
    removed
}

#[tauri::command]
pub async fn unlock_vault(
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    passphrase: String,
) -> Result<(), CodedError> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    let path = auth.vault_path.clone();
    let started = auth.epoch.load(Ordering::SeqCst);
    let vault = tokio::task::spawn_blocking(move || Vault::unlock(&path, &passphrase))
        .await
        .map_err(|_| CodedError::new("vault_unreadable", "the vault task failed"))?
        .map_err(|e| vault_error(&e))?;
    auth.install_vault(vault, started, |v| {
        purge_orphans(v, accounts.path());
    })
}

fn transport(auth: &AuthState) -> Result<&HttpFetcher, AuthError> {
    auth.http
        .as_ref()
        .map_err(|_| AuthError::BadResponse("networking is unavailable"))
}

#[tauri::command]
pub async fn begin_sign_in(
    app: AppHandle,
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    account_id: Option<String>,
) -> Result<AccountsDto, SignInError> {
    run_sign_in(&app, &auth, &accounts, account_id)
        .await
        .map_err(|e| {
            tracing::info!(code = e.code(), "sign-in did not complete");
            SignInError::from(e)
        })
}

async fn run_sign_in(
    app: &AppHandle,
    auth: &AuthState,
    accounts: &AccountsState,
    reauth_for: Option<String>,
) -> Result<AccountsDto, AuthError> {
    if lock(&auth.vault).is_none() {
        return Err(AuthError::VaultLocked);
    }
    let cancel = Arc::new(Notify::new());
    {
        let mut running = lock(&auth.running);
        if running.is_some() {
            return Err(AuthError::InProgress);
        }
        *running = Some(cancel.clone());
    }
    let _guard = RunGuard(&auth.running);

    let t = transport(auth)?;
    let loopback = Loopback::bind().await?;
    let signed = sign_in(
        t,
        &loopback,
        |url| app.opener().open_url(url.as_str(), None::<&str>).is_ok(),
        |stage| {
            let _ = app.emit(PROGRESS_EVENT, stage);
        },
        &cancel,
        SIGN_IN_WAIT,
    )
    .await;
    // The browser took focus for the sign-in: bring the launcher back, whatever the outcome.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    let signed = signed?;

    {
        let mut guard = lock(&auth.vault);
        let vault = guard.as_mut().ok_or(AuthError::VaultLocked)?;
        persist(vault, accounts, &signed, reauth_for.as_deref(), now_unix())?;
    }
    tracing::info!("signed in a Minecraft Java account");
    // The avatar is cosmetic: noting and fetching the skin can never fail the sign-in.
    accounts.record_skin(&signed.profile.uuid, signed.profile.skin_url.as_deref());
    accounts.sync_pending_skins(SKIN_WAIT).await; // briefly; whatever is not done finishes later
    accounts
        .list()
        .map_err(|e| AuthError::Storage(e.to_string()))
}

#[tauri::command]
pub fn cancel_sign_in(auth: State<'_, AuthState>) {
    if let Some(n) = lock(&auth.running).as_ref() {
        n.notify_one();
    }
}

/// Refreshes one account's credentials now (the same step that will run before a launch).
#[tauri::command]
pub async fn refresh_account(
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    id: String,
) -> Result<AccountsDto, SignInError> {
    let result = async {
        let t = transport(&auth)?;
        ensure_fresh(t, &auth.vault, &accounts, &id, now_unix()).await?;
        Ok(())
    }
    .await;
    match result {
        // A needs-sign-in verdict is recorded on the account; the list shows it.
        Ok(()) | Err(AuthError::NeedsSignIn(_)) => {
            accounts.sync_pending_skins(SKIN_WAIT).await;
            accounts
                .list()
                .map_err(|e| AuthError::Storage(e.to_string()).into())
        }
        Err(e) => Err(e.into()),
    }
}

pub fn exists_in(path: &Path) -> bool {
    Vault::exists(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::AccountsFile;
    use crate::ms_auth::ENDPOINTS;
    use crate::net::testing_transport::FakeTransport;
    use crate::vault::KdfParams;

    const FAST: KdfParams = KdfParams {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };
    const TOKEN: &str = ENDPOINTS[1];
    const XBL: &str = ENDPOINTS[2];
    const XSTS: &str = ENDPOINTS[3];
    const LOGIN: &str = ENDPOINTS[4];
    const PROFILE: &str = ENDPOINTS[6];
    const UUID_RAW: &str = "0000000000004000800000000000a001";

    fn script_ok(t: &FakeTransport, refresh: &str, name: &str, uuid_raw: &str) {
        t.reply(
            TOKEN,
            200,
            format!(
                r#"{{"access_token":"MS-ACCESS","refresh_token":"{refresh}","expires_in":3600}}"#
            ),
        );
        t.reply(
            XBL,
            200,
            r#"{"Token":"XBL","DisplayClaims":{"xui":[{"uhs":"99"}]}}"#,
        );
        t.reply(
            XSTS,
            200,
            r#"{"Token":"XSTS","DisplayClaims":{"xui":[{"uhs":"99"}]}}"#,
        );
        t.reply(
            LOGIN,
            200,
            r#"{"access_token":"MC-ACCESS","expires_in":86400}"#,
        );
        t.reply(
            PROFILE,
            200,
            format!(r#"{{"id":"{uuid_raw}","name":"{name}"}}"#),
        );
    }

    struct Env {
        dir: tempfile::TempDir,
        accounts: AccountsState,
        vault: Mutex<Option<Vault>>,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            Vault::create_with(&dir.path().join("vault.bin"), "correct horse battery", FAST)
                .unwrap();
        Env {
            accounts: AccountsState::new(dir.path().join("accounts.json")),
            vault: Mutex::new(Some(vault)),
            dir,
        }
    }

    fn signed(refresh: &str, name: &str, uuid: &str) -> SignedIn {
        SignedIn {
            refresh: Secret::new(refresh.into()),
            session: McSession {
                access_token: Secret::new("MC-ACCESS".into()),
                expires_in: 86400,
            },
            profile: JavaProfile {
                uuid: uuid.into(),
                name: name.into(),
                skin_url: None,
            },
        }
    }

    const UUID: &str = "00000000-0000-4000-8000-00000000a001";
    const UUID_B: &str = "00000000-0000-4000-8000-00000000b002";

    /// Plays the browser: reads state and redirect from the authorize URL and calls back.
    fn browser(outcome: &'static str) -> impl FnOnce(&Url) -> bool {
        move |url: &Url| {
            let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            let redirect = Url::parse(&q["redirect_uri"]).unwrap();
            let port = redirect.port().unwrap();
            let state = q["state"].clone();
            tokio::spawn(async move {
                let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                let target = match outcome {
                    "code" => format!("/?code=THE-CODE&state={state}"),
                    "denied" => {
                        format!("/?error=access_denied&error_description=cancelled&state={state}")
                    }
                    _ => "/?code=X&state=WRONG".to_owned(),
                };
                let req = format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n");
                s.write_all(req.as_bytes()).await.unwrap();
                let mut sink = Vec::new();
                let _ = s.read_to_end(&mut sink).await;
            });
            true
        }
    }

    // ---- callback parsing

    fn vault_with(ids: &[&str]) -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let mut v =
            Vault::create_with(&dir.path().join("vault.bin"), "correct horse battery", FAST)
                .unwrap();
        for id in ids {
            v.set_account(
                id,
                secrets_for(&Secret::new(format!("R-{id}")), &mc_session(), 100),
            )
            .unwrap();
        }
        (dir, v)
    }

    fn mc_session() -> McSession {
        McSession {
            access_token: Secret::new("MC".into()),
            expires_in: 3600,
        }
    }

    fn ids(v: &Vault) -> Vec<String> {
        v.account_ids().map(str::to_owned).collect()
    }

    #[test]
    fn a_missing_or_damaged_account_list_can_never_erase_credentials() {
        let (dir, mut v) = vault_with(&["acc-a", "acc-b"]);
        let list = dir.path().join("accounts.json");
        // no accounts.json at all
        assert_eq!(purge_orphans(&mut v, &list), 0);
        assert_eq!(ids(&v), ["acc-a", "acc-b"]);
        // a damaged one (empty, truncated, garbage)
        for bytes in [&b""[..], b"{\"schema\":1,\"accounts\":[", b"nope"] {
            std::fs::write(&list, bytes).unwrap();
            assert_eq!(purge_orphans(&mut v, &list), 0);
            assert_eq!(ids(&v), ["acc-a", "acc-b"]);
        }
        // and the vault on disk still has both
        assert_eq!(
            Vault::unlock(&dir.path().join("vault.bin"), "correct horse battery")
                .map(|v| ids(&v))
                .unwrap(),
            ["acc-a", "acc-b"]
        );
    }

    #[test]
    fn an_authoritative_list_still_cleans_up_orphans() {
        let (dir, mut v) = vault_with(&["acc-a", "acc-b"]);
        let list = dir.path().join("accounts.json");
        let mut f = AccountsFile::default();
        f.accounts.push(AccountMeta {
            id: "acc-a".into(),
            username: "A".into(),
            minecraft_uuid: "00000000-0000-4000-8000-00000000a001".into(),
            added_at: 1,
            last_used_at: None,
            status: AccountStatus::Ready,
        });
        f.save(&list).unwrap();
        assert_eq!(purge_orphans(&mut v, &list), 1);
        assert_eq!(ids(&v), ["acc-a"]);
        // a list that really is empty (it exists and parses) keeps its meaning: nothing is owned
        AccountsFile::default().save(&list).unwrap();
        assert_eq!(purge_orphans(&mut v, &list), 1);
        assert!(ids(&v).is_empty());
    }

    #[test]
    fn only_the_exact_expected_callback_is_accepted() {
        let ok = "GET /?code=abc&state=S HTTP/1.1\r\nHost: localhost:5000\r\nUser-Agent: x\r\n\r\n";
        assert!(matches!(parse_callback(ok, 5000, "S"), Callback::Code(c) if c.expose() == "abc"));
        assert!(matches!(
            parse_callback(&ok.replace("localhost", "127.0.0.1"), 5000, "S"),
            Callback::Code(_)
        ));
        for bad in [
            "GET /?code=abc&state=OTHER HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
            "GET /?code=abc HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
            "GET /other?code=abc&state=S HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
            "POST /?code=abc&state=S HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
            "GET /?code=abc&state=S HTTP/1.1\r\nHost: evil.example:5000\r\n\r\n",
            "GET /?code=abc&state=S HTTP/1.1\r\n\r\n",
            "GET /?code=&state=S HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
            "GET /favicon.ico HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
            "garbage",
        ] {
            assert!(
                matches!(parse_callback(bad, 5000, "S"), Callback::Ignore),
                "{bad}"
            );
        }
        let denied = "GET /?error=access_denied&state=S HTTP/1.1\r\nHost: localhost:5000\r\n\r\n";
        assert!(
            matches!(parse_callback(denied, 5000, "S"), Callback::Denied(e) if e == "access_denied")
        );
        // An error without the right state is not trusted either.
        assert!(matches!(
            parse_callback(&denied.replace("state=S", "state=Z"), 5000, "S"),
            Callback::Ignore
        ));
    }

    // ---- the loopback listener over real TCP

    #[tokio::test]
    async fn the_listener_ignores_strangers_and_returns_the_real_code() {
        let lb = Loopback::bind().await.unwrap();
        let port = lb.port();
        tokio::spawn(async move {
            for target in [
                "/favicon.ico",
                "/?code=FORGED&state=nope",
                "/?code=REAL&state=S",
            ] {
                let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                s.write_all(
                    format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes(),
                )
                .await
                .unwrap();
                let mut out = Vec::new();
                let _ = s.read_to_end(&mut out).await;
            }
        });
        let code = lb.wait_for_code("S", Duration::from_secs(5)).await.unwrap();
        assert_eq!(code.expose(), "REAL");
    }

    #[tokio::test]
    async fn the_listener_times_out_cleanly() {
        let lb = Loopback::bind().await.unwrap();
        let r = lb.wait_for_code("S", Duration::from_millis(150)).await;
        assert!(matches!(r, Err(AuthError::TimedOut)));
    }

    // ---- the sign-in

    #[tokio::test]
    async fn a_successful_sign_in_uses_pkce_and_ends_with_a_verified_java_profile() {
        let t = FakeTransport::new();
        script_ok(&t, "MS-REFRESH", "Steve_01", UUID_RAW);
        let lb = Loopback::bind().await.unwrap();
        let stages = Mutex::new(Vec::new());
        let challenge = Mutex::new(String::new());
        let open = browser("code");
        let s = sign_in(
            &t,
            &lb,
            |url| {
                *challenge.lock().unwrap() = url
                    .query_pairs()
                    .find(|(k, _)| k == "code_challenge")
                    .unwrap()
                    .1
                    .into_owned();
                open(url)
            },
            |st| stages.lock().unwrap().push(st),
            &Notify::new(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(s.profile.name, "Steve_01");
        assert_eq!(
            *stages.lock().unwrap(),
            [Stage::Opening, Stage::Waiting, Stage::Finishing]
        );
        // The verifier sent to the token endpoint hashes to the challenge sent to the browser.
        let body = &t.calls()[0].3;
        let verifier = body
            .split('&')
            .find_map(|p| p.strip_prefix("code_verifier="))
            .unwrap();
        assert_eq!(
            crate::ms_auth::Pkce::from_verifier(verifier.to_owned()).challenge,
            *challenge.lock().unwrap()
        );
        assert!(body.contains("code=THE-CODE"));
    }

    #[tokio::test]
    async fn cancelling_in_the_browser_or_in_the_launcher_is_a_clean_cancel() {
        let t = FakeTransport::new();
        let lb = Loopback::bind().await.unwrap();
        let r = sign_in(
            &t,
            &lb,
            browser("denied"),
            |_| {},
            &Notify::new(),
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(r, Err(AuthError::Cancelled)));

        let lb = Loopback::bind().await.unwrap();
        let cancel = Arc::new(Notify::new());
        let c2 = cancel.clone();
        let r = sign_in(
            &t,
            &lb,
            |_| true,
            move |st| {
                if st == Stage::Waiting {
                    c2.notify_one();
                }
            },
            &cancel,
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(r, Err(AuthError::Cancelled)));
        assert!(t.calls().is_empty(), "nothing was sent to Microsoft");
    }

    #[tokio::test]
    async fn a_missing_browser_and_a_forged_callback_do_not_sign_anyone_in() {
        let t = FakeTransport::new();
        let lb = Loopback::bind().await.unwrap();
        let r = sign_in(
            &t,
            &lb,
            |_| false,
            |_| {},
            &Notify::new(),
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(r, Err(AuthError::BrowserUnavailable)));
        let lb = Loopback::bind().await.unwrap();
        let r = sign_in(
            &t,
            &lb,
            browser("forged"),
            |_| {},
            &Notify::new(),
            Duration::from_millis(300),
        )
        .await;
        assert!(matches!(r, Err(AuthError::TimedOut)));
        assert!(t.calls().is_empty());
    }

    #[tokio::test]
    async fn the_unapproved_launcher_stops_at_minecraft_services_and_saves_nothing() {
        let t = FakeTransport::new();
        script_ok(&t, "R", "Steve", UUID_RAW);
        t.clear(LOGIN);
        t.reply(LOGIN, 403, r#"{"errorMessage":"Invalid app registration"}"#);
        let lb = Loopback::bind().await.unwrap();
        let r = sign_in(
            &t,
            &lb,
            browser("code"),
            |_| {},
            &Notify::new(),
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(r, Err(AuthError::ApprovalRequired)));
    }

    // ---- storage

    #[test]
    fn a_new_account_is_saved_with_secrets_only_in_the_vault() {
        let e = env();
        let id = persist(
            e.vault.lock().unwrap().as_mut().unwrap(),
            &e.accounts,
            &signed("MS-REFRESH-SECRET", "Steve_01", UUID),
            None,
            100,
        )
        .unwrap();
        let file = AccountsFile::load(&e.dir.path().join("accounts.json")).unwrap();
        assert_eq!(
            (file.accounts.len(), file.active.as_deref()),
            (1, Some(id.as_str()))
        );
        assert_eq!(file.accounts[0].username, "Steve_01");
        assert_eq!(file.accounts[0].minecraft_uuid, UUID);
        let json = std::fs::read_to_string(e.dir.path().join("accounts.json")).unwrap();
        assert!(
            !json.contains("SECRET")
                && !json.contains("MC-ACCESS")
                && !json.to_lowercase().contains("token")
        );
        // The vault file is ciphertext and the secrets come back after a fresh unlock.
        let raw = std::fs::read(e.dir.path().join("vault.bin")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("MS-REFRESH-SECRET"));
        e.vault.lock().unwrap().take();
        let v = Vault::unlock(&e.dir.path().join("vault.bin"), "correct horse battery").unwrap();
        assert_eq!(
            v.account(&id).unwrap().ms_refresh_token.as_deref(),
            Some("MS-REFRESH-SECRET")
        );
    }

    #[test]
    fn signing_in_the_same_minecraft_account_twice_updates_instead_of_duplicating() {
        let e = env();
        let mut g = e.vault.lock().unwrap();
        let v = g.as_mut().unwrap();
        let a = persist(v, &e.accounts, &signed("R1", "OldName", UUID), None, 1).unwrap();
        let b = persist(
            v,
            &e.accounts,
            &signed("R2", "NewName", &UUID.to_uppercase()),
            None,
            2,
        )
        .unwrap();
        let c = persist(v, &e.accounts, &signed("R3", "Other", UUID_B), None, 3).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let file = AccountsFile::load(e.accounts.path()).unwrap();
        assert_eq!(file.accounts.len(), 2);
        assert_eq!(
            file.accounts[0].username, "NewName",
            "the profile name follows a rename"
        );
        assert_eq!(
            v.account(&a).unwrap().ms_refresh_token.as_deref(),
            Some("R2")
        );
        assert_eq!(file.active.as_deref(), Some(c.as_str()));
    }

    #[test]
    fn signing_in_again_must_be_the_same_minecraft_account() {
        let e = env();
        let mut g = e.vault.lock().unwrap();
        let v = g.as_mut().unwrap();
        let a = persist(v, &e.accounts, &signed("R1", "Steve", UUID), None, 1).unwrap();
        e.accounts
            .update(|f| f.mark_needs_reauth(&a, ReauthReason::Revoked, 5))
            .unwrap();
        let wrong = persist(v, &e.accounts, &signed("R2", "Alex", UUID_B), Some(&a), 6);
        assert!(matches!(wrong, Err(AuthError::WrongAccount)));
        assert!(matches!(
            persist(
                v,
                &e.accounts,
                &signed("R2", "Alex", UUID_B),
                Some("nope"),
                6
            ),
            Err(AuthError::UnknownAccount)
        ));
        assert!(
            AccountsFile::load(e.accounts.path()).unwrap().accounts[0].status
                != AccountStatus::Ready
        );
        persist(v, &e.accounts, &signed("R3", "Steve", UUID), Some(&a), 7).unwrap();
        let file = AccountsFile::load(e.accounts.path()).unwrap();
        assert_eq!(
            (file.accounts.len(), &file.accounts[0].status),
            (1, &AccountStatus::Ready)
        );
    }

    // ---- token lifecycle

    fn seeded(e: &Env, expires: i64) -> String {
        let id = persist(
            e.vault.lock().unwrap().as_mut().unwrap(),
            &e.accounts,
            &signed("OLD-REFRESH", "Steve_01", UUID),
            None,
            0,
        )
        .unwrap();
        let mut g = e.vault.lock().unwrap();
        let v = g.as_mut().unwrap();
        let mut s = v.account(&id).unwrap().clone();
        s.minecraft_token_expires_at = Some(expires);
        v.set_account(&id, s).unwrap();
        id
    }

    #[tokio::test]
    async fn a_fresh_token_is_reused_without_any_network_traffic() {
        let e = env();
        let id = seeded(&e, 10_000);
        let t = FakeTransport::new();
        let tok = ensure_fresh(&t, &e.vault, &e.accounts, &id, 100)
            .await
            .unwrap();
        assert_eq!(tok.expose(), "MC-ACCESS");
        assert!(t.calls().is_empty());
    }

    #[tokio::test]
    async fn an_expired_token_is_refreshed_and_the_rotated_refresh_token_is_kept() {
        let e = env();
        let id = seeded(&e, 50);
        let t = FakeTransport::new();
        script_ok(&t, "NEW-REFRESH", "Steve_01", UUID_RAW);
        let tok = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000)
            .await
            .unwrap();
        assert_eq!(tok.expose(), "MC-ACCESS");
        assert!(
            t.calls()[0].3.contains("refresh_token=OLD-REFRESH")
                && t.calls()[0].3.contains("grant_type=refresh_token")
        );
        let g = e.vault.lock().unwrap();
        let s = g.as_ref().unwrap().account(&id).unwrap();
        assert_eq!(s.ms_refresh_token.as_deref(), Some("NEW-REFRESH"));
        assert_eq!(s.minecraft_token_expires_at, Some(1000 + 86400));
    }

    #[tokio::test]
    async fn revoked_credentials_mark_the_account_sign_in_again_and_never_delete_it() {
        let e = env();
        let id = seeded(&e, 50);
        let t = FakeTransport::new();
        t.reply(TOKEN, 400, r#"{"error":"invalid_grant"}"#);
        let r = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await;
        assert!(matches!(
            r,
            Err(AuthError::NeedsSignIn(ReauthReason::Revoked))
        ));
        let file = AccountsFile::load(e.accounts.path()).unwrap();
        assert_eq!(file.accounts.len(), 1, "the account stays");
        assert!(matches!(
            file.accounts[0].status,
            AccountStatus::NeedsReauth {
                reason: ReauthReason::Revoked,
                since: 1000
            }
        ));
        assert!(
            e.vault
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .account(&id)
                .is_some(),
            "its secrets stay too"
        );
    }

    #[tokio::test]
    async fn an_outage_never_changes_an_accounts_state() {
        let e = env();
        let id = seeded(&e, 50);
        let t = FakeTransport::new();
        t.offline(TOKEN);
        assert!(matches!(
            ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await,
            Err(AuthError::Offline)
        ));
        t.reply(TOKEN, 503, "busy");
        assert!(matches!(
            ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await,
            Err(AuthError::Offline)
        ));
        assert_eq!(
            AccountsFile::load(e.accounts.path()).unwrap().accounts[0].status,
            AccountStatus::Ready
        );
    }

    #[tokio::test]
    async fn a_rotated_refresh_token_survives_a_later_chain_failure() {
        let e = env();
        let id = seeded(&e, 50); // the cached Minecraft token is expired at `now`
        let stored = |e: &Env| {
            e.vault
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .account(&id)
                .and_then(|a| a.ms_refresh_token.clone())
        };
        let old = stored(&e);
        assert!(old.is_some());
        for (failing_step, status) in [(XBL, 500u16), (XSTS, 401), (LOGIN, 503)] {
            let t = FakeTransport::new();
            t.reply(
                TOKEN,
                200,
                r#"{"access_token":"MS-ACCESS","refresh_token":"ROTATED-REFRESH","expires_in":3600}"#,
            );
            // the steps before the failing one succeed
            if failing_step != XBL {
                t.reply(
                    XBL,
                    200,
                    r#"{"Token":"XBL","DisplayClaims":{"xui":[{"uhs":"99"}]}}"#,
                );
            }
            if failing_step == LOGIN {
                t.reply(
                    XSTS,
                    200,
                    r#"{"Token":"XSTS","DisplayClaims":{"xui":[{"uhs":"99"}]}}"#,
                );
            }
            t.reply(failing_step, status, "{}");
            assert!(ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000)
                .await
                .is_err());
            assert_eq!(
                stored(&e).as_deref(),
                Some("ROTATED-REFRESH"),
                "the rotated token was kept after {failing_step} failed"
            );
            // put the old one back for the next round
            e.vault
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .set_account(
                    &id,
                    AccountSecrets {
                        ms_refresh_token: old.clone(),
                        minecraft_access_token: Some("expired".into()),
                        minecraft_token_expires_at: Some(50),
                    },
                )
                .unwrap();
        }
        // and the tokens never reach the account list
        let list = std::fs::read_to_string(e.accounts.path()).unwrap();
        assert!(!list.contains("ROTATED") && !list.to_lowercase().contains("token"));
    }

    #[tokio::test]
    async fn an_account_without_secrets_asks_to_sign_in_again_and_a_locked_vault_is_reported() {
        let e = env();
        let id = seeded(&e, 50);
        e.vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .remove_account(&id)
            .unwrap();
        let t = FakeTransport::new();
        assert!(matches!(
            ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await,
            Err(AuthError::NeedsSignIn(_))
        ));
        assert_eq!(
            AccountsFile::load(e.accounts.path())
                .unwrap()
                .accounts
                .len(),
            1
        );
        e.vault.lock().unwrap().take();
        assert!(matches!(
            ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await,
            Err(AuthError::VaultLocked)
        ));
    }

    #[test]
    fn accounts_are_independent_from_instances_in_every_stored_field() {
        let e = env();
        persist(
            e.vault.lock().unwrap().as_mut().unwrap(),
            &e.accounts,
            &signed("R", "Steve", UUID),
            None,
            1,
        )
        .unwrap();
        let json = std::fs::read_to_string(e.accounts.path())
            .unwrap()
            .to_lowercase();
        assert!(!json.contains("instance") && !json.contains("profile_id"));
    }

    #[test]
    fn signing_out_removes_the_secrets_from_the_vault_too() {
        let e = env();
        let id = persist(
            e.vault.lock().unwrap().as_mut().unwrap(),
            &e.accounts,
            &signed("R", "Steve", UUID),
            None,
            1,
        )
        .unwrap();
        let auth = AuthState::new(
            e.dir.path().join("vault.bin"),
            Err("no network in tests".into()),
        );
        *auth.vault.lock().unwrap() = e.vault.lock().unwrap().take();
        assert!(auth
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .account(&id)
            .is_some());
        auth.forget(&id);
        assert!(auth
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .account(&id)
            .is_none());
        // And it is gone from the file on disk, not just from memory.
        auth.vault.lock().unwrap().take();
        let v = Vault::unlock(&e.dir.path().join("vault.bin"), "correct horse battery").unwrap();
        assert_eq!(v.account_ids().count(), 0);
    }

    #[test]
    fn the_sign_in_error_sent_to_the_ui_has_a_code_and_a_plain_message() {
        let e: SignInError = AuthError::ApprovalRequired.into();
        assert_eq!(e.code, "approval_required");
        assert!(e.message.contains("Mojang has not approved"));
        let json = serde_json::to_string(&SignInError::from(AuthError::NoJavaProfile {
            owns_minecraft: false,
        }))
        .unwrap();
        assert!(json.contains("no_java_profile") && json.contains("does not own"));
    }

    /// A transport that, at the first request, swaps what is in the vault slot: what happens to a
    /// refresh that is in flight while "Start fresh" archives the vault and a new one is created.
    struct SwapsTheVault<'a> {
        inner: FakeTransport,
        slot: &'a Mutex<Option<Vault>>,
        next: Mutex<Option<Option<Vault>>>,
    }

    impl Transport for SwapsTheVault<'_> {
        fn send(
            &self,
            request: crate::net::HttpRequest,
        ) -> impl std::future::Future<
            Output = Result<crate::net::HttpResponse, crate::net::FetchError>,
        > + Send {
            if let Some(next) = lock(&self.next).take() {
                *lock(self.slot) = next;
            }
            self.inner.send(request)
        }
    }

    #[tokio::test]
    async fn a_refresh_that_started_before_start_fresh_cannot_write_into_the_new_vault() {
        let e = env();
        let id = seeded(&e, 50); // expired: a refresh will run
        let accounts_before = AccountsFile::load(e.accounts.path()).unwrap();
        let fresh = Vault::create_with(&e.dir.path().join("fresh.bin"), "another passphrase", FAST)
            .unwrap();
        let t = SwapsTheVault {
            inner: FakeTransport::new(),
            slot: &e.vault,
            next: Mutex::new(Some(Some(fresh))),
        };
        script_ok(&t.inner, "NEW-REFRESH", "Steve_01", UUID_RAW);
        let r = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await;
        assert!(matches!(r, Err(AuthError::VaultLocked)), "{r:?}");
        // nothing of the in-flight refresh reached the new vault
        let g = lock(&e.vault);
        assert!(g.as_ref().unwrap().account(&id).is_none());
        // and the account list is exactly as it was
        assert_eq!(
            AccountsFile::load(e.accounts.path()).unwrap(),
            accounts_before
        );
    }

    #[tokio::test]
    async fn a_refresh_whose_vault_was_locked_meanwhile_writes_nothing() {
        let e = env();
        let id = seeded(&e, 50);
        let t = SwapsTheVault {
            inner: FakeTransport::new(),
            slot: &e.vault,
            next: Mutex::new(Some(None)), // locked while the request is out
        };
        script_ok(&t.inner, "NEW-REFRESH", "Steve_01", UUID_RAW);
        let r = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await;
        assert!(matches!(r, Err(AuthError::VaultLocked)), "{r:?}");
        assert!(lock(&e.vault).is_none());
    }

    #[test]
    fn unlocking_a_vault_file_that_is_not_there_has_its_own_code() {
        let e = env();
        let missing = e.dir.path().join("not-there.bin");
        let err = Vault::unlock(&missing, "whatever passphrase").unwrap_err();
        assert_eq!(vault_error(&err).code, "vault_missing");
    }
}
