//! The Microsoft -> Xbox -> Minecraft Java sign-in protocol. Pure request building and response
//! parsing over the [`Transport`] trait; no files, no windows, no vault.
//!
//! Chain (verified 2026-10-04 against Microsoft's identity-platform documentation and the
//! community-maintained Minecraft Wiki; Mojang publishes no formal spec for the last steps):
//! 1. System-browser OAuth 2.0 authorization code flow with PKCE (S256) against the `consumers`
//!    tenant, public client (never a client secret), scope `XboxLive.signin offline_access`.
//! 2. `user.auth.xboxlive.com/user/authenticate` with the Microsoft access token.
//! 3. `xsts.auth.xboxlive.com/xsts/authorize` for relying party `rp://api.minecraftservices.com/`.
//! 4. `api.minecraftservices.com/authentication/login_with_xbox`. **Needs Mojang's approval of the
//!    client id; without it the service answers 403, reported as [`AuthError::ApprovalRequired`].
//!    That restriction is never worked around.**
//! 5. `GET /minecraft/profile` proves a Java profile exists (404: none); `GET
//!    /entitlements/mcstore` only explains a 404 (owns the game or not).
//!
//! Secrets live in [`Secret`] (zeroized, redacted `Debug`). Errors carry codes and plain messages,
//! never response bodies.

use std::fmt;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::accounts::{classify_oauth_error, ReauthReason};
use crate::net::{FetchError, HttpRequest, HttpResponse, Method, Transport};

/// The launcher's public client id (an Entra app registration for personal Microsoft accounts,
/// public client, redirect `http://localhost`). Not a secret; there is no client secret.
pub const CLIENT_ID: &str = "27200331-f467-4c61-a013-b307305c4ccf";
pub const SCOPE: &str = "XboxLive.signin offline_access";

const AUTHORIZE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_AUTH: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_ENTITLEMENTS: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const MC_PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";

/// Every URL this module talks to (asserted against the network policy in tests).
pub const ENDPOINTS: [&str; 7] = [
    AUTHORIZE,
    TOKEN,
    XBL_AUTH,
    XSTS_AUTH,
    MC_LOGIN,
    MC_ENTITLEMENTS,
    MC_PROFILE,
];

const MAX_BODY: usize = 256 * 1024;

// ------------------------------------------------------------------------------ errors

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Sign-in was cancelled.")]
    Cancelled,
    #[error("Sign-in took too long and was stopped. Try again when you are ready.")]
    TimedOut,
    #[error("The launcher could not open your browser.")]
    BrowserUnavailable,
    #[error("A sign-in is already in progress.")]
    InProgress,
    #[error("Could not reach Microsoft. Check your connection and try again.")]
    Offline,
    #[error("Microsoft did not accept the sign-in ({0}).")]
    OAuth(String),
    /// Refresh failed in a way only the user can fix: sign in again. Never deletes the account.
    #[error("This account needs to sign in again.")]
    NeedsSignIn(ReauthReason),
    #[error("This Microsoft account has no Xbox account. Create one at xbox.com, then try again.")]
    NoXboxAccount,
    #[error("Xbox Live is not available in this account's country or region.")]
    XboxUnavailableInRegion,
    #[error("This account needs adult verification before it can sign in. Finish it on xbox.com.")]
    AdultVerificationRequired,
    #[error("This is a child account. An adult must add it to a Microsoft family before it can sign in.")]
    ChildAccount,
    #[error("This Xbox account is banned.")]
    XboxBanned,
    #[error("Xbox sign-in failed (code {0}).")]
    Xsts(u64),
    /// Microsoft and Xbox sign-in worked, but Mojang has not approved this launcher's client id.
    #[error("Microsoft sign-in worked, but Mojang has not approved this launcher for Minecraft sign-in yet.")]
    ApprovalRequired,
    /// Signed in, but there is no Minecraft: Java Edition profile on this account.
    #[error("{}", if *.owns_minecraft { "This account owns Minecraft but has no Java profile yet. Open the official Minecraft Launcher once to create it, then try again." } else { "This Microsoft account does not own Minecraft: Java Edition." })]
    NoJavaProfile { owns_minecraft: bool },
    #[error("Minecraft sign-in returned something unexpected ({0}).")]
    BadResponse(&'static str),
    #[error("Unlock your accounts first.")]
    VaultLocked,
    #[error("Your accounts could not be saved: {0}")]
    Storage(String),
    #[error("That is a different Minecraft account from the one being signed in again.")]
    WrongAccount,
    #[error("That account no longer exists.")]
    UnknownAccount,
}

impl AuthError {
    /// Stable machine-readable code for the UI.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::BrowserUnavailable => "browser_unavailable",
            Self::InProgress => "in_progress",
            Self::Offline => "offline",
            Self::OAuth(_) => "oauth",
            Self::NeedsSignIn(_) => "needs_sign_in",
            Self::NoXboxAccount => "no_xbox_account",
            Self::XboxUnavailableInRegion => "xbox_region",
            Self::AdultVerificationRequired => "adult_verification",
            Self::ChildAccount => "child_account",
            Self::XboxBanned => "xbox_banned",
            Self::Xsts(_) => "xsts",
            Self::ApprovalRequired => "approval_required",
            Self::NoJavaProfile { .. } => "no_java_profile",
            Self::BadResponse(_) => "bad_response",
            Self::VaultLocked => "vault_locked",
            Self::Storage(_) => "storage",
            Self::WrongAccount => "wrong_account",
            Self::UnknownAccount => "unknown_account",
        }
    }
}

impl AuthError {
    /// Facts the UI needs to word the failure. Never secrets.
    pub fn params(&self) -> Option<serde_json::Value> {
        match self {
            Self::NoJavaProfile { owns_minecraft } => {
                Some(serde_json::json!({ "ownsMinecraft": owns_minecraft }))
            }
            Self::Xsts(code) => Some(serde_json::json!({ "xstsCode": code })),
            _ => None,
        }
    }
}

fn network(e: FetchError) -> AuthError {
    match e {
        FetchError::TooLarge(_) => AuthError::BadResponse("response too large"),
        FetchError::Policy(_) => AuthError::BadResponse("blocked by network policy"),
        _ => AuthError::Offline,
    }
}

// ------------------------------------------------------------------------------ secrets

/// A secret string: wiped on drop, never printed.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

pub struct MsTokens {
    pub access: Secret,
    /// Present because `offline_access` was granted. Replaced on every refresh.
    pub refresh: Option<Secret>,
}

pub struct McSession {
    pub access_token: Secret,
    pub expires_in: u64,
}

impl fmt::Debug for McSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("McSession(<redacted>)")
    }
}

impl fmt::Debug for MsTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MsTokens(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaProfile {
    /// Dashed lowercase UUID.
    pub uuid: String,
    pub name: String,
}

// ------------------------------------------------------------------------------ PKCE and state

pub fn base64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = match *chunk {
            [a, b, c] => u32::from(a) << 16 | u32::from(b) << 8 | u32::from(c),
            [a, b] => u32::from(a) << 16 | u32::from(b) << 8,
            [a] => u32::from(a) << 16,
            _ => 0,
        };
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// 32 random bytes as URL-safe text.
pub fn random_token() -> Result<String, AuthError> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|_| AuthError::BadResponse("no randomness"))?;
    Ok(base64url(&b))
}

pub struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Result<Self, AuthError> {
        Ok(Self::from_verifier(random_token()?))
    }
    /// The S256 challenge: base64url(SHA-256(verifier)).
    pub fn from_verifier(verifier: String) -> Self {
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Self {
            verifier: Secret::new(verifier),
            challenge,
        }
    }
}

pub fn authorize_url(redirect_uri: &str, state: &str, challenge: &str) -> Url {
    let mut url = Url::parse(AUTHORIZE).expect("constant URL"); // infallible: a literal
    url.query_pairs_mut()
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", SCOPE)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        // Lets a person with several Microsoft accounts pick the one to add.
        .append_pair("prompt", "select_account");
    url
}

// ------------------------------------------------------------------------------ requests

fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
        .into_bytes()
}

async fn send<T: Transport>(
    t: &T,
    method: Method,
    url: &str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
) -> Result<HttpResponse, AuthError> {
    let url = Url::parse(url).map_err(|_| AuthError::BadResponse("bad url"))?;
    t.send(HttpRequest {
        method,
        url,
        headers,
        body,
        max_response_bytes: MAX_BODY,
    })
    .await
    .map_err(network)
}

async fn post_json<T: Transport>(
    t: &T,
    url: &str,
    body: &Value,
    extra: Vec<(&'static str, String)>,
) -> Result<HttpResponse, AuthError> {
    let mut headers = vec![
        ("Content-Type", "application/json".to_owned()),
        ("Accept", "application/json".to_owned()),
    ];
    headers.extend(extra);
    send(t, Method::Post, url, headers, body.to_string().into_bytes()).await
}

async fn get_bearer<T: Transport>(
    t: &T,
    url: &str,
    token: &Secret,
) -> Result<HttpResponse, AuthError> {
    send(
        t,
        Method::Get,
        url,
        vec![
            ("Authorization", format!("Bearer {}", token.expose())),
            ("Accept", "application/json".to_owned()),
        ],
        Vec::new(),
    )
    .await
}

fn json_of(r: &HttpResponse) -> Result<Value, AuthError> {
    serde_json::from_slice(&r.body).map_err(|_| AuthError::BadResponse("not JSON"))
}

fn str_at<'a>(v: &'a Value, key: &str, what: &'static str) -> Result<&'a str, AuthError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(AuthError::BadResponse(what))
}

// ------------------------------------------------------------------------------ Microsoft OAuth

#[derive(Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// Exchanging the code from the browser: failure means this sign-in failed.
    Code,
    /// Refreshing: `invalid_grant` and friends mean "sign in again".
    Refresh,
}

fn tokens_from(r: &HttpResponse, grant: Grant) -> Result<MsTokens, AuthError> {
    if r.status >= 500 || r.status == 429 {
        return Err(AuthError::Offline); // Microsoft is busy: retry later, not a verdict on the account
    }
    let v = json_of(r)?;
    if r.status == 200 {
        return Ok(MsTokens {
            access: Secret::new(str_at(&v, "access_token", "no access token")?.to_owned()),
            refresh: v
                .get("refresh_token")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|s| Secret::new(s.to_owned())),
        });
    }
    let code = v.get("error").and_then(Value::as_str).unwrap_or("unknown");
    match grant {
        Grant::Refresh
            if matches!(
                code,
                "invalid_grant" | "interaction_required" | "consent_required" | "login_required"
            ) =>
        {
            Err(AuthError::NeedsSignIn(classify_oauth_error(code)))
        }
        _ => Err(AuthError::OAuth(code.chars().take(40).collect())),
    }
}

pub async fn exchange_code<T: Transport>(
    t: &T,
    redirect_uri: &str,
    code: &Secret,
    verifier: &Secret,
) -> Result<MsTokens, AuthError> {
    let body = form(&[
        ("client_id", CLIENT_ID),
        ("grant_type", "authorization_code"),
        ("code", code.expose()),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier.expose()),
        ("scope", SCOPE),
    ]);
    let r = send(
        t,
        Method::Post,
        TOKEN,
        vec![("Content-Type", "application/x-www-form-urlencoded".into())],
        body,
    )
    .await?;
    tokens_from(&r, Grant::Code)
}

pub async fn refresh_ms<T: Transport>(t: &T, refresh: &Secret) -> Result<MsTokens, AuthError> {
    let body = form(&[
        ("client_id", CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh.expose()),
        ("scope", SCOPE),
    ]);
    let r = send(
        t,
        Method::Post,
        TOKEN,
        vec![("Content-Type", "application/x-www-form-urlencoded".into())],
        body,
    )
    .await?;
    tokens_from(&r, Grant::Refresh)
}

// ------------------------------------------------------------------------------ Xbox and Minecraft

fn xsts_error(code: u64) -> AuthError {
    match code {
        2148916233 => AuthError::NoXboxAccount,
        2148916235 => AuthError::XboxUnavailableInRegion,
        2148916236 | 2148916237 => AuthError::AdultVerificationRequired,
        2148916238 => AuthError::ChildAccount,
        2148916227 => AuthError::XboxBanned,
        other => AuthError::Xsts(other),
    }
}

/// Microsoft access token -> Minecraft services access token.
pub async fn minecraft_login<T: Transport>(
    t: &T,
    ms_access: &Secret,
) -> Result<McSession, AuthError> {
    // 2. Xbox Live user token
    let r = post_json(
        t,
        XBL_AUTH,
        &json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", ms_access.expose()),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
        Vec::new(),
    )
    .await?;
    if r.status != 200 {
        return Err(AuthError::BadResponse("Xbox Live refused the sign-in"));
    }
    let v = json_of(&r)?;
    let xbl = Secret::new(str_at(&v, "Token", "no Xbox token")?.to_owned());

    // 3. XSTS token for the Minecraft services
    let r = post_json(
        t,
        XSTS_AUTH,
        &json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl.expose()] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }),
        Vec::new(),
    )
    .await?;
    let v = json_of(&r).unwrap_or(Value::Null);
    if r.status == 401 {
        return Err(match v.get("XErr").and_then(Value::as_u64) {
            Some(code) => xsts_error(code),
            None => AuthError::BadResponse("XSTS refused the sign-in"),
        });
    }
    if r.status != 200 {
        return Err(AuthError::BadResponse("XSTS refused the sign-in"));
    }
    let xsts = Secret::new(str_at(&v, "Token", "no XSTS token")?.to_owned());
    let uhs = v
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .ok_or(AuthError::BadResponse("no user hash"))?;

    // 4. Minecraft services (needs Mojang's approval of the client id)
    let identity = Secret::new(format!("XBL3.0 x={uhs};{}", xsts.expose()));
    let r = post_json(
        t,
        MC_LOGIN,
        &json!({ "identityToken": identity.expose() }),
        Vec::new(),
    )
    .await?;
    if r.status == 403 {
        return Err(AuthError::ApprovalRequired);
    }
    if r.status != 200 {
        return Err(AuthError::BadResponse(
            "Minecraft services refused the sign-in",
        ));
    }
    let v = json_of(&r)?;
    Ok(McSession {
        access_token: Secret::new(str_at(&v, "access_token", "no Minecraft token")?.to_owned()),
        expires_in: v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600),
    })
}

/// "0000000000004000800000000000a001" -> "00000000-0000-4000-8000-00000000a001".
pub fn dashed_uuid(raw: &str) -> Option<String> {
    let id = raw.replace('-', "").to_ascii_lowercase();
    if id.len() != 32 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    ))
}

fn valid_java_name(n: &str) -> bool {
    (1..=16).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Proves the account really has a Minecraft: Java Edition profile, and returns its name and UUID.
pub async fn verify_java_profile<T: Transport>(
    t: &T,
    mc: &McSession,
) -> Result<JavaProfile, AuthError> {
    let r = get_bearer(t, MC_PROFILE, &mc.access_token).await?;
    match r.status {
        200 => {
            let v = json_of(&r)?;
            let uuid = dashed_uuid(str_at(&v, "id", "no profile id")?)
                .ok_or(AuthError::BadResponse("bad profile id"))?;
            let name = str_at(&v, "name", "no profile name")?;
            if !valid_java_name(name) {
                return Err(AuthError::BadResponse("bad profile name"));
            }
            Ok(JavaProfile {
                uuid,
                name: name.to_owned(),
            })
        }
        404 => {
            // No profile: explain why, using the entitlements (ownership) the same token can read.
            let owns = match get_bearer(t, MC_ENTITLEMENTS, &mc.access_token).await {
                Ok(e) if e.status == 200 => json_of(&e).is_ok_and(|v| owns_minecraft(&v)),
                _ => false,
            };
            Err(AuthError::NoJavaProfile {
                owns_minecraft: owns,
            })
        }
        401 | 403 => Err(AuthError::BadResponse(
            "Minecraft services refused the token",
        )),
        s if s >= 500 || s == 429 => Err(AuthError::Offline),
        _ => Err(AuthError::BadResponse("unexpected profile answer")),
    }
}

fn owns_minecraft(entitlements: &Value) -> bool {
    entitlements
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|i| {
                matches!(
                    i.get("name").and_then(Value::as_str),
                    Some("product_minecraft" | "game_minecraft" | "product_game_pass_pc")
                )
            })
        })
}

/// The whole chain after Microsoft sign-in: tokens -> Minecraft session -> verified Java profile.
pub async fn complete_chain<T: Transport>(
    t: &T,
    ms_access: &Secret,
) -> Result<(McSession, JavaProfile), AuthError> {
    let session = minecraft_login(t, ms_access).await?;
    let profile = verify_java_profile(t, &session).await?;
    Ok((session, profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download_policy::{DownloadPolicy, DEFAULT_ALLOWED_HOSTS};
    use crate::net::testing_transport::FakeTransport;

    const UUID_RAW: &str = "0000000000004000800000000000a001";
    const UUID: &str = "00000000-0000-4000-8000-00000000a001";

    fn ok_chain(t: &FakeTransport) {
        t.reply(
            XBL_AUTH,
            200,
            r#"{"Token":"XBL-TOKEN","DisplayClaims":{"xui":[{"uhs":"1234567890"}]}}"#,
        );
        t.reply(
            XSTS_AUTH,
            200,
            r#"{"Token":"XSTS-TOKEN","DisplayClaims":{"xui":[{"uhs":"1234567890"}]}}"#,
        );
        t.reply(
            MC_LOGIN,
            200,
            r#"{"access_token":"MC-TOKEN","expires_in":86400,"username":"x"}"#,
        );
        t.reply(
            MC_PROFILE,
            200,
            format!(r#"{{"id":"{UUID_RAW}","name":"Steve_01","skins":[],"capes":[]}}"#),
        );
    }

    #[test]
    fn every_endpoint_is_https_and_on_the_network_allow_list() {
        let policy = DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS);
        for e in ENDPOINTS {
            assert!(policy.check(&Url::parse(e).unwrap()).is_ok(), "{e}");
        }
    }

    #[test]
    fn base64url_matches_the_rfc_vectors_and_pkce_matches_the_rfc_example() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        // RFC 7636 appendix B
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let g = Pkce::generate().unwrap();
        assert_eq!(g.verifier.expose().len(), 43);
        assert_ne!(random_token().unwrap(), random_token().unwrap());
    }

    #[test]
    fn the_authorize_url_is_a_public_client_pkce_request_for_personal_accounts() {
        let u = authorize_url("http://localhost:5555", "STATE", "CHALLENGE");
        assert_eq!(u.path(), "/consumers/oauth2/v2.0/authorize");
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["redirect_uri"], "http://localhost:5555");
        assert_eq!(q["scope"], "XboxLive.signin offline_access");
        assert_eq!(q["state"], "STATE");
        assert_eq!(q["code_challenge"], "CHALLENGE");
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(!q.contains_key("client_secret"));
    }

    #[tokio::test]
    async fn the_code_exchange_sends_the_verifier_and_never_a_secret() {
        let t = FakeTransport::new();
        t.reply(
            TOKEN,
            200,
            r#"{"access_token":"MS-ACCESS","refresh_token":"MS-REFRESH","expires_in":3600}"#,
        );
        let tokens = exchange_code(
            &t,
            "http://localhost:5555",
            &Secret::new("CODE".into()),
            &Secret::new("VERIFIER".into()),
        )
        .await
        .unwrap();
        assert_eq!(tokens.access.expose(), "MS-ACCESS");
        assert_eq!(tokens.refresh.unwrap().expose(), "MS-REFRESH");
        let body = &t.calls()[0].3;
        for needed in [
            "grant_type=authorization_code",
            "code=CODE",
            "code_verifier=VERIFIER",
            "client_id=27200331",
        ] {
            assert!(body.contains(needed), "{needed} in {body}");
        }
        assert!(!body.contains("client_secret"));
    }

    #[tokio::test]
    async fn refresh_failures_mean_sign_in_again_but_outages_do_not() {
        let t = FakeTransport::new();
        let tok = Secret::new("R".into());
        t.reply(
            TOKEN,
            400,
            r#"{"error":"invalid_grant","error_description":"expired"}"#,
        );
        assert!(matches!(
            refresh_ms(&t, &tok).await,
            Err(AuthError::NeedsSignIn(ReauthReason::Revoked))
        ));
        t.reply(TOKEN, 400, r#"{"error":"interaction_required"}"#);
        assert!(matches!(
            refresh_ms(&t, &tok).await,
            Err(AuthError::NeedsSignIn(ReauthReason::ConsentRequired))
        ));
        t.reply(TOKEN, 503, "busy");
        assert!(matches!(
            refresh_ms(&t, &tok).await,
            Err(AuthError::Offline)
        ));
        t.offline(TOKEN);
        assert!(matches!(
            refresh_ms(&t, &tok).await,
            Err(AuthError::Offline)
        ));
        // The same code during the first sign-in is a failed sign-in, not "sign in again".
        t.reply(TOKEN, 400, r#"{"error":"invalid_grant"}"#);
        let r = exchange_code(&t, "http://localhost:1", &tok, &tok).await;
        assert!(matches!(r, Err(AuthError::OAuth(c)) if c == "invalid_grant"));
    }

    #[tokio::test]
    async fn the_full_chain_yields_a_verified_java_profile() {
        let t = FakeTransport::new();
        ok_chain(&t);
        let (session, profile) = complete_chain(&t, &Secret::new("MS-ACCESS".into()))
            .await
            .unwrap();
        assert_eq!(session.access_token.expose(), "MC-TOKEN");
        assert_eq!(
            profile,
            JavaProfile {
                uuid: UUID.into(),
                name: "Steve_01".into()
            }
        );
        let calls = t.calls();
        assert!(
            calls[0].3.contains("d=MS-ACCESS") && calls[0].3.contains("http://auth.xboxlive.com")
        );
        assert!(
            calls[1].3.contains("rp://api.minecraftservices.com/")
                && calls[1].3.contains("XBL-TOKEN")
        );
        assert!(calls[2].3.contains("XBL3.0 x=1234567890;XSTS-TOKEN"));
        assert!(calls[3]
            .2
            .iter()
            .any(|(n, v)| n == "Authorization" && v == "Bearer MC-TOKEN"));
    }

    #[tokio::test]
    async fn without_mojangs_approval_the_chain_stops_with_a_distinct_error_and_no_workaround() {
        let t = FakeTransport::new();
        ok_chain(&t);
        t.clear(MC_LOGIN);
        t.reply(MC_LOGIN, 403, r#"{"path":"/authentication/login_with_xbox","errorType":"FORBIDDEN","error":"FORBIDDEN","errorMessage":"Invalid app registration"}"#);
        let r = complete_chain(&t, &Secret::new("MS".into())).await;
        assert!(matches!(r, Err(AuthError::ApprovalRequired)), "{r:?}");
        assert_eq!(AuthError::ApprovalRequired.code(), "approval_required");
        // Nothing was retried through another route and the profile endpoint was never called.
        assert!(!t.urls().iter().any(|u| u.contains("/minecraft/profile")));
    }

    #[tokio::test]
    async fn xsts_failures_become_plain_distinct_errors() {
        for (x, code) in [
            (2148916233u64, "no_xbox_account"),
            (2148916238, "child_account"),
            (2148916235, "xbox_region"),
            (2148916236, "adult_verification"),
            (2148916227, "xbox_banned"),
            (1, "xsts"),
        ] {
            let t = FakeTransport::new();
            t.reply(
                XBL_AUTH,
                200,
                r#"{"Token":"T","DisplayClaims":{"xui":[{"uhs":"1"}]}}"#,
            );
            t.reply(XSTS_AUTH, 401, format!(r#"{{"Identity":"0","XErr":{x},"Message":"","Redirect":"https://start.ui.xboxlive.com/"}}"#));
            let e = minecraft_login(&t, &Secret::new("MS".into()))
                .await
                .err()
                .unwrap();
            assert_eq!(e.code(), code, "{x}");
        }
    }

    #[tokio::test]
    async fn microsoft_login_without_a_java_profile_is_told_apart_from_other_errors() {
        for (items, owns) in [
            (r#"{"items":[{"name":"product_minecraft"}]}"#, true),
            (r#"{"items":[]}"#, false),
        ] {
            let t = FakeTransport::new();
            t.reply(
                MC_PROFILE,
                404,
                r#"{"path":"/minecraft/profile","errorType":"NOT_FOUND"}"#,
            );
            t.reply(MC_ENTITLEMENTS, 200, items);
            let mc = McSession {
                access_token: Secret::new("MC".into()),
                expires_in: 1,
            };
            let e = verify_java_profile(&t, &mc).await.err().unwrap();
            assert!(
                matches!(e, AuthError::NoJavaProfile { owns_minecraft } if owns_minecraft == owns)
            );
            assert_eq!(e.code(), "no_java_profile");
        }
    }

    #[tokio::test]
    async fn hostile_profile_data_is_refused() {
        for body in [
            r#"{"id":"not-a-uuid","name":"Steve"}"#,
            r#"{"id":"0000000000004000800000000000a001","name":"../../x"}"#,
            r#"{"id":"0000000000004000800000000000a001","name":""}"#,
            r#"{"name":"Steve"}"#,
            "<html>",
        ] {
            let t = FakeTransport::new();
            t.reply(MC_PROFILE, 200, body);
            let mc = McSession {
                access_token: Secret::new("MC".into()),
                expires_in: 1,
            };
            assert!(verify_java_profile(&t, &mc).await.is_err(), "{body}");
        }
    }

    /// Run by hand: `cargo test -- --ignored live_`. Sends deliberately invalid values (no account,
    /// no credentials) to the real Microsoft endpoint to prove the client id is accepted and the
    /// error mapping matches the real answers.
    #[tokio::test]
    #[ignore = "talks to the real Microsoft endpoint"]
    async fn live_microsoft_answers_garbage_the_way_the_mapping_expects() {
        let t =
            crate::net::HttpFetcher::for_auth(DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS)).unwrap();
        let g = Secret::new("not-a-real-token".into());
        let refreshed = refresh_ms(&t, &g).await;
        println!(
            "refresh with garbage -> {:?}",
            refreshed.as_ref().err().map(|e| e.code())
        );
        assert!(
            matches!(refreshed, Err(AuthError::NeedsSignIn(_))),
            "{refreshed:?}"
        );
        let exchanged = exchange_code(&t, "http://localhost:5555", &g, &g).await;
        println!(
            "exchange with garbage -> {:?}",
            exchanged.as_ref().err().map(ToString::to_string)
        );
        // invalid_grant (the code is bad) proves Microsoft accepts this client id and redirect.
        assert!(
            matches!(&exchanged, Err(AuthError::OAuth(c)) if c == "invalid_grant"),
            "{exchanged:?}"
        );
    }

    #[test]
    fn secrets_and_errors_never_print_token_values() {
        let s = Secret::new("TOP-SECRET".into());
        assert!(!format!("{s:?}").contains("TOP-SECRET"));
        for e in [
            AuthError::OAuth("invalid_grant".into()),
            AuthError::BadResponse("x"),
            AuthError::ApprovalRequired,
        ] {
            assert!(!e.to_string().contains("TOP-SECRET"));
        }
    }
}
