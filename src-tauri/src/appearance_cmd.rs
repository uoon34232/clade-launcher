//! The commands behind the account dialog's Skin and Cape sections.
//!
//! Every command names ONE account by its id and acts on that account only (see
//! `profile_service.rs`). Credentials are resolved in Rust from the encrypted vault and never leave
//! it: the frontend sends an account id, a skin model, PNG bytes or an owned cape id, and gets back
//! plain appearance data. A successful change updates the account's cached avatar and tells the
//! interface (`accounts-changed`). The log carries the kind of change and its result code, nothing
//! else: no names, ids, paths or credentials.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::accounts_api::{AccountsState, ACCOUNTS_CHANGED_EVENT};
use crate::auth::AuthState;
use crate::coded_error::CodedError;
use crate::net::HttpFetcher;
use crate::profile_service::{
    CapeId, MinecraftProfile, MinecraftServices, ProfileService, ProfileServiceError, SkinPng,
    SkinProblem, SkinVariant, MAX_SKIN_FILE_BYTES,
};
use crate::profiles::now_unix;

/// How long a change waits for the new texture to be cached before answering without it.
const SKIN_WAIT: Duration = Duration::from_secs(3);
/// Extra array elements drained past [`MAX_SKIN_FILE_BYTES`] so modestly oversized files still
/// surface as `invalid_skin` (`TooLarge`), while multi-megabyte payloads abort immediately.
const MAX_SKIN_DRAIN_EXTRA: usize = 65_536;

/// Bounded deserializer for IPC skin uploads (`png`), preventing unbounded `Vec<u8>` allocation.
#[derive(Debug, PartialEq, Eq)]
pub struct BoundedSkinBytes(Result<Vec<u8>, SkinProblem>);

impl BoundedSkinBytes {
    fn into_bytes(self) -> Result<Vec<u8>, ProfileServiceError> {
        self.0.map_err(ProfileServiceError::InvalidSkin)
    }
}

struct BoundedSkinVisitor;

impl<'de> serde::de::Visitor<'de> for BoundedSkinVisitor {
    type Value = BoundedSkinBytes;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a byte array of at most {MAX_SKIN_FILE_BYTES} bytes")
    }

    fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
        if v.len() <= MAX_SKIN_FILE_BYTES {
            Ok(BoundedSkinBytes(Ok(v.to_vec())))
        } else if v.len() <= MAX_SKIN_FILE_BYTES + MAX_SKIN_DRAIN_EXTRA {
            Ok(BoundedSkinBytes(Err(SkinProblem::TooLarge)))
        } else {
            Err(E::custom("skin file exceeds maximum size"))
        }
    }

    fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
        if v.len() <= MAX_SKIN_FILE_BYTES {
            Ok(BoundedSkinBytes(Ok(v)))
        } else if v.len() <= MAX_SKIN_FILE_BYTES + MAX_SKIN_DRAIN_EXTRA {
            Ok(BoundedSkinBytes(Err(SkinProblem::TooLarge)))
        } else {
            Err(E::custom("skin file exceeds maximum size"))
        }
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let cap = seq.size_hint().unwrap_or(0).min(MAX_SKIN_FILE_BYTES);
        let mut buf = Vec::with_capacity(cap);
        while buf.len() < MAX_SKIN_FILE_BYTES {
            match seq.next_element::<u8>()? {
                Some(b) => buf.push(b),
                None => return Ok(BoundedSkinBytes(Ok(buf))),
            }
        }
        if seq.next_element::<u8>()?.is_none() {
            return Ok(BoundedSkinBytes(Ok(buf)));
        }
        drop(buf);
        for _ in 1..MAX_SKIN_DRAIN_EXTRA {
            if seq.next_element::<u8>()?.is_none() {
                return Ok(BoundedSkinBytes(Err(SkinProblem::TooLarge)));
            }
        }
        if seq.next_element::<u8>()?.is_none() {
            return Ok(BoundedSkinBytes(Err(SkinProblem::TooLarge)));
        }
        Err(serde::de::Error::custom("skin file exceeds maximum size"))
    }
}

impl<'de> serde::Deserialize<'de> for BoundedSkinBytes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_byte_buf(BoundedSkinVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapeDto {
    /// Only ever passed back to `set_account_cape`; the interface does not show it.
    pub id: String,
    pub alias: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceDto {
    /// "classic" or "slim" when the service names the active skin's model.
    pub skin_variant: Option<&'static str>,
    pub capes: Vec<CapeDto>,
}

impl AppearanceDto {
    pub fn from_profile(p: &MinecraftProfile) -> Self {
        Self {
            skin_variant: p.skin_variant.map(SkinVariant::form_value),
            capes: p
                .capes
                .iter()
                .map(|c| CapeDto {
                    id: c.id.as_str().to_owned(),
                    alias: c.alias.clone(),
                    active: c.active,
                })
                .collect(),
        }
    }
}

fn service<'a>(
    auth: &'a AuthState,
    accounts: &'a AccountsState,
) -> Result<MinecraftServices<'a, HttpFetcher>, ProfileServiceError> {
    let transport = auth
        .http
        .as_ref()
        .map_err(|_| ProfileServiceError::Offline)?;
    Ok(MinecraftServices {
        transport,
        vault: &auth.vault,
        accounts,
        now: now_unix(),
    })
}

/// Notes the skin the profile reports for the account, so the avatar follows it (the texture is
/// fetched best effort: a temporary failure there never undoes a change Minecraft accepted).
fn note_skin(accounts: &AccountsState, profile: &MinecraftProfile) {
    accounts.record_skin(&profile.uuid, profile.skin_url.as_deref());
}

fn done(op: &'static str, r: &Result<MinecraftProfile, ProfileServiceError>) {
    match r {
        Ok(_) => tracing::info!(op, result = "ok", "profile change"),
        Err(e) => tracing::info!(op, result = e.code(), "profile change"),
    }
}

/// Everything after a successful change: avatar, event, answer.
async fn finish(
    app: &AppHandle,
    accounts: &AccountsState,
    profile: &MinecraftProfile,
) -> AppearanceDto {
    note_skin(accounts, profile);
    accounts.sync_pending_skins(SKIN_WAIT).await; // briefly; whatever is not done finishes later
    let _ = app.emit(ACCOUNTS_CHANGED_EVENT, ());
    AppearanceDto::from_profile(profile)
}

/// The account's current skin model and the capes it owns.
#[tauri::command]
pub async fn account_appearance(
    app: AppHandle,
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    id: String,
) -> Result<AppearanceDto, CodedError> {
    let result = async { service(&auth, &accounts)?.profile(&id).await }.await;
    done("read", &result);
    let profile = result.map_err(ProfileServiceError::coded)?;
    // The profile may show a skin changed elsewhere: let the avatar follow it, in the background.
    note_skin(&accounts, &profile);
    accounts.sync_skins_in_background(&app);
    Ok(AppearanceDto::from_profile(&profile))
}

/// Uploads a skin PNG with the chosen model. The bytes are checked here, whatever the interface did.
#[tauri::command]
pub async fn set_account_skin(
    app: AppHandle,
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    id: String,
    variant: SkinVariant,
    png: BoundedSkinBytes,
) -> Result<AppearanceDto, CodedError> {
    let result = async {
        let skin = SkinPng::validate(png.into_bytes()?)?;
        let _change = accounts
            .begin_appearance_change()
            .ok_or(ProfileServiceError::Busy)?;
        service(&auth, &accounts)?
            .upload_skin(&id, variant, skin)
            .await
    }
    .await;
    done("skin_upload", &result);
    let profile = result.map_err(ProfileServiceError::coded)?;
    Ok(finish(&app, &accounts, &profile).await)
}

/// Goes back to the account's default skin.
#[tauri::command]
pub async fn reset_account_skin(
    app: AppHandle,
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    id: String,
) -> Result<AppearanceDto, CodedError> {
    let result = async {
        let _change = accounts
            .begin_appearance_change()
            .ok_or(ProfileServiceError::Busy)?;
        service(&auth, &accounts)?.reset_skin(&id).await
    }
    .await;
    done("skin_reset", &result);
    let profile = result.map_err(ProfileServiceError::coded)?;
    Ok(finish(&app, &accounts, &profile).await)
}

/// Equips one of the capes the account owns, or hides the cape (`cape_id` is `None`).
#[tauri::command]
pub async fn set_account_cape(
    app: AppHandle,
    auth: State<'_, AuthState>,
    accounts: State<'_, AccountsState>,
    id: String,
    cape_id: Option<String>,
) -> Result<AppearanceDto, CodedError> {
    let op = if cape_id.is_some() {
        "cape_equip"
    } else {
        "cape_hide"
    };
    let result = async {
        let cape = cape_id.as_deref().map(CapeId::new).transpose()?;
        let _change = accounts
            .begin_appearance_change()
            .ok_or(ProfileServiceError::Busy)?;
        service(&auth, &accounts)?.set_cape(&id, cape).await
    }
    .await;
    done(op, &result);
    let profile = result.map_err(ProfileServiceError::coded)?;
    Ok(finish(&app, &accounts, &profile).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::IntoDeserializer;

    fn json_zero_array(len: usize) -> String {
        if len == 0 {
            return "[]".to_owned();
        }
        let mut s = String::with_capacity(len * 2 + 2);
        s.push('[');
        for i in 0..len {
            if i > 0 {
                s.push(',');
            }
            s.push('0');
        }
        s.push(']');
        s
    }

    #[test]
    fn bounded_skin_bytes_accepts_arrays_up_to_the_limit() {
        let small: BoundedSkinBytes = serde_json::from_str("[137,80,78,71]").unwrap();
        assert_eq!(small.into_bytes().unwrap(), vec![137, 80, 78, 71]);

        let exact: BoundedSkinBytes =
            serde_json::from_str(&json_zero_array(MAX_SKIN_FILE_BYTES)).unwrap();
        assert_eq!(exact.into_bytes().unwrap().len(), MAX_SKIN_FILE_BYTES);
    }

    #[test]
    fn bounded_skin_bytes_maps_modestly_oversized_arrays_to_too_large() {
        let plus_one: BoundedSkinBytes =
            serde_json::from_str(&json_zero_array(MAX_SKIN_FILE_BYTES + 1)).unwrap();
        assert_eq!(
            plus_one.into_bytes().unwrap_err(),
            ProfileServiceError::InvalidSkin(SkinProblem::TooLarge)
        );

        let edge: BoundedSkinBytes =
            serde_json::from_str(&json_zero_array(MAX_SKIN_FILE_BYTES + MAX_SKIN_DRAIN_EXTRA))
                .unwrap();
        assert_eq!(
            edge.into_bytes().unwrap_err(),
            ProfileServiceError::InvalidSkin(SkinProblem::TooLarge)
        );
    }

    #[test]
    fn bounded_skin_bytes_aborts_huge_arrays_and_bytes_slices_early() {
        let huge = json_zero_array(MAX_SKIN_FILE_BYTES + MAX_SKIN_DRAIN_EXTRA + 1024);
        let err = serde_json::from_str::<BoundedSkinBytes>(&huge).unwrap_err();
        assert!(err.to_string().contains("skin file exceeds maximum size"));

        let over_bytes = vec![0u8; MAX_SKIN_FILE_BYTES + 1];
        let de: serde::de::value::BytesDeserializer<serde::de::value::Error> =
            over_bytes.as_slice().into_deserializer();
        let over = <BoundedSkinBytes as serde::Deserialize>::deserialize(de).unwrap();
        assert_eq!(
            over.into_bytes().unwrap_err(),
            ProfileServiceError::InvalidSkin(SkinProblem::TooLarge)
        );

        let huge_bytes = vec![0u8; MAX_SKIN_FILE_BYTES + MAX_SKIN_DRAIN_EXTRA + 1];
        let de: serde::de::value::BytesDeserializer<serde::de::value::Error> =
            huge_bytes.as_slice().into_deserializer();
        assert!(<BoundedSkinBytes as serde::Deserialize>::deserialize(de).is_err());
    }
}
