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
    SkinVariant,
};
use crate::profiles::now_unix;

/// How long a change waits for the new texture to be cached before answering without it.
const SKIN_WAIT: Duration = Duration::from_secs(3);

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
    png: Vec<u8>,
) -> Result<AppearanceDto, CodedError> {
    let result = async {
        let skin = SkinPng::validate(png)?;
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
