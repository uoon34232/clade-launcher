//! Account *metadata* (`data/accounts.json`). Contains no tokens.
//!
//! Names and UUIDs are not secret, so this file stays readable and works even while the
//! vault is locked (the UI can list accounts before the passphrase is entered). Everything
//! sensitive lives in the encrypted vault, keyed by `AccountMeta::id`.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::portable::fsutil::atomic_write;

const SCHEMA: u32 = 1;

/// Why an account must sign in again. Portability cannot promise Microsoft never asks:
/// refresh tokens expire after inactivity, passwords change, sessions get revoked, and a new
/// PC can trigger a consent or risk prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReauthReason {
    RefreshTokenExpired,
    Revoked,
    ConsentRequired,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AccountStatus {
    Ready,
    /// Not an error state: the account stays listed and its instances stay playable
    /// offline-style; the UI just offers "Sign in again".
    NeedsReauth {
        reason: ReauthReason,
        since: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountMeta {
    pub id: String,
    pub username: String,
    pub minecraft_uuid: String,
    pub added_at: i64,
    pub last_used_at: Option<i64>,
    pub status: AccountStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsFile {
    pub schema: u32,
    pub active: Option<String>,
    pub accounts: Vec<AccountMeta>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            active: None,
            accounts: Vec::new(),
        }
    }
}

impl AccountsFile {
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Like [`Self::load`], but says where the list came from: `None` when there is no
    /// `accounts.json` at all, `Some` only for a file that exists and parsed. A caller that is about
    /// to do something destructive on the strength of "these are the accounts" must use this: a
    /// missing file is *not* an empty list, and a damaged one is an error.
    pub fn load_authoritative(path: &Path) -> io::Result<Option<Self>> {
        match fs::read(path) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        atomic_write(
            path,
            &serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )
    }

    pub fn mark_needs_reauth(&mut self, id: &str, reason: ReauthReason, now: i64) -> bool {
        match self.accounts.iter_mut().find(|a| a.id == id) {
            Some(a) => {
                a.status = AccountStatus::NeedsReauth { reason, since: now };
                true
            }
            None => false,
        }
    }

    /// Makes `id` the active account. The active account is global convenience state: it is
    /// never part of an instance, and no instance, mod, save or playtime is touched by it.
    pub fn select(&mut self, id: &str) -> bool {
        if self.accounts.iter().any(|a| a.id == id) {
            self.active = Some(id.to_owned());
            true
        } else {
            false
        }
    }

    /// Removes the account's metadata. If it was the active one, the first remaining account
    /// (or none) becomes active. The caller must also delete the account's vault secrets.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        if self.accounts.len() == before {
            return false;
        }
        if self.active.as_deref() == Some(id) {
            self.active = self.accounts.first().map(|a| a.id.clone());
        }
        true
    }

    /// The active account id, repaired: a dangling or missing value falls back to the first account.
    pub fn effective_active(&self) -> Option<&str> {
        self.active
            .as_deref()
            .filter(|id| self.accounts.iter().any(|a| a.id == *id))
            .or_else(|| self.accounts.first().map(|a| a.id.as_str()))
    }

    pub fn mark_ready(&mut self, id: &str) -> bool {
        match self.accounts.iter_mut().find(|a| a.id == id) {
            Some(a) => {
                a.status = AccountStatus::Ready;
                true
            }
            None => false,
        }
    }
}

/// Maps an OAuth error code from the Microsoft token endpoint to a re-auth reason.
/// Network failures are *not* re-auth conditions and must not be passed here.
pub fn classify_oauth_error(error_code: &str) -> ReauthReason {
    match error_code {
        "invalid_grant" => ReauthReason::Revoked, // expired, revoked or password changed
        "interaction_required" | "consent_required" | "login_required" => {
            ReauthReason::ConsentRequired
        }
        _ => ReauthReason::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str) -> AccountMeta {
        AccountMeta {
            id: id.into(),
            username: "Steve".into(),
            minecraft_uuid: "00000000-0000-0000-0000-000000000000".into(),
            added_at: 1,
            last_used_at: None,
            status: AccountStatus::Ready,
        }
    }

    #[test]
    fn missing_file_is_empty_and_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("accounts.json");
        assert!(AccountsFile::load(&p).unwrap().accounts.is_empty());
        let mut f = AccountsFile::default();
        f.accounts.push(meta("a"));
        f.active = Some("a".into());
        f.save(&p).unwrap();
        assert_eq!(AccountsFile::load(&p).unwrap(), f);
    }

    #[test]
    fn reauth_state_is_recorded_and_cleared_without_losing_the_account() {
        let mut f = AccountsFile::default();
        f.accounts.push(meta("a"));
        assert!(f.mark_needs_reauth("a", ReauthReason::Revoked, 99));
        assert_eq!(
            f.accounts[0].status,
            AccountStatus::NeedsReauth {
                reason: ReauthReason::Revoked,
                since: 99
            }
        );
        assert!(f.mark_ready("a"));
        assert!(!f.mark_needs_reauth("missing", ReauthReason::Unknown, 1));
    }

    #[test]
    fn selecting_and_removing_keeps_a_valid_active_account() {
        let mut f = AccountsFile::default();
        f.accounts.push(meta("a"));
        f.accounts.push(meta("b"));
        assert_eq!(f.effective_active(), Some("a"), "no choice yet: the first");
        assert!(f.select("b"));
        assert!(!f.select("zzz"));
        assert_eq!(f.active.as_deref(), Some("b"));
        assert!(f.remove("b"));
        assert_eq!(f.active.as_deref(), Some("a"));
        assert!(!f.remove("b"));
        assert!(f.remove("a"));
        assert_eq!((f.active.as_deref(), f.effective_active()), (None, None));
        f.active = Some("gone".into());
        f.accounts.push(meta("c"));
        assert_eq!(f.effective_active(), Some("c"), "dangling id is repaired");
    }

    #[test]
    fn file_has_no_token_fields() {
        let mut f = AccountsFile::default();
        f.accounts.push(meta("a"));
        let json = serde_json::to_string(&f).unwrap().to_lowercase();
        assert!(!json.contains("token"));
    }

    #[test]
    fn oauth_errors_map_to_reauth_reasons() {
        assert_eq!(classify_oauth_error("invalid_grant"), ReauthReason::Revoked);
        assert_eq!(
            classify_oauth_error("interaction_required"),
            ReauthReason::ConsentRequired
        );
        assert_eq!(classify_oauth_error("server_error"), ReauthReason::Unknown);
    }
}
