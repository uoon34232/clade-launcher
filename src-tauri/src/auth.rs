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

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

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
/// Longest lifetime accepted for a cached or newly issued Minecraft access token (24 hours).
const MAX_TOKEN_LIFETIME_SECS: u64 = 86_400;
/// Clock-skew allowance above [`MAX_TOKEN_LIFETIME_SECS`] before a cached expiry is treated as bogus.
const CLOCK_SKEW_MARGIN_SECS: i64 = 14_400;
const MAX_CACHED_TTL_SECS: i64 = (MAX_TOKEN_LIFETIME_SECS as i64) + CLOCK_SKEW_MARGIN_SECS;
/// Per-connection deadline to read the HTTP callback headers (and send the reply) on loopback.
const CALLBACK_CONN_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum simultaneous loopback connections serviced at once.
const MAX_LOOPBACK_CONNS: usize = 16;
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
    let Some((header_block, _)) = head.split_once("\r\n\r\n") else {
        return Callback::Ignore;
    };
    let mut lines = header_block.split("\r\n");
    let request = lines.next().unwrap_or("");
    let mut parts = request.split(' ');
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Ignore;
    };
    if !target.starts_with('/') || target.starts_with("//") {
        return Callback::Ignore;
    }
    let mut host_count = 0usize;
    let mut host_ok = false;
    for l in lines {
        let lower = l.to_ascii_lowercase();
        if let Some(h) = lower.strip_prefix("host:") {
            host_count += 1;
            let h = h.trim();
            host_ok = h == format!("localhost:{port}")
                || h == format!("127.0.0.1:{port}")
                || h == format!("[::1]:{port}");
        }
    }
    if host_count != 1 || !host_ok {
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
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{page}",
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
            return None;
        }
        let room = 8192 - buf.len();
        buf.extend_from_slice(&chunk[..n.min(room)]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&buf).into_owned());
        }
    }
    None
}

fn set_exclusive_addr_use(sock: &socket2::Socket) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, SOCKET, SOL_SOCKET, SO_EXCLUSIVEADDRUSE,
    };
    let on: i32 = 1;
    // SAFETY: `sock` is an open Winsock handle and `on` is a valid 4-byte integer for the call.
    let rc = unsafe {
        setsockopt(
            sock.as_raw_socket() as SOCKET,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            std::ptr::from_ref(&on).cast(),
            std::mem::size_of_val(&on) as i32,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn bind_v4_exclusive(port: u16) -> std::io::Result<TcpListener> {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    set_exclusive_addr_use(&sock)?;
    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    sock.bind(&addr.into())?;
    sock.listen(128)?;
    sock.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = sock.into();
    TcpListener::from_std(std_listener)
}

fn bind_v6_exclusive(port: u16) -> std::io::Result<TcpListener> {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    sock.set_only_v6(true)?;
    set_exclusive_addr_use(&sock)?;
    let addr = std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port));
    sock.bind(&addr.into())?;
    sock.listen(128)?;
    sock.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = sock.into();
    TcpListener::from_std(std_listener)
}

fn ipv6_unavailable(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
    ) || matches!(e.raw_os_error(), Some(10043 | 10047 | 10049))
}

fn is_transient_accept(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
    )
}

async fn accept_opt(
    listener: Option<&TcpListener>,
) -> std::io::Result<(TcpStream, std::net::SocketAddr)> {
    match listener {
        Some(l) => l.accept().await,
        None => std::future::pending().await,
    }
}

fn spawn_callback_conn(
    tasks: &mut tokio::task::JoinSet<()>,
    mut stream: TcpStream,
    port: u16,
    expected: Arc<str>,
    tx: tokio::sync::mpsc::Sender<Result<Secret, AuthError>>,
) {
    tasks.spawn(async move {
        let Ok(Some(head)) = timeout(CALLBACK_CONN_TIMEOUT, read_head(&mut stream)).await else {
            return;
        };
        match parse_callback(&head, port, &expected) {
            Callback::Code(code) => {
                let _ = timeout(
                    CALLBACK_CONN_TIMEOUT,
                    respond(&mut stream, "200 OK", PAGE_DONE),
                )
                .await;
                let _ = tx.send(Ok(code)).await;
            }
            Callback::Denied(error) => {
                let _ = timeout(
                    CALLBACK_CONN_TIMEOUT,
                    respond(&mut stream, "200 OK", PAGE_STOPPED),
                )
                .await;
                let err = if error == "access_denied" {
                    AuthError::Cancelled
                } else {
                    AuthError::OAuth(error.chars().take(40).collect())
                };
                let _ = tx.send(Err(err)).await;
            }
            Callback::Ignore => {
                let _ = timeout(
                    CALLBACK_CONN_TIMEOUT,
                    respond(&mut stream, "404 Not Found", "Not found"),
                )
                .await;
            }
        }
    });
}

fn bind_loopback_with(
    mut next_v4: impl FnMut() -> std::io::Result<TcpListener>,
) -> Result<Loopback, AuthError> {
    let mut rejected = Vec::new();
    for _ in 0..8 {
        let v4 = next_v4().map_err(|_| AuthError::BrowserUnavailable)?;
        let port = v4
            .local_addr()
            .map_err(|_| AuthError::BrowserUnavailable)?
            .port();
        match bind_v6_exclusive(port) {
            Ok(v6) => {
                return Ok(Loopback {
                    v4,
                    v6: Some(v6),
                    port,
                })
            }
            Err(e) if ipv6_unavailable(&e) => return Ok(Loopback { v4, v6: None, port }),
            Err(_) => {
                // Keep `v4` held until the loop finishes so the OS picks a distinct port next.
                rejected.push(v4);
            }
        }
    }
    Err(AuthError::BrowserUnavailable)
}

/// The one-shot listener for the browser's redirect (`127.0.0.1` and `[::1]` on the same port).
pub struct Loopback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    port: u16,
}

impl Loopback {
    pub async fn bind() -> Result<Self, AuthError> {
        bind_loopback_with(|| bind_v4_exclusive(0))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://localhost:<port>`. Microsoft matches the registered `http://localhost` on any port.
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    pub async fn wait_for_code(&self, state: &str, wait: Duration) -> Result<Secret, AuthError> {
        let deadline = tokio::time::Instant::now() + wait;
        let expected: Arc<str> = Arc::from(state);
        let port = self.port;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Secret, AuthError>>(1);
        let mut tasks = tokio::task::JoinSet::new();
        let mut v6_alive = self.v6.is_some();

        loop {
            let accept_enabled = tasks.len() < MAX_LOOPBACK_CONNS;
            tokio::select! {
                biased;
                Some(outcome) = rx.recv() => return outcome,
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                () = tokio::time::sleep_until(deadline) => return Err(AuthError::TimedOut),
                res = self.v4.accept(), if accept_enabled => match res {
                    Ok((stream, _)) => {
                        spawn_callback_conn(&mut tasks, stream, port, expected.clone(), tx.clone());
                    }
                    Err(e) if is_transient_accept(&e) => {}
                    Err(_) => return Err(AuthError::BrowserUnavailable),
                },
                res = accept_opt(self.v6.as_ref()), if accept_enabled && v6_alive => match res {
                    Ok((stream, _)) => {
                        spawn_callback_conn(&mut tasks, stream, port, expected.clone(), tx.clone());
                    }
                    Err(e) if is_transient_accept(&e) => {}
                    Err(_) => {
                        v6_alive = false;
                    }
                },
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
    sign_in_with_open(
        t,
        loopback,
        move |url| std::future::ready(open(&url)),
        progress,
        cancel,
        wait,
    )
    .await
}

async fn sign_in_with_open<T: Transport, F, Fut>(
    t: &T,
    loopback: &Loopback,
    open: F,
    progress: impl Fn(Stage),
    cancel: &Notify,
    wait: Duration,
) -> Result<SignedIn, AuthError>
where
    F: FnOnce(Url) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let pkce = Pkce::generate()?;
    let state = random_token()?;
    let redirect = loopback.redirect_uri();
    let url = authorize_url(&redirect, &state, &pkce.challenge);

    progress(Stage::Opening);
    if !open(url).await {
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
    let ttl = i64::try_from(session.expires_in.min(MAX_TOKEN_LIFETIME_SECS)).unwrap_or(3600);
    AccountSecrets {
        ms_refresh_token: Some(refresh.expose().to_owned()),
        minecraft_access_token: Some(session.access_token.expose().to_owned()),
        minecraft_token_expires_at: Some(now.saturating_add(ttl)),
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

struct RefreshSlot {
    mutex: tokio::sync::Mutex<()>,
    revision: AtomicU64,
}

type RefreshLockKey = (usize, String);
type RefreshLockMap = HashMap<RefreshLockKey, Weak<RefreshSlot>>;

static REFRESH_LOCKS: OnceLock<Mutex<RefreshLockMap>> = OnceLock::new();

fn refresh_lock_for(vault: &Mutex<Option<Vault>>, id: &str) -> Arc<RefreshSlot> {
    let map = REFRESH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = lock(map);
    let key = (std::ptr::from_ref(vault) as usize, id.to_owned());
    if let Some(existing) = guard.get(&key).and_then(Weak::upgrade) {
        return existing;
    }
    if guard.len() > 64 {
        guard.retain(|_, v| v.strong_count() > 0);
    }
    let created = Arc::new(RefreshSlot {
        mutex: tokio::sync::Mutex::new(()),
        revision: AtomicU64::new(0),
    });
    guard.insert(key, Arc::downgrade(&created));
    created
}

fn bump_account_revision(vault: &Mutex<Option<Vault>>, id: &str) {
    let Some(map) = REFRESH_LOCKS.get() else {
        return;
    };
    let mut guard = lock(map);
    let key = (std::ptr::from_ref(vault) as usize, id.to_owned());
    if let Some(slot) = guard.get(&key).and_then(Weak::upgrade) {
        slot.revision.fetch_add(1, Ordering::SeqCst);
    }
    if guard.len() > 64 {
        guard.retain(|_, v| v.strong_count() > 0);
    }
}

fn same_secrets(a: &AccountSecrets, b: &AccountSecrets) -> bool {
    a.ms_refresh_token == b.ms_refresh_token
        && a.minecraft_access_token == b.minecraft_access_token
        && a.minecraft_token_expires_at == b.minecraft_token_expires_at
}

fn was_superseded(
    v: &Vault,
    id: &str,
    generation: [u8; 16],
    acct_slot: &RefreshSlot,
    start_rev: u64,
    expected_secrets: &AccountSecrets,
) -> bool {
    v.generation() == generation
        && (acct_slot.revision.load(Ordering::SeqCst) != start_rev
            || v.account(id)
                .is_some_and(|cur| !same_secrets(cur, expected_secrets)))
}

fn check_superseded_locked(
    vault: &Mutex<Option<Vault>>,
    id: &str,
    generation: [u8; 16],
    acct_slot: &RefreshSlot,
    start_rev: u64,
    expected_secrets: &AccountSecrets,
) -> bool {
    let guard = lock(vault);
    guard
        .as_ref()
        .is_some_and(|v| was_superseded(v, id, generation, acct_slot, start_rev, expected_secrets))
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
    ensure_fresh_inner(t, vault, accounts, id, now, false).await
}

/// Forces a credential refresh even when the cached Minecraft access token has not expired yet
/// (used by explicit account refresh and single-shot `401` recovery).
pub async fn ensure_fresh_forced<T: Transport>(
    t: &T,
    vault: &Mutex<Option<Vault>>,
    accounts: &AccountsState,
    id: &str,
    now: i64,
) -> Result<Secret, AuthError> {
    ensure_fresh_inner(t, vault, accounts, id, now, true).await
}

async fn ensure_fresh_inner<T: Transport>(
    t: &T,
    vault: &Mutex<Option<Vault>>,
    accounts: &AccountsState,
    id: &str,
    now: i64,
    mut force: bool,
) -> Result<Secret, AuthError> {
    const MAX_SIGN_IN_RETRIES: usize = 3;
    let acct_slot = refresh_lock_for(vault, id);
    let _acct_guard = acct_slot.mutex.lock().await;

    for attempt in 0..=MAX_SIGN_IN_RETRIES {
        // Which vault and credential revision this refresh belongs to. The network steps below take
        // a while; if the vault is replaced ("Start fresh") or a new sign-in updates this account's
        // credentials meanwhile, the stale refresh must never overwrite them or revert status.
        let (mut expected_secrets, generation, start_rev, expected_uuid, old_refresh) = {
            let guard = lock(vault);
            let v = guard.as_ref().ok_or(AuthError::VaultLocked)?;
            let start_rev = acct_slot.revision.load(Ordering::SeqCst);
            let expected_uuid = accounts
                .load()
                .map_err(storage)?
                .accounts
                .into_iter()
                .find(|a| a.id == id)
                .ok_or(AuthError::UnknownAccount)?
                .minecraft_uuid;
            let Some(secrets) = v.account(id).cloned() else {
                let _ = accounts.update(|f| f.mark_needs_reauth(id, ReauthReason::Unknown, now));
                return Err(AuthError::NeedsSignIn(ReauthReason::Unknown));
            };
            if !force {
                if let (Some(token), Some(expires)) = (
                    &secrets.minecraft_access_token,
                    secrets.minecraft_token_expires_at,
                ) {
                    if expires > now.saturating_add(TOKEN_MARGIN_SECS)
                        && expires <= now.saturating_add(MAX_CACHED_TTL_SECS)
                    {
                        return Ok(Secret::new(token.clone()));
                    }
                }
            }
            let Some(old_refresh) = secrets
                .ms_refresh_token
                .as_ref()
                .map(|s| Secret::new(s.clone()))
            else {
                let _ = accounts.update(|f| f.mark_needs_reauth(id, ReauthReason::Unknown, now));
                return Err(AuthError::NeedsSignIn(ReauthReason::Unknown));
            };
            (
                secrets,
                v.generation(),
                start_rev,
                expected_uuid,
                old_refresh,
            )
        };

        let tokens = match refresh_ms(t, &old_refresh).await {
            Ok(tokens) => tokens,
            Err(AuthError::NeedsSignIn(reason)) => {
                let superseded = {
                    let guard = lock(vault);
                    let v = guard.as_ref().ok_or(AuthError::VaultLocked)?;
                    if v.generation() != generation {
                        return Err(AuthError::VaultLocked);
                    }
                    if was_superseded(v, id, generation, &acct_slot, start_rev, &expected_secrets) {
                        true
                    } else {
                        let in_accounts =
                            accounts.load().map_err(storage)?.accounts.iter().any(|a| {
                                a.id == id && same_uuid(&a.minecraft_uuid, &expected_uuid)
                            });
                        if !in_accounts {
                            return Err(AuthError::UnknownAccount);
                        }
                        let _ = accounts.update(|f| f.mark_needs_reauth(id, reason, now));
                        return Err(AuthError::NeedsSignIn(reason));
                    }
                };
                if superseded {
                    if attempt < MAX_SIGN_IN_RETRIES {
                        force = false;
                        continue;
                    }
                    return Err(AuthError::Storage(
                        "account credentials changed concurrently".into(),
                    ));
                }
                return Err(AuthError::NeedsSignIn(reason));
            }
            Err(e) => {
                if check_superseded_locked(
                    vault,
                    id,
                    generation,
                    &acct_slot,
                    start_rev,
                    &expected_secrets,
                ) {
                    if attempt < MAX_SIGN_IN_RETRIES {
                        force = false;
                        continue;
                    }
                    return Err(AuthError::Storage(
                        "account credentials changed concurrently".into(),
                    ));
                }
                return Err(e);
            }
        };

        // Microsoft rotates refresh tokens, and the old one may stop working once it has been used. So
        // the new one is saved to the vault right now, before the Xbox and Minecraft steps: if one of
        // them fails (an outage, a rate limit), the account still holds a refresh token that works. Only
        // the refresh token is written here, and only if the account was not removed or superseded meanwhile.
        if let Some(new_refresh) = &tokens.refresh {
            let superseded =
                {
                    let mut guard = lock(vault);
                    let v = guard.as_mut().ok_or(AuthError::VaultLocked)?;
                    if v.generation() != generation {
                        return Err(AuthError::VaultLocked);
                    }
                    let Some(cur) = v.account(id) else {
                        return Err(AuthError::UnknownAccount);
                    };
                    if was_superseded(v, id, generation, &acct_slot, start_rev, &expected_secrets) {
                        true
                    } else {
                        let _ = cur;
                        let still_present =
                            accounts.load().map_err(storage)?.accounts.iter().any(|a| {
                                a.id == id && same_uuid(&a.minecraft_uuid, &expected_uuid)
                            });
                        if !still_present {
                            return Err(AuthError::UnknownAccount);
                        }
                        let mut updated = expected_secrets.clone();
                        updated.ms_refresh_token = Some(new_refresh.expose().to_owned());
                        v.set_account(id, updated.clone()).map_err(storage)?;
                        expected_secrets = updated;
                        false
                    }
                };
            if superseded {
                if attempt < MAX_SIGN_IN_RETRIES {
                    force = false;
                    continue;
                }
                return Err(AuthError::Storage(
                    "account credentials changed concurrently".into(),
                ));
            }
        }

        let (session, profile) = match complete_chain(t, &tokens.access).await {
            Ok(res) => res,
            Err(e) => {
                if check_superseded_locked(
                    vault,
                    id,
                    generation,
                    &acct_slot,
                    start_rev,
                    &expected_secrets,
                ) {
                    if attempt < MAX_SIGN_IN_RETRIES {
                        force = false;
                        continue;
                    }
                    return Err(AuthError::Storage(
                        "account credentials changed concurrently".into(),
                    ));
                }
                return Err(e);
            }
        };
        if !same_uuid(&expected_uuid, &profile.uuid) {
            if check_superseded_locked(
                vault,
                id,
                generation,
                &acct_slot,
                start_rev,
                &expected_secrets,
            ) {
                if attempt < MAX_SIGN_IN_RETRIES {
                    force = false;
                    continue;
                }
                return Err(AuthError::Storage(
                    "account credentials changed concurrently".into(),
                ));
            }
            return Err(AuthError::WrongAccount);
        }

        // The refresh token to keep: the new one, or the old one if none came back.
        let refresh = tokens.refresh.unwrap_or(old_refresh);
        let superseded = {
            let mut guard = lock(vault);
            let v = guard.as_mut().ok_or(AuthError::VaultLocked)?;
            if v.generation() != generation {
                return Err(AuthError::VaultLocked); // the vault changed: nothing of this refresh is kept
            }
            if v.account(id).is_none() {
                return Err(AuthError::UnknownAccount);
            }
            if was_superseded(v, id, generation, &acct_slot, start_rev, &expected_secrets) {
                true
            } else {
                let current = accounts.load().map_err(storage)?;
                match current.accounts.iter().find(|a| a.id == id) {
                    Some(a) if same_uuid(&a.minecraft_uuid, &profile.uuid) => {}
                    Some(_) => return Err(AuthError::WrongAccount),
                    None => return Err(AuthError::UnknownAccount),
                }
                v.set_account(id, secrets_for(&refresh, &session, now))
                    .map_err(storage)?;
                accounts.update(|f| match f.accounts.iter_mut().find(|a| a.id == id) {
                    Some(a) if same_uuid(&a.minecraft_uuid, &profile.uuid) => {
                        a.username = profile.name.clone();
                        a.status = AccountStatus::Ready;
                        Ok(())
                    }
                    Some(_) => Err(AuthError::WrongAccount),
                    None => Err(AuthError::UnknownAccount),
                })??;
                false
            }
        };
        if superseded {
            if attempt < MAX_SIGN_IN_RETRIES {
                force = false;
                continue;
            }
            return Err(AuthError::Storage(
                "account credentials changed concurrently".into(),
            ));
        }

        // The profile just reported the current skin: note it (the download happens off this path).
        accounts.record_skin(&profile.uuid, profile.skin_url.as_deref());
        return Ok(session.access_token);
    }

    Err(AuthError::Storage(
        "account credentials changed concurrently".into(),
    ))
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------------------ commands

/// Shared sign-in state: the (possibly locked) vault and the sign-in in progress, if any.
pub struct AuthState {
    pub vault_path: std::path::PathBuf,
    pub vault: Mutex<Option<Vault>>,
    pub(crate) kdf_gate: tokio::sync::Mutex<()>,
    pub(crate) running: Mutex<Option<Arc<Notify>>>,
    pub(crate) sign_in_cancelled: AtomicBool,
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
            kdf_gate: tokio::sync::Mutex::new(()),
            running: Mutex::new(None),
            sign_in_cancelled: AtomicBool::new(false),
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
        if slot.is_some() {
            return Err(CodedError::new("vault_exists", "a vault is already open"));
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
        VaultError::PassphraseTooLong => CodedError::new("passphrase_too_long", e.to_string()),
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

fn finish_create_vault(auth: &AuthState, vault: Vault, started: u64) -> Result<(), CodedError> {
    let mut slot = lock(&auth.vault);
    if auth.epoch.load(Ordering::SeqCst) != started {
        return Err(CodedError::new(
            "vault_missing",
            "the vault was archived while it was being created",
        ));
    }
    if slot.is_some() {
        return Err(CodedError::new("vault_exists", "a vault already exists"));
    }
    vault.persist_new().map_err(|e| vault_error(&e))?;
    *slot = Some(vault);
    Ok(())
}

pub(crate) async fn create_vault_with(
    auth: &AuthState,
    passphrase: String,
) -> Result<(), CodedError> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    let _kdf = auth.kdf_gate.lock().await;
    let started = {
        let slot = lock(&auth.vault);
        if slot.is_some() {
            return Err(CodedError::new("vault_exists", "a vault already exists"));
        }
        auth.epoch.load(Ordering::SeqCst)
    };
    let path = auth.vault_path.clone();
    let vault = tokio::task::spawn_blocking(move || Vault::derive_empty(&path, &passphrase))
        .await
        .map_err(|_| CodedError::new("vault_unreadable", "the vault task failed"))?
        .map_err(|e| vault_error(&e))?;
    finish_create_vault(auth, vault, started)
}

#[tauri::command]
pub async fn create_vault(
    auth: State<'_, AuthState>,
    passphrase: String,
) -> Result<(), CodedError> {
    create_vault_with(&auth, passphrase).await
}

fn purge_orphans_against(vault: &mut Vault, file: &crate::accounts::AccountsFile) -> usize {
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

/// Drops the vault entries of accounts that were removed while the vault was locked.
///
/// This deletes credentials, so it only runs against an account list that is known to be
/// authoritative: `accounts.json` must exist and parse. A missing file (a copied `vault.bin`, a
/// restored backup, a deleted list) or a damaged one proves nothing about which accounts are gone,
/// so the vault is left exactly as it is. A file that parses and really lists no accounts is a real
/// empty list. Returns how many entries were removed.
#[cfg(test)]
pub(crate) fn purge_orphans(vault: &mut Vault, accounts_path: &Path) -> usize {
    match crate::accounts::AccountsFile::load_authoritative(accounts_path) {
        Ok(Some(file)) => purge_orphans_against(vault, &file),
        Ok(None) => {
            tracing::info!("accounts.json is missing; the vault was left untouched");
            0
        }
        Err(e) => {
            tracing::warn!(error = %e, "accounts.json could not be read; the vault was left untouched");
            0
        }
    }
}

fn purge_orphans_locked(vault: &mut Vault, accounts: &AccountsState) -> usize {
    match accounts.load_authoritative() {
        Ok(Some(file)) => purge_orphans_against(vault, &file),
        Ok(None) => {
            tracing::info!("accounts.json is missing; the vault was left untouched");
            0
        }
        Err(e) => {
            tracing::warn!(error = %e, "accounts.json could not be read; the vault was left untouched");
            0
        }
    }
}

pub(crate) async fn unlock_vault_with(
    auth: &AuthState,
    accounts: &AccountsState,
    passphrase: String,
) -> Result<(), CodedError> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    let _kdf = auth.kdf_gate.lock().await;
    let started = {
        let slot = lock(&auth.vault);
        if slot.is_some() {
            return Ok(());
        }
        auth.epoch.load(Ordering::SeqCst)
    };
    let path = auth.vault_path.clone();
    let vault = tokio::task::spawn_blocking(move || Vault::unlock(&path, &passphrase))
        .await
        .map_err(|_| CodedError::new("vault_unreadable", "the vault task failed"))?
        .map_err(|e| vault_error(&e))?;
    auth.install_vault(vault, started, |v| {
        purge_orphans_locked(v, accounts);
    })
}

#[tauri::command]
pub async fn unlock_vault(
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    passphrase: String,
) -> Result<(), CodedError> {
    unlock_vault_with(&auth, &accounts, passphrase).await
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

fn persist_signed_in(
    auth: &AuthState,
    accounts: &AccountsState,
    signed: &SignedIn,
    reauth_for: Option<&str>,
    now: i64,
) -> Result<String, AuthError> {
    let mut guard = lock(&auth.vault);
    if auth.sign_in_cancelled.load(Ordering::SeqCst) {
        return Err(AuthError::Cancelled);
    }
    let vault = guard.as_mut().ok_or(AuthError::VaultLocked)?;
    let id = persist(vault, accounts, signed, reauth_for, now)?;
    bump_account_revision(&auth.vault, &id);
    Ok(id)
}

pub(crate) fn cancel_sign_in_with(auth: &AuthState) {
    let running = lock(&auth.running);
    auth.sign_in_cancelled.store(true, Ordering::SeqCst);
    if let Some(n) = running.as_ref() {
        n.notify_one();
    }
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
        auth.sign_in_cancelled.store(false, Ordering::SeqCst);
        *running = Some(cancel.clone());
    }
    let _guard = RunGuard(&auth.running);

    let t = transport(auth)?;
    let loopback = Loopback::bind().await?;
    let app_open = app.clone();
    let signed = sign_in_with_open(
        t,
        &loopback,
        |url| async move {
            tokio::task::spawn_blocking(move || {
                app_open
                    .opener()
                    .open_url(url.as_str(), None::<&str>)
                    .is_ok()
            })
            .await
            .unwrap_or(false)
        },
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

    persist_signed_in(auth, accounts, &signed, reauth_for.as_deref(), now_unix())?;
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
    cancel_sign_in_with(&auth);
}

/// Refreshes one account's credentials now (forcing a fresh token exchange).
#[tauri::command]
pub async fn refresh_account(
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    id: String,
) -> Result<AccountsDto, SignInError> {
    let result = async {
        let t = transport(&auth)?;
        ensure_fresh_forced(t, &auth.vault, &accounts, &id, now_unix()).await?;
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
            "GET @evil.example/?code=abc&state=S HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
            "GET //evil.example/?code=abc&state=S HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
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

    #[tokio::test]
    async fn callback_requires_terminated_headers_accepts_ipv6_host_and_sets_security_headers() {
        let v6_ok = "GET /?code=abc&state=S HTTP/1.1\r\nHost: [::1]:5000\r\n\r\n";
        assert!(
            matches!(parse_callback(v6_ok, 5000, "S"), Callback::Code(c) if c.expose() == "abc")
        );
        let upper_ok = "GET /?code=abc&state=S HTTP/1.1\r\nHOST: LOCALHOST:5000\r\n\r\n";
        assert!(matches!(
            parse_callback(upper_ok, 5000, "S"),
            Callback::Code(_)
        ));
        for bad in [
            "GET /?code=abc&state=S HTTP/1.1\r\nHost: localhost:5000\r\n",
            "GET /?code=abc&state=S HTTP/1.1\r\nHost: localhost:5000",
            "GET /?code=abc&state=S HTTP/1.1\r\nHost: evil.example:5000\r\nHost: localhost:5000\r\n\r\n",
            "GET /?code=abc&state=S HTTP/1.1\r\nX-Pad: 1\r\n\r\nHost: localhost:5000\r\n\r\n",
        ] {
            assert!(
                matches!(parse_callback(bad, 5000, "S"), Callback::Ignore),
                "{bad}"
            );
        }

        let lb = Loopback::bind().await.unwrap();
        let port = lb.port();
        let reply_handle = tokio::spawn(async move {
            // 1. Truncated stream closed before `\r\n\r\n` must be ignored
            let mut half = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let partial = format!("GET /?code=BAD&state=S HTTP/1.1\r\nHost: localhost:{port}\r\n");
            half.write_all(partial.as_bytes()).await.unwrap();
            let _ = half.shutdown().await;
            drop(half);

            // 2. Real callback receives hardened response headers
            let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(
                format!("GET /?code=GOOD&state=S HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
            let mut raw = Vec::new();
            s.read_to_end(&mut raw).await.unwrap();
            String::from_utf8_lossy(&raw).into_owned()
        });
        let code = lb.wait_for_code("S", Duration::from_secs(5)).await.unwrap();
        assert_eq!(code.expose(), "GOOD");
        let resp = reply_handle.await.unwrap();
        let lower = resp.to_ascii_lowercase();
        assert!(lower.contains("x-content-type-options: nosniff"), "{resp}");
        assert!(lower.contains("frame-ancestors 'none'"), "{resp}");
    }

    #[tokio::test]
    async fn loopback_handles_idle_preconnects_ipv6_and_port_collisions() {
        {
            let v4 = bind_v4_exclusive(0).unwrap();
            let p = v4.local_addr().unwrap().port();
            let hijack = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )
            .unwrap();
            let _ = hijack.set_reuse_address(true);
            let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, p));
            assert!(
                hijack.bind(&addr.into()).is_err(),
                "SO_EXCLUSIVEADDRUSE must block SO_REUSEADDR hijacking"
            );
        }

        // If IPv6 loopback is supported on this host, occupying `[::1]:occupied` forces a retry.
        if let Ok(occupied_v6) = bind_v6_exclusive(0) {
            let occupied_port = occupied_v6.local_addr().unwrap().port();
            let mut first = true;
            let lb = bind_loopback_with(|| {
                if first {
                    first = false;
                    bind_v4_exclusive(occupied_port)
                } else {
                    bind_v4_exclusive(0)
                }
            })
            .unwrap();
            assert_ne!(lb.port(), occupied_port);
            assert!(lb.v6.is_some());
            drop(occupied_v6);
        }

        // Idle preconnect sockets must never block a real callback on either 127.0.0.1 or [::1].
        let lb = Loopback::bind().await.unwrap();
        let port = lb.port();
        let has_v6 = lb.v6.is_some();
        let t0 = tokio::time::Instant::now();
        let _idle_v4 = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let _idle_v6 = if has_v6 {
            Some(TcpStream::connect(("::1", port)).await.unwrap())
        } else {
            None
        };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if has_v6 {
                let mut s = TcpStream::connect(("::1", port)).await.unwrap();
                s.write_all(
                    format!("GET /?code=V6-CODE&state=S HTTP/1.1\r\nHost: [::1]:{port}\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
                let mut out = Vec::new();
                let _ = s.read_to_end(&mut out).await;
            } else {
                let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                s.write_all(
                    format!(
                        "GET /?code=V6-CODE&state=S HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
                let mut out = Vec::new();
                let _ = s.read_to_end(&mut out).await;
            }
        });
        let code = lb.wait_for_code("S", Duration::from_secs(5)).await.unwrap();
        assert_eq!(code.expose(), "V6-CODE");
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "idle preconnect blocked callback: {:?}",
            t0.elapsed()
        );
    }

    #[tokio::test]
    async fn vault_creation_and_unlock_serialize_kdf_and_never_write_orphaned_vault_after_reset() {
        assert_eq!(
            vault_error(&VaultError::PassphraseTooLong).code,
            "passphrase_too_long"
        );

        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("vault.bin");
        let auth = AuthState::new(vault_path.clone(), Err("no network".into()));
        let accounts = AccountsState::new(dir.path().join("accounts.json"));

        // Epoch bump during create_vault_with (after KDF derived in memory, before persist_new)
        // must NOT write `vault.bin` to disk!
        let started = auth.epoch.load(Ordering::SeqCst);
        let derived = Vault::derive_empty_with(&vault_path, "correct horse battery", FAST).unwrap();
        assert!(!vault_path.exists(), "derive_empty writes nothing to disk");
        auth.epoch.fetch_add(1, Ordering::SeqCst);
        let err = finish_create_vault(&auth, derived, started).unwrap_err();
        assert_eq!(err.code, "vault_missing");
        assert!(
            !vault_path.exists(),
            "no orphaned vault.bin written after epoch bump"
        );
        assert!(lock(&auth.vault).is_none());

        // Real create_vault_with persists and installs cleanly, and rejects a duplicate create.
        create_vault_with(&auth, "correct horse battery".into())
            .await
            .unwrap();
        assert!(vault_path.exists());
        assert!(lock(&auth.vault).is_some());
        let dup = create_vault_with(&auth, "correct horse battery".into())
            .await
            .unwrap_err();
        assert_eq!(dup.code, "vault_exists");

        // `install_vault` also refuses to overwrite an already-open vault slot.
        let extra =
            Vault::derive_empty_with(&dir.path().join("other.bin"), "correct horse battery", FAST)
                .unwrap();
        let cur_epoch = auth.epoch.load(Ordering::SeqCst);
        assert_eq!(
            auth.install_vault(extra, cur_epoch, |_| panic!("must not run"))
                .unwrap_err()
                .code,
            "vault_exists"
        );

        // Lock the vault, add an orphan + a valid account, and unlock via `unlock_vault_with`.
        {
            let mut g = lock(&auth.vault);
            let v = g.as_mut().unwrap();
            v.set_account("kept", AccountSecrets::default()).unwrap();
            v.set_account("orphan", AccountSecrets::default()).unwrap();
        }
        *lock(&auth.vault) = None;
        let mut af = AccountsFile::default();
        af.accounts.push(AccountMeta {
            id: "kept".into(),
            username: "Steve".into(),
            minecraft_uuid: UUID.into(),
            added_at: 1,
            last_used_at: None,
            status: AccountStatus::Ready,
        });
        af.save(accounts.path()).unwrap();

        unlock_vault_with(&auth, &accounts, "correct horse battery".into())
            .await
            .unwrap();
        {
            let g = lock(&auth.vault);
            let v = g.as_ref().unwrap();
            assert!(v.account("kept").is_some());
            assert!(v.account("orphan").is_none());
        }
        // Already unlocked: returns Ok(()) immediately without re-running Argon2.
        unlock_vault_with(&auth, &accounts, "ignored while unlocked".into())
            .await
            .unwrap();
    }

    struct SlowRefreshTransport {
        inner: FakeTransport,
    }

    impl Transport for SlowRefreshTransport {
        fn send(
            &self,
            request: crate::net::HttpRequest,
        ) -> impl std::future::Future<
            Output = Result<crate::net::HttpResponse, crate::net::FetchError>,
        > + Send {
            let fut = self.inner.send(request);
            async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                fut.await
            }
        }
    }

    #[tokio::test]
    async fn token_expiry_bounds_forced_refresh_and_concurrent_single_flight_refresh() {
        let s = secrets_for(
            &Secret::new("R".into()),
            &McSession {
                access_token: Secret::new("M".into()),
                expires_in: 999_999,
            },
            1000,
        );
        assert_eq!(s.minecraft_token_expires_at, Some(1000 + 86_400));

        // 1. `now + 100_000` (within 86_400 + 14_400 skew margin) is reused without network calls
        let e = env();
        let id = seeded(&e, 1000 + 100_000);
        let t = FakeTransport::new();
        let tok = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000)
            .await
            .unwrap();
        assert_eq!(tok.expose(), "MC-ACCESS");
        assert!(t.calls().is_empty());

        // 2. Bogus far-future expiry (`now + 200_000` or `i64::MAX`) is rejected and refreshed
        for bogus in [1000 + 200_000, i64::MAX] {
            let e = env();
            let id = seeded(&e, bogus);
            let t = FakeTransport::new();
            script_ok(&t, "NEW-R", "Steve_01", UUID_RAW);
            let tok = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000)
                .await
                .unwrap();
            assert_eq!(tok.expose(), "MC-ACCESS");
            assert_eq!(
                t.calls().iter().filter(|c| c.1 == TOKEN).count(),
                1,
                "bogus expiry {bogus} must force a refresh"
            );
        }

        // 3. `ensure_fresh_forced` refreshes even when the cached token is completely fresh
        let e = env();
        let id = seeded(&e, 1000 + 3600);
        let t = FakeTransport::new();
        t.reply(
            TOKEN,
            200,
            r#"{"access_token":"MS-ACCESS","refresh_token":"FORCED-R","expires_in":3600}"#,
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
            r#"{"access_token":"FORCED-MC","expires_in":86400}"#,
        );
        t.reply(
            PROFILE,
            200,
            format!(r#"{{"id":"{UUID_RAW}","name":"Steve_01"}}"#),
        );
        let tok = ensure_fresh_forced(&t, &e.vault, &e.accounts, &id, 1000)
            .await
            .unwrap();
        assert_eq!(tok.expose(), "FORCED-MC");

        // 4. Concurrent `ensure_fresh` calls for the same account serialize so only ONE MS refresh runs
        let e = env();
        let id = seeded(&e, 50);
        let slow = SlowRefreshTransport {
            inner: FakeTransport::new(),
        };
        // Script only ONE refresh chain: if the second caller hit the network, it would fail!
        script_ok(&slow.inner, "SINGLE-FLIGHT-R", "Steve_01", UUID_RAW);
        let (r1, r2) = tokio::join!(
            ensure_fresh(&slow, &e.vault, &e.accounts, &id, 1000),
            ensure_fresh(&slow, &e.vault, &e.accounts, &id, 1000),
        );
        assert_eq!(r1.unwrap().expose(), "MC-ACCESS");
        assert_eq!(r2.unwrap().expose(), "MC-ACCESS");
        assert_eq!(
            slow.inner.calls().iter().filter(|c| c.1 == TOKEN).count(),
            1
        );
    }

    /// Removes the account while the first network request is in flight (simulating sign-out).
    struct SignsOutInFlight<'a> {
        inner: FakeTransport,
        vault: &'a Mutex<Option<Vault>>,
        accounts: &'a AccountsState,
        id: String,
        fired: AtomicBool,
    }

    impl Transport for SignsOutInFlight<'_> {
        fn send(
            &self,
            request: crate::net::HttpRequest,
        ) -> impl std::future::Future<
            Output = Result<crate::net::HttpResponse, crate::net::FetchError>,
        > + Send {
            if !self.fired.swap(true, Ordering::SeqCst) {
                if let Some(v) = lock(self.vault).as_mut() {
                    let _ = v.remove_account(&self.id);
                }
                let _ = self.accounts.update(|f| {
                    f.remove(&self.id);
                });
            }
            self.inner.send(request)
        }
    }

    #[tokio::test]
    async fn refresh_rejects_foreign_uuid_and_post_sign_out_zombies_before_touching_vault() {
        // 1. Wrong UUID from profile endpoint must NOT overwrite the vault's Minecraft access token
        let e = env();
        let id = seeded(&e, 50);
        {
            let mut g = lock(&e.vault);
            let v = g.as_mut().unwrap();
            let mut s = v.account(&id).unwrap().clone();
            s.minecraft_access_token = Some("ORIGINAL-TOKEN".into());
            v.set_account(&id, s).unwrap();
        }
        let t = FakeTransport::new();
        t.reply(
            TOKEN,
            200,
            r#"{"access_token":"MS-ACCESS","refresh_token":"ROTATED-R","expires_in":3600}"#,
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
            r#"{"access_token":"FOREIGN-TOKEN","expires_in":86400}"#,
        );
        t.reply(
            PROFILE,
            200,
            r#"{"id":"0000000000004000800000000000b002","name":"WrongPerson"}"#,
        );
        let res = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await;
        assert!(matches!(res, Err(AuthError::WrongAccount)));
        let stored_tok = lock(&e.vault)
            .as_ref()
            .unwrap()
            .account(&id)
            .unwrap()
            .minecraft_access_token
            .clone();
        assert_eq!(
            stored_tok.as_deref(),
            Some("ORIGINAL-TOKEN"),
            "foreign session token must never be saved to vault.bin"
        );

        // 2. Signing out while refresh is in flight must NOT resurrect a zombie vault entry
        let e = env();
        let id = seeded(&e, 50);
        let t = SignsOutInFlight {
            inner: FakeTransport::new(),
            vault: &e.vault,
            accounts: &e.accounts,
            id: id.clone(),
            fired: AtomicBool::new(false),
        };
        script_ok(&t.inner, "ZOMBIE-REFRESH", "Steve_01", UUID_RAW);
        let res = ensure_fresh(&t, &e.vault, &e.accounts, &id, 1000).await;
        assert!(matches!(res, Err(AuthError::UnknownAccount)));
        assert!(
            lock(&e.vault).as_ref().unwrap().account(&id).is_none(),
            "signed-out account must stay removed from vault"
        );
    }

    #[test]
    fn cancelling_sign_in_blocks_late_persistence() {
        let e = env();
        let auth = AuthState::new(e.dir.path().join("vault.bin"), Err("no network".into()));
        *lock(&auth.vault) = lock(&e.vault).take();
        *lock(&auth.running) = Some(Arc::new(Notify::new()));
        auth.sign_in_cancelled.store(false, Ordering::SeqCst);

        cancel_sign_in_with(&auth);
        assert!(auth.sign_in_cancelled.load(Ordering::SeqCst));

        let res = persist_signed_in(
            &auth,
            &e.accounts,
            &signed("R", "Steve_01", UUID),
            None,
            1000,
        );
        assert!(matches!(res, Err(AuthError::Cancelled)));
        assert!(e.accounts.load().unwrap().accounts.is_empty());
        assert_eq!(lock(&auth.vault).as_ref().unwrap().account_ids().count(), 0);
    }
}
