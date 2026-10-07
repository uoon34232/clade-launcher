//! Portable encrypted credential vault (`data/vault.bin`).
//!
//! Why not Windows Credential Manager / DPAPI: both bind secrets to one Windows user on one
//! machine, so a launcher folder carried to another PC would lose every login. This vault
//! is a single file whose key is derived from a master passphrase, so it travels with the
//! folder.
//!
//! Scheme
//! - KDF: Argon2id (v1.3), memory-hard, parameters stored in the header and bounds-checked
//!   on read. 32-byte key, random 16-byte salt.
//! - AEAD: XChaCha20-Poly1305 with a fresh random 192-bit nonce for every save.
//! - The complete header (magic, version, KDF parameters, salt, nonce) is authenticated as
//!   associated data, so editing any of it makes decryption fail.
//! - The passphrase is never stored; the derived key lives only in memory while unlocked and
//!   is zeroized on lock/drop.
//!
//! File layout (all integers little-endian)
//! ```text
//!  0..8   magic "LNCHVLT\0"
//!  8      format version (1)
//!  9      kdf id (1 = Argon2id v1.3)
//! 10..14  m_cost (KiB)    14..18 t_cost    18..22 p_cost
//! 22..38  salt (16)       38..62 nonce (24)
//! 62..    ciphertext || 16-byte Poly1305 tag   (plaintext = JSON `VaultSecrets`)
//! ```
//!
//! A wrong passphrase and a tampered file are intentionally indistinguishable
//! (`VaultError::Authentication`).

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::portable::fsutil::atomic_write;

const MAGIC: &[u8; 8] = b"LNCHVLT\0";
const FORMAT_VERSION: u8 = 1;
const KDF_ARGON2ID_V13: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const HEADER_LEN: usize = 8 + 1 + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN;
const TAG_LEN: usize = 16;
const MAX_FILE_LEN: u64 = 16 * 1024 * 1024;

pub const MIN_PASSPHRASE_CHARS: usize = 8;

// Floors reject vaults that someone downgraded to a cheap KDF; ceilings stop a hostile file
// from making the launcher allocate gigabytes or spin for minutes.
const MIN_M_COST_KIB: u32 = if cfg!(test) { 8 } else { 19 * 1024 };
const MAX_M_COST_KIB: u32 = 1024 * 1024;
const MIN_T_COST: u32 = if cfg!(test) { 1 } else { 2 };
const MAX_T_COST: u32 = 16;
const MAX_P_COST: u32 = 8;

// The shipped defaults must always satisfy the production floors (checked at compile time,
// independent of the relaxed floors used by unit tests).
const _: () = assert!(KdfParams::DEFAULT.m_cost_kib >= 19 * 1024 && KdfParams::DEFAULT.t_cost >= 2);

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault io error: {0}")]
    Io(#[from] io::Error),
    /// There is no vault file at all (never created, or archived by "Start fresh").
    #[error("there is no vault file")]
    Missing,
    #[error("vault file is malformed or from an unsupported version")]
    BadFormat,
    #[error("vault key-derivation parameters are outside the accepted range")]
    UnacceptableKdf,
    /// Wrong passphrase *or* corrupted/tampered file; deliberately not distinguished.
    #[error("wrong passphrase, or the vault file is damaged")]
    Authentication,
    #[error("a vault already exists at this location")]
    AlreadyExists,
    #[error("passphrase must be at least {MIN_PASSPHRASE_CHARS} characters")]
    PassphraseTooShort,
    #[error("internal cryptography error")]
    Crypto,
    #[error("vault contents could not be serialized: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 1 lane: a few hundred ms on a modern PC, costly to brute-force.
    pub const DEFAULT: Self = Self {
        m_cost_kib: 64 * 1024,
        t_cost: 3,
        p_cost: 1,
    };

    fn validate(&self) -> Result<(), VaultError> {
        let ok = (MIN_M_COST_KIB..=MAX_M_COST_KIB).contains(&self.m_cost_kib)
            && (MIN_T_COST..=MAX_T_COST).contains(&self.t_cost)
            && (1..=MAX_P_COST).contains(&self.p_cost);
        if ok {
            Ok(())
        } else {
            Err(VaultError::UnacceptableKdf)
        }
    }

    fn derive(
        &self,
        passphrase: &[u8],
        salt: &[u8],
    ) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
        let params = Params::new(self.m_cost_kib, self.t_cost, self.p_cost, Some(KEY_LEN))
            .map_err(|_| VaultError::UnacceptableKdf)?;
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase, salt, key.as_mut())
            .map_err(|_| VaultError::Crypto)?;
        Ok(key)
    }
}

/// Secrets for one account. Never logged: `Debug` is redacted, memory is zeroized on drop.
#[derive(Clone, Default, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct AccountSecrets {
    /// Microsoft OAuth refresh token (the long-lived credential).
    pub ms_refresh_token: Option<String>,
    /// Short-lived tokens are cached so a prepared instance can launch offline-ish.
    pub minecraft_access_token: Option<String>,
    #[zeroize(skip)]
    pub minecraft_token_expires_at: Option<i64>,
}

impl fmt::Debug for AccountSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountSecrets(<redacted>)")
    }
}

/// No derive needed: dropping the map drops each `AccountSecrets`, which wipes itself.
#[derive(Default, Serialize, Deserialize)]
pub struct VaultSecrets {
    pub schema: u32,
    pub accounts: BTreeMap<String, AccountSecrets>,
}

impl fmt::Debug for VaultSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "VaultSecrets({} accounts, <redacted>)",
            self.accounts.len()
        )
    }
}

/// An unlocked vault. Dropping it (or calling `lock`) wipes the key and secrets from memory.
pub struct Vault {
    path: PathBuf,
    key: Zeroizing<[u8; KEY_LEN]>,
    salt: [u8; SALT_LEN],
    kdf: KdfParams,
    secrets: VaultSecrets,
}

struct Header {
    kdf: KdfParams,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
}

impl Header {
    fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..8].copy_from_slice(MAGIC);
        h[8] = FORMAT_VERSION;
        h[9] = KDF_ARGON2ID_V13;
        h[10..14].copy_from_slice(&self.kdf.m_cost_kib.to_le_bytes());
        h[14..18].copy_from_slice(&self.kdf.t_cost.to_le_bytes());
        h[18..22].copy_from_slice(&self.kdf.p_cost.to_le_bytes());
        h[22..38].copy_from_slice(&self.salt);
        h[38..62].copy_from_slice(&self.nonce);
        h
    }

    fn decode(bytes: &[u8]) -> Result<Self, VaultError> {
        if bytes.len() < HEADER_LEN + TAG_LEN
            || &bytes[0..8] != MAGIC
            || bytes[8] != FORMAT_VERSION
            || bytes[9] != KDF_ARGON2ID_V13
        {
            return Err(VaultError::BadFormat);
        }
        let u32_at =
            |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let kdf = KdfParams {
            m_cost_kib: u32_at(10),
            t_cost: u32_at(14),
            p_cost: u32_at(18),
        };
        kdf.validate()?;
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&bytes[22..38]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[38..62]);
        Ok(Self { kdf, salt, nonce })
    }
}

fn check_passphrase(p: &str) -> Result<(), VaultError> {
    if p.chars().count() < MIN_PASSPHRASE_CHARS {
        Err(VaultError::PassphraseTooShort)
    } else {
        Ok(())
    }
}

fn random<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|_| VaultError::Crypto)?;
    Ok(b)
}

fn cipher(key: &[u8; KEY_LEN]) -> Result<XChaCha20Poly1305, VaultError> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|_| VaultError::Crypto)
}

impl Vault {
    pub fn exists(path: &Path) -> bool {
        path.is_file()
    }

    pub fn create(path: &Path, passphrase: &str) -> Result<Self, VaultError> {
        Self::create_with(path, passphrase, KdfParams::DEFAULT)
    }

    pub(crate) fn create_with(
        path: &Path,
        passphrase: &str,
        kdf: KdfParams,
    ) -> Result<Self, VaultError> {
        check_passphrase(passphrase)?;
        kdf.validate()?;
        if path.exists() {
            return Err(VaultError::AlreadyExists);
        }
        let salt = random::<SALT_LEN>()?;
        let key = kdf.derive(passphrase.as_bytes(), &salt)?;
        let vault = Self {
            path: path.to_path_buf(),
            key,
            salt,
            kdf,
            secrets: VaultSecrets {
                schema: 1,
                accounts: BTreeMap::new(),
            },
        };
        vault.save()?;
        Ok(vault)
    }

    pub fn unlock(path: &Path, passphrase: &str) -> Result<Self, VaultError> {
        let missing = |e: io::Error| match e.kind() {
            io::ErrorKind::NotFound => VaultError::Missing,
            _ => VaultError::Io(e),
        };
        if fs::metadata(path).map_err(missing)?.len() > MAX_FILE_LEN {
            return Err(VaultError::BadFormat);
        }
        let bytes = fs::read(path).map_err(missing)?;
        let header = Header::decode(&bytes)?;
        let key = header.kdf.derive(passphrase.as_bytes(), &header.salt)?;
        let plaintext = cipher(&key)?
            .decrypt(
                &XNonce::try_from(&header.nonce[..]).map_err(|_| VaultError::Crypto)?,
                Payload {
                    msg: &bytes[HEADER_LEN..],
                    aad: &bytes[..HEADER_LEN],
                },
            )
            .map_err(|_| VaultError::Authentication)?;
        let plaintext = Zeroizing::new(plaintext);
        let secrets: VaultSecrets =
            serde_json::from_slice(&plaintext).map_err(|_| VaultError::BadFormat)?;
        Ok(Self {
            path: path.to_path_buf(),
            key,
            salt: header.salt,
            kdf: header.kdf,
            secrets,
        })
    }

    fn save(&self) -> Result<(), VaultError> {
        let header = Header {
            kdf: self.kdf,
            salt: self.salt,
            nonce: random::<NONCE_LEN>()?,
        };
        let header_bytes = header.encode();
        let plaintext = Zeroizing::new(serde_json::to_vec(&self.secrets)?);
        let ct = cipher(&self.key)?
            .encrypt(
                &XNonce::try_from(&header.nonce[..]).map_err(|_| VaultError::Crypto)?,
                Payload {
                    msg: &plaintext,
                    aad: &header_bytes,
                },
            )
            .map_err(|_| VaultError::Crypto)?;
        let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(&ct);

        // Keep one previous generation in case the new file is damaged by a yanked stick.
        if self.path.exists() {
            let _ = fs::copy(&self.path, self.path.with_extension("bin.bak"));
        }
        atomic_write(&self.path, &out)?;
        Ok(())
    }

    /// Who this vault is: its random salt, new for every vault that is created. A credential write
    /// that was prepared against one vault must never land in another (see `auth::ensure_fresh`).
    pub fn generation(&self) -> [u8; SALT_LEN] {
        self.salt
    }

    pub fn account(&self, id: &str) -> Option<&AccountSecrets> {
        self.secrets.accounts.get(id)
    }

    pub fn account_ids(&self) -> impl Iterator<Item = &str> {
        self.secrets.accounts.keys().map(String::as_str)
    }

    pub fn set_account(&mut self, id: &str, secrets: AccountSecrets) -> Result<(), VaultError> {
        self.secrets.accounts.insert(id.to_owned(), secrets);
        self.save()
    }

    pub fn remove_account(&mut self, id: &str) -> Result<(), VaultError> {
        self.secrets.accounts.remove(id);
        self.save()
    }

    /// Re-keys the vault under a new passphrase with a fresh salt.
    pub fn change_passphrase(&mut self, new_passphrase: &str) -> Result<(), VaultError> {
        check_passphrase(new_passphrase)?;
        let salt = random::<SALT_LEN>()?;
        let key = self.kdf.derive(new_passphrase.as_bytes(), &salt)?;
        self.salt = salt;
        self.key = key;
        self.save()
    }

    /// Explicitly wipe secrets and key (also happens on drop).
    pub fn lock(self) {
        drop(self);
    }
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Vault(<unlocked, redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };
    const PW: &str = "correct horse battery staple";

    fn secrets(token: &str) -> AccountSecrets {
        AccountSecrets {
            ms_refresh_token: Some(token.to_owned()),
            minecraft_access_token: Some(format!("mc-{token}")),
            minecraft_token_expires_at: Some(1_900_000_000),
        }
    }

    #[test]
    fn roundtrip_and_file_contains_no_plaintext() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let mut v = Vault::create_with(&p, PW, FAST).unwrap();
        v.set_account("acc1", secrets("SUPER-SECRET-REFRESH-TOKEN"))
            .unwrap();
        drop(v);

        let raw = fs::read(&p).unwrap();
        for needle in [
            "SUPER-SECRET-REFRESH-TOKEN",
            "mc-SUPER",
            "refresh",
            "acc1",
            PW,
        ] {
            assert!(
                !raw.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "plaintext {needle:?} leaked"
            );
        }

        let v = Vault::unlock(&p, PW).unwrap();
        assert_eq!(
            v.account("acc1").unwrap().ms_refresh_token.as_deref(),
            Some("SUPER-SECRET-REFRESH-TOKEN")
        );
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        Vault::create_with(&p, PW, FAST).unwrap();
        assert!(matches!(
            Vault::unlock(&p, "not the passphrase"),
            Err(VaultError::Authentication)
        ));
    }

    #[test]
    fn any_bit_flip_is_detected() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let mut v = Vault::create_with(&p, PW, FAST).unwrap();
        v.set_account("a", secrets("t")).unwrap();
        let original = fs::read(&p).unwrap();
        // Flip one bit in several regions: salt, nonce, ciphertext, tag. (Version/KDF-field flips
        // are rejected earlier as BadFormat/UnacceptableKdf, also fine.)
        for idx in [22, 40, HEADER_LEN + 1, original.len() - 1] {
            let mut t = original.clone();
            t[idx] ^= 0x01;
            fs::write(&p, &t).unwrap();
            assert!(Vault::unlock(&p, PW).is_err(), "flip at {idx} not detected");
        }
        // Changing KDF params (even to another valid value) breaks the header's authentication.
        let mut t = original.clone();
        t[14] = 2; // t_cost 1 -> 2
        fs::write(&p, &t).unwrap();
        assert!(Vault::unlock(&p, PW).is_err());
    }

    #[test]
    fn hostile_kdf_parameters_are_refused_before_any_work() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        Vault::create_with(&p, PW, FAST).unwrap();
        let mut t = fs::read(&p).unwrap();
        t[10..14].copy_from_slice(&u32::MAX.to_le_bytes()); // would be ~4 TiB of RAM
        fs::write(&p, &t).unwrap();
        assert!(matches!(
            Vault::unlock(&p, PW),
            Err(VaultError::UnacceptableKdf)
        ));
        fs::write(&p, b"not a vault").unwrap();
        assert!(matches!(Vault::unlock(&p, PW), Err(VaultError::BadFormat)));
    }

    #[test]
    fn nonce_is_fresh_on_every_save() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let mut v = Vault::create_with(&p, PW, FAST).unwrap();
        let n1 = fs::read(&p).unwrap()[38..62].to_vec();
        v.set_account("a", secrets("t")).unwrap();
        let n2 = fs::read(&p).unwrap()[38..62].to_vec();
        assert_ne!(n1, n2);
    }

    #[test]
    fn change_passphrase_rekeys() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let mut v = Vault::create_with(&p, PW, FAST).unwrap();
        v.set_account("a", secrets("tok")).unwrap();
        v.change_passphrase("a brand new passphrase").unwrap();
        drop(v);
        assert!(Vault::unlock(&p, PW).is_err());
        let v = Vault::unlock(&p, "a brand new passphrase").unwrap();
        assert!(v.account("a").is_some());
    }

    #[test]
    fn weak_passphrase_and_overwrite_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        assert!(matches!(
            Vault::create_with(&p, "short", FAST),
            Err(VaultError::PassphraseTooShort)
        ));
        Vault::create_with(&p, PW, FAST).unwrap();
        assert!(matches!(
            Vault::create_with(&p, PW, FAST),
            Err(VaultError::AlreadyExists)
        ));
    }

    #[test]
    fn vault_survives_moving_the_folder() {
        // The vault file stores no paths, so relocating the whole launcher folder
        // (new drive letter, new PC) must not affect unlocking.
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("A").join("Launcher").join("data");
        fs::create_dir_all(&a).unwrap();
        let mut v = Vault::create_with(&a.join("vault.bin"), PW, FAST).unwrap();
        v.set_account("a", secrets("moved-token")).unwrap();
        drop(v);
        let b = d.path().join("E_drive").join("USB stick");
        fs::create_dir_all(&b).unwrap();
        fs::rename(d.path().join("A").join("Launcher"), b.join("Launcher")).unwrap();
        let v = Vault::unlock(&b.join("Launcher").join("data").join("vault.bin"), PW).unwrap();
        assert_eq!(
            v.account("a").unwrap().ms_refresh_token.as_deref(),
            Some("moved-token")
        );
    }

    #[test]
    fn debug_output_never_contains_secrets() {
        let s = secrets("LEAKY");
        assert!(!format!("{s:?}").contains("LEAKY"));
    }
}
