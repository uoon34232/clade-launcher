//! The interface to the signed-in Minecraft Java profile: skin and cape changes.
//!
//! Architecture only. Nothing here talks to the network yet; the only implementation is
//! [`UnavailableService`], which fails safely. The real one arrives with Microsoft sign-in.
//!
//! Rules this interface exists to enforce:
//! - Skin and cape changes are made **account-side**, through Mojang's authenticated Minecraft
//!   services, so the same profile looks the same in the game, the official launcher and the
//!   account pages. There are no launcher-local cosmetic overrides, and no third-party skin or
//!   cape services.
//! - Only capes the account really owns can be equipped (`OwnedCape`); there is no way to
//!   express "upload a cape".
//! - Accounts are independent from instances: no function here takes or returns an instance.
//!
//! Endpoints (https only, host `api.minecraftservices.com`, bearer token from the vault; as
//! documented by the community-maintained Minecraft Wiki and used by the official launcher, since
//! Mojang publishes no formal spec; re-verify when implementing):
//! `GET /minecraft/profile`, `POST /minecraft/profile/skins` (multipart `variant` + `file`),
//! `DELETE /minecraft/profile/skins/active`, `PUT /minecraft/profile/capes/active` (`{"capeId"}`),
//! `DELETE /minecraft/profile/capes/active`. Access for a third-party app requires Mojang's
//! approval of its Azure application; without it the service answers 403.

use std::future::Future;

use serde::{Deserialize, Serialize};

pub const SERVICE_HOST: &str = "api.minecraftservices.com";
pub const PROFILE_PATH: &str = "/minecraft/profile";
pub const SKINS_PATH: &str = "/minecraft/profile/skins";
pub const ACTIVE_SKIN_PATH: &str = "/minecraft/profile/skins/active";
pub const ACTIVE_CAPE_PATH: &str = "/minecraft/profile/capes/active";

/// A local skin file is read fully into memory, so it is bounded.
pub const MAX_SKIN_FILE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// A cape identifier as the service reports it (a UUID). Validated so it can be put in a body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
            Err(ProfileServiceError::Invalid("not a cape id".into()))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A cape the account owns, as reported by its Minecraft profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedCape {
    pub id: CapeId,
    pub alias: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftProfile {
    pub uuid: String,
    pub name: String,
    pub skin_variant: Option<SkinVariant>,
    pub capes: Vec<OwnedCape>,
}

/// A skin image the user picked, checked before anything is sent anywhere.
#[derive(Debug, PartialEq, Eq)]
pub struct SkinPng(Vec<u8>);

impl SkinPng {
    /// Accepts a PNG whose size is 64x64 or 64x32 (the skin formats). The service validates
    /// again; this only stops obviously wrong files early with a clear message.
    pub fn validate(bytes: Vec<u8>) -> Result<Self, ProfileServiceError> {
        const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
        if bytes.len() > MAX_SKIN_FILE_BYTES {
            return Err(ProfileServiceError::Invalid(
                "That file is too large for a skin.".into(),
            ));
        }
        // 8 signature + 4 length + 4 "IHDR" + 4 width + 4 height
        if bytes.len() < 24 || &bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
            return Err(ProfileServiceError::Invalid("Choose a PNG image.".into()));
        }
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        if (w, h) != (64, 64) && (w, h) != (64, 32) {
            return Err(ProfileServiceError::Invalid(
                "A skin must be 64\u{d7}64 pixels (or the older 64\u{d7}32).".into(),
            ));
        }
        Ok(Self(bytes))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileServiceError {
    /// The official capability is not available (not implemented yet, or not permitted).
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Invalid(String),
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

/// The only implementation for now: every capability is reported as unavailable.
pub struct UnavailableService;

const NOT_YET: &str = "Skin and cape changes need Microsoft sign-in, which isn't available yet.";

impl ProfileService for UnavailableService {
    async fn profile(&self, _: &str) -> Result<MinecraftProfile, ProfileServiceError> {
        Err(ProfileServiceError::Unavailable(NOT_YET.into()))
    }
    async fn upload_skin(
        &self,
        _: &str,
        _: SkinVariant,
        _: SkinPng,
    ) -> Result<MinecraftProfile, ProfileServiceError> {
        Err(ProfileServiceError::Unavailable(NOT_YET.into()))
    }
    async fn reset_skin(&self, _: &str) -> Result<MinecraftProfile, ProfileServiceError> {
        Err(ProfileServiceError::Unavailable(NOT_YET.into()))
    }
    async fn set_cape(
        &self,
        _: &str,
        _: Option<CapeId>,
    ) -> Result<MinecraftProfile, ProfileServiceError> {
        Err(ProfileServiceError::Unavailable(NOT_YET.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download_policy::{DownloadPolicy, DEFAULT_ALLOWED_HOSTS};
    use url::Url;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b
    }

    #[test]
    fn every_official_endpoint_is_https_on_the_minecraft_services_host_and_allowed() {
        let policy = DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS);
        for path in [PROFILE_PATH, SKINS_PATH, ACTIVE_SKIN_PATH, ACTIVE_CAPE_PATH] {
            let url = Url::parse(&format!("https://{SERVICE_HOST}{path}")).unwrap();
            assert!(policy.check(&url).is_ok(), "{path}");
        }
    }

    #[test]
    fn skin_files_are_checked_before_use() {
        assert!(SkinPng::validate(png(64, 64)).is_ok());
        assert!(SkinPng::validate(png(64, 32)).is_ok());
        assert!(SkinPng::validate(png(128, 128)).is_err());
        assert!(SkinPng::validate(b"GIF89a-not-a-png-at-all-really".to_vec()).is_err());
        assert!(SkinPng::validate(Vec::new()).is_err());
        assert!(SkinPng::validate(vec![0; MAX_SKIN_FILE_BYTES + 1]).is_err());
    }

    #[test]
    fn cape_ids_are_uuids_only() {
        assert!(CapeId::new("2340C0E0-3A8C-4F5B-9F2E-0123456789AB").is_ok());
        assert!(CapeId::new("../../etc").is_err());
        assert!(CapeId::new("{\"capeId\":1}").is_err());
    }

    #[test]
    fn variants_use_the_services_spelling() {
        assert_eq!(SkinVariant::Classic.form_value(), "classic");
        assert_eq!(SkinVariant::Slim.form_value(), "slim");
    }

    #[tokio::test]
    async fn the_default_service_fails_safely_for_every_capability() {
        let s = UnavailableService;
        assert!(matches!(
            s.profile("a").await,
            Err(ProfileServiceError::Unavailable(_))
        ));
        assert!(s.reset_skin("a").await.is_err());
        assert!(s.set_cape("a", None).await.is_err());
        let skin = SkinPng::validate(png(64, 64)).unwrap();
        assert!(s.upload_skin("a", SkinVariant::Slim, skin).await.is_err());
    }
}
