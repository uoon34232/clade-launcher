//! The signed-in Minecraft Java profile: skin and cape changes, made account-side.
//!
//! Rules this module exists to enforce:
//! - Skin and cape changes go through Mojang's authenticated Minecraft services, so the same
//!   profile looks the same in the game, the official launcher and the account pages. There are no
//!   launcher-local cosmetic overrides, and no third-party skin or cape services.
//! - Only capes the account really owns can be equipped (`OwnedCape`); there is no way to express
//!   "upload a cape".
//! - Accounts are independent from instances: no function here takes or returns an instance.
//! - Every call acts on the ONE account id it is given. That account must exist and be `Ready`;
//!   another account is never substituted. Credentials come from the existing refresh hook
//!   ([`crate::auth::ensure_fresh`]) and the portable vault; the Minecraft access token is read,
//!   used for one request and dropped here, and never reaches the frontend, a log or an error.
//!
//! Endpoints (https only, the fixed host [`SERVICE_HOST`], bearer token; as documented by the
//! community-maintained Minecraft Wiki and used by the official launcher, since Mojang publishes no
//! formal specification, so everything that depends on it stays in this module):
//! `GET /minecraft/profile`, `POST /minecraft/profile/skins` (multipart `variant` + `file`),
//! `DELETE /minecraft/profile/skins/active`, `PUT /minecraft/profile/capes/active`
//! (`{"capeId"}`), `DELETE /minecraft/profile/capes/active`. The client never follows a redirect,
//! so the token cannot be forwarded to another host, and a change is sent exactly once: it is never
//! retried (the skin endpoint is rate limited).

use std::future::Future;
use std::sync::Mutex;

use serde_json::Value;
use url::Url;

use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
use crate::accounts_api::AccountsState;
use crate::auth::ensure_fresh;
use crate::coded_error::CodedError;
use crate::ms_auth::{dashed_uuid, AuthError, Secret};
use crate::net::{FetchError, HttpRequest, HttpResponse, Method, Transport};
use crate::vault::Vault;

pub const SERVICE_HOST: &str = "api.minecraftservices.com";
pub const PROFILE_PATH: &str = "/minecraft/profile";
pub const SKINS_PATH: &str = "/minecraft/profile/skins";
pub const ACTIVE_SKIN_PATH: &str = "/minecraft/profile/skins/active";
pub const ACTIVE_CAPE_PATH: &str = "/minecraft/profile/capes/active";

/// A local skin file is read fully into memory, so it is bounded. A 64x64 skin is a few kilobytes;
/// even an uncompressed one is about 16 KB.
pub const MAX_SKIN_FILE_BYTES: usize = 256 * 1024;
/// A profile answer is small; anything bigger is not one.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Longest cape name kept for the interface.
const MAX_ALIAS_CHARS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkinVariant {
    /// "Classic" (wide arms).
    Classic,
    Slim,
}

impl SkinVariant {
    /// The value of the service's `variant` form field.
    pub fn form_value(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Slim => "slim",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "classic" => Some(Self::Classic),
            "slim" => Some(Self::Slim),
            _ => None,
        }
    }
}

/// A cape identifier as the service reports it (a UUID). Validated so it can be put in a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapeId(String);

impl CapeId {
    pub fn new(raw: &str) -> Result<Self, ProfileServiceError> {
        let ok = raw.len() == 36
            && raw.char_indices().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            });
        if ok {
            Ok(Self(raw.to_ascii_lowercase()))
        } else {
            Err(ProfileServiceError::CapeNotOwned)
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A cape the account owns, as reported by its Minecraft profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedCape {
    pub id: CapeId,
    /// The name the service gives the cape ("Migrator"), cleaned up for display.
    pub alias: Option<String>,
    pub active: bool,
}

/// What the service reports about one account's profile. Only what the launcher uses is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftProfile {
    /// Dashed lowercase UUID.
    pub uuid: String,
    pub name: String,
    /// The model of the active skin, when the service says.
    pub skin_variant: Option<SkinVariant>,
    /// The public address of the active skin texture. Never sent to the frontend; `skin.rs`
    /// validates it before anything is fetched.
    pub skin_url: Option<String>,
    pub capes: Vec<OwnedCape>,
}

/// Why a skin file was refused before anything was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkinProblem {
    NotPng,
    WrongSize,
    TooLarge,
}

/// A skin image the user picked, checked before anything is sent anywhere.
#[derive(Debug, PartialEq, Eq)]
pub struct SkinPng(Vec<u8>);

impl SkinPng {
    /// Accepts a PNG whose size is 64x64 or 64x32 (the Java skin formats) within
    /// [`MAX_SKIN_FILE_BYTES`]. The service validates again; this stops wrong files early.
    pub fn validate(bytes: Vec<u8>) -> Result<Self, ProfileServiceError> {
        const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
        if bytes.len() > MAX_SKIN_FILE_BYTES {
            return Err(ProfileServiceError::InvalidSkin(SkinProblem::TooLarge));
        }
        // 8 signature + 4 length (13) + 4 "IHDR" + 4 width + 4 height
        if bytes.len() < 24
            || &bytes[..8] != SIG
            || bytes[8..12] != [0, 0, 0, 13]
            || &bytes[12..16] != b"IHDR"
        {
            return Err(ProfileServiceError::InvalidSkin(SkinProblem::NotPng));
        }
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        if (w, h) != (64, 64) && (w, h) != (64, 32) {
            return Err(ProfileServiceError::InvalidSkin(SkinProblem::WrongSize));
        }
        Ok(Self(bytes))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Everything that can go wrong, in terms the interface can word. No variant carries a response
/// body, a token or a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProfileServiceError {
    #[error("That account no longer exists.")]
    UnknownAccount,
    #[error("This account needs to sign in again.")]
    NeedsSignIn,
    #[error("Unlock your accounts first.")]
    VaultLocked,
    #[error("That is not a usable Minecraft skin.")]
    InvalidSkin(SkinProblem),
    /// Minecraft services refused the credentials even after a normal refresh.
    #[error("Minecraft services did not accept the sign-in.")]
    Unauthorized,
    #[error("Minecraft services refused this change.")]
    Forbidden,
    #[error("This account does not own that cape.")]
    CapeNotOwned,
    #[error("Too many changes; try again later.")]
    RateLimited,
    #[error("Could not reach Minecraft services.")]
    Offline,
    #[error("Minecraft services are not available right now.")]
    ServiceUnavailable,
    #[error("This account has no Minecraft: Java Edition profile.")]
    NoProfile,
    #[error("Minecraft services answered something unexpected ({0}).")]
    BadResponse(&'static str),
    #[error("The account list could not be read.")]
    Storage,
    #[error("Another appearance change is already running.")]
    Busy,
}

impl ProfileServiceError {
    /// Stable machine-readable code for the interface.
    pub fn code(self) -> &'static str {
        match self {
            Self::UnknownAccount => "unknown_account",
            Self::NeedsSignIn => "needs_sign_in",
            Self::VaultLocked => "vault_locked",
            Self::InvalidSkin(_) => "invalid_skin",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::CapeNotOwned => "cape_not_owned",
            Self::RateLimited => "rate_limited",
            Self::Offline => "offline",
            Self::ServiceUnavailable => "service_unavailable",
            Self::NoProfile => "no_profile",
            Self::BadResponse(_) => "bad_response",
            Self::Storage => "storage",
            Self::Busy => "busy",
        }
    }

    pub fn coded(self) -> CodedError {
        CodedError::new(self.code(), self.to_string())
    }
}

/// What the signed-in profile service can do. Every method acts on one account's own profile.
pub trait ProfileService: Send + Sync {
    fn profile(
        &self,
        account_id: &str,
    ) -> impl Future<Output = Result<MinecraftProfile, ProfileServiceError>> + Send;
    fn upload_skin(
        &self,
        account_id: &str,
        variant: SkinVariant,
        skin: SkinPng,
    ) -> impl Future<Output = Result<MinecraftProfile, ProfileServiceError>> + Send;
    fn reset_skin(
        &self,
        account_id: &str,
    ) -> impl Future<Output = Result<MinecraftProfile, ProfileServiceError>> + Send;
    /// `None` hides the cape. `Some` must be one of the account's [`OwnedCape`]s.
    fn set_cape(
        &self,
        account_id: &str,
        cape: Option<CapeId>,
    ) -> impl Future<Output = Result<MinecraftProfile, ProfileServiceError>> + Send;
}

// ------------------------------------------------------------------------------------ parsing

fn valid_java_name(n: &str) -> bool {
    (1..=16).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_active(entry: &Value) -> bool {
    entry
        .get("state")
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("ACTIVE"))
}

/// A cape name made safe to show: printable characters only, trimmed and short.
fn clean_alias(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ALIAS_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Reads the profile document. Unknown fields are ignored (the service may add some); an entry the
/// launcher cannot use (a cape with a malformed id) is left out instead of failing the whole answer.
pub fn parse_profile(body: &[u8]) -> Result<MinecraftProfile, ProfileServiceError> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|_| ProfileServiceError::BadResponse("the profile is not JSON"))?;
    let uuid = v
        .get("id")
        .and_then(Value::as_str)
        .and_then(dashed_uuid)
        .ok_or(ProfileServiceError::BadResponse(
            "the profile has no valid id",
        ))?;
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| valid_java_name(n))
        .ok_or(ProfileServiceError::BadResponse(
            "the profile has no valid name",
        ))?
        .to_owned();
    let active_skin = v
        .get("skins")
        .and_then(Value::as_array)
        .and_then(|skins| skins.iter().find(|s| is_active(s)));
    let skin_variant = active_skin
        .and_then(|s| s.get("variant"))
        .and_then(Value::as_str)
        .and_then(SkinVariant::parse);
    let skin_url = active_skin
        .and_then(|s| s.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let capes = v
        .get("capes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|c| {
                    let id = CapeId::new(c.get("id")?.as_str()?).ok()?;
                    Some(OwnedCape {
                        id,
                        alias: c.get("alias").and_then(Value::as_str).and_then(clean_alias),
                        active: is_active(c),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(MinecraftProfile {
        uuid,
        name,
        skin_variant,
        skin_url,
        capes,
    })
}

/// The `multipart/form-data` body of a skin upload: the `variant` field, then the `file`.
/// Returns the body and the `Content-Type` header that names its boundary.
pub fn skin_form(
    variant: SkinVariant,
    skin: &SkinPng,
) -> Result<(Vec<u8>, String), ProfileServiceError> {
    let contains =
        |haystack: &[u8], needle: &[u8]| haystack.windows(needle.len()).any(|w| w == needle);
    // A random boundary that, as a certainty rather than a hope, is not inside the image.
    let boundary = loop {
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw).map_err(|_| ProfileServiceError::BadResponse("no randomness"))?;
        let candidate = format!(
            "clade-{}",
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        if !contains(skin.bytes(), candidate.as_bytes()) {
            break candidate;
        }
    };
    let mut body = Vec::with_capacity(skin.bytes().len() + 400);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"variant\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"skin.png\"\r\nContent-Type: image/png\r\n\r\n",
            variant.form_value()
        )
        .as_bytes(),
    );
    body.extend_from_slice(skin.bytes());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((body, format!("multipart/form-data; boundary={boundary}")))
}

/// One change the service is asked to make.
struct Change<'a> {
    op: Op,
    method: Method,
    path: &'a str,
    content_type: Option<String>,
    body: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Read,
    Skin,
    Cape,
}

/// How a failed status is worded. The response body is never read for this.
fn status_error(status: u16, op: Op) -> ProfileServiceError {
    match status {
        400 => match op {
            Op::Skin => ProfileServiceError::InvalidSkin(SkinProblem::NotPng),
            Op::Cape => ProfileServiceError::CapeNotOwned,
            Op::Read => ProfileServiceError::BadResponse("the request was refused"),
        },
        401 => ProfileServiceError::Unauthorized,
        403 => ProfileServiceError::Forbidden,
        404 => ProfileServiceError::NoProfile,
        429 => ProfileServiceError::RateLimited,
        500..=599 => ProfileServiceError::ServiceUnavailable,
        _ => ProfileServiceError::BadResponse("an unexpected status"),
    }
}

fn fetch_error(e: &FetchError) -> ProfileServiceError {
    match e {
        FetchError::Network(_) => ProfileServiceError::Offline,
        FetchError::TooLarge(_) => ProfileServiceError::BadResponse("the answer was too large"),
        _ => ProfileServiceError::ServiceUnavailable,
    }
}

fn auth_error(e: &AuthError) -> ProfileServiceError {
    match e {
        AuthError::VaultLocked => ProfileServiceError::VaultLocked,
        AuthError::NeedsSignIn(_) => ProfileServiceError::NeedsSignIn,
        AuthError::Offline => ProfileServiceError::Offline,
        AuthError::Storage(_) => ProfileServiceError::Storage,
        _ => ProfileServiceError::Unauthorized,
    }
}

// ------------------------------------------------------------------------------ the real service

/// The real service: one account's own profile, through the existing credential machinery.
pub struct MinecraftServices<'a, T: Transport> {
    pub transport: &'a T,
    pub vault: &'a Mutex<Option<Vault>>,
    pub accounts: &'a AccountsState,
    pub now: i64,
}

impl<T: Transport> MinecraftServices<'_, T> {
    /// The exact account asked for, which must be `Ready`, and a fresh token for it.
    async fn credentials(
        &self,
        account_id: &str,
    ) -> Result<(AccountMeta, Secret), ProfileServiceError> {
        let file =
            AccountsFile::load(self.accounts.path()).map_err(|_| ProfileServiceError::Storage)?;
        let meta = file
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or(ProfileServiceError::UnknownAccount)?
            .clone();
        if !matches!(meta.status, AccountStatus::Ready) {
            return Err(ProfileServiceError::NeedsSignIn);
        }
        let token = ensure_fresh(
            self.transport,
            self.vault,
            self.accounts,
            account_id,
            self.now,
        )
        .await
        .map_err(|e| auth_error(&e))?;
        Ok((meta, token))
    }

    async fn request(
        &self,
        token: &Secret,
        method: Method,
        path: &str,
        content_type: Option<String>,
        body: Vec<u8>,
    ) -> Result<HttpResponse, ProfileServiceError> {
        // The host is a constant: no caller can steer the credential anywhere else.
        let url = Url::parse(&format!("https://{SERVICE_HOST}{path}"))
            .map_err(|_| ProfileServiceError::BadResponse("the address"))?;
        let mut headers = vec![
            ("Authorization", format!("Bearer {}", token.expose())),
            ("Accept", "application/json".to_owned()),
        ];
        if let Some(ct) = content_type {
            headers.push(("Content-Type", ct));
        }
        self.transport
            .send(HttpRequest {
                method,
                url,
                headers,
                body,
                max_response_bytes: MAX_RESPONSE_BYTES,
            })
            .await
            .map_err(|e| fetch_error(&e))
    }

    /// `GET /minecraft/profile`, checked to be the profile of the account it was asked for.
    async fn read(
        &self,
        meta: &AccountMeta,
        token: &Secret,
    ) -> Result<MinecraftProfile, ProfileServiceError> {
        let r = self
            .request(token, Method::Get, PROFILE_PATH, None, Vec::new())
            .await?;
        if r.status != 200 {
            return Err(status_error(r.status, Op::Read));
        }
        Self::for_account(meta, parse_profile(&r.body)?)
    }

    /// A profile that is not the account's own is never used (it could not be, unless the service
    /// or a proxy misbehaved).
    fn for_account(
        meta: &AccountMeta,
        profile: MinecraftProfile,
    ) -> Result<MinecraftProfile, ProfileServiceError> {
        if dashed_uuid(&meta.minecraft_uuid).as_deref() == Some(profile.uuid.as_str()) {
            Ok(profile)
        } else {
            Err(ProfileServiceError::BadResponse("another profile answered"))
        }
    }

    /// One change request, sent once. The answer is the new profile; when the service answers a
    /// bare success the current profile is read (a read, never a second change).
    async fn change(
        &self,
        meta: &AccountMeta,
        token: &Secret,
        change: Change<'_>,
    ) -> Result<MinecraftProfile, ProfileServiceError> {
        let r = self
            .request(
                token,
                change.method,
                change.path,
                change.content_type,
                change.body,
            )
            .await?;
        if !matches!(r.status, 200 | 204) {
            return Err(status_error(r.status, change.op));
        }
        match parse_profile(&r.body).and_then(|p| Self::for_account(meta, p)) {
            Ok(profile) => Ok(profile),
            Err(_) => self.read(meta, token).await,
        }
    }
}

impl<T: Transport> ProfileService for MinecraftServices<'_, T> {
    async fn profile(&self, account_id: &str) -> Result<MinecraftProfile, ProfileServiceError> {
        let (meta, token) = self.credentials(account_id).await?;
        self.read(&meta, &token).await
    }

    async fn upload_skin(
        &self,
        account_id: &str,
        variant: SkinVariant,
        skin: SkinPng,
    ) -> Result<MinecraftProfile, ProfileServiceError> {
        let (body, content_type) = skin_form(variant, &skin)?;
        let (meta, token) = self.credentials(account_id).await?;
        self.change(
            &meta,
            &token,
            Change {
                op: Op::Skin,
                method: Method::Post,
                path: SKINS_PATH,
                content_type: Some(content_type),
                body,
            },
        )
        .await
    }

    async fn reset_skin(&self, account_id: &str) -> Result<MinecraftProfile, ProfileServiceError> {
        let (meta, token) = self.credentials(account_id).await?;
        self.change(
            &meta,
            &token,
            Change {
                op: Op::Skin,
                method: Method::Delete,
                path: ACTIVE_SKIN_PATH,
                content_type: None,
                body: Vec::new(),
            },
        )
        .await
    }

    async fn set_cape(
        &self,
        account_id: &str,
        cape: Option<CapeId>,
    ) -> Result<MinecraftProfile, ProfileServiceError> {
        let (meta, token) = self.credentials(account_id).await?;
        match cape {
            None => {
                self.change(
                    &meta,
                    &token,
                    Change {
                        op: Op::Cape,
                        method: Method::Delete,
                        path: ACTIVE_CAPE_PATH,
                        content_type: None,
                        body: Vec::new(),
                    },
                )
                .await
            }
            Some(id) => {
                // Only a cape from THIS account's own list is ever sent.
                let current = self.read(&meta, &token).await?;
                if !current.capes.iter().any(|c| c.id == id) {
                    return Err(ProfileServiceError::CapeNotOwned);
                }
                let body = serde_json::json!({ "capeId": id.as_str() })
                    .to_string()
                    .into_bytes();
                self.change(
                    &meta,
                    &token,
                    Change {
                        op: Op::Cape,
                        method: Method::Put,
                        path: ACTIVE_CAPE_PATH,
                        content_type: Some("application/json".to_owned()),
                        body,
                    },
                )
                .await
            }
        }
    }
}

#[cfg(test)]
mod tests;
