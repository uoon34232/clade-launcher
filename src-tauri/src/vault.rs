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

use crate::portable::fsutil::{atomic_write, is_link, read_bounded};

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
pub const MAX_PASSPHRASE_BYTES: usize = 4096;

// Floors reject vaults that someone downgraded to a cheap KDF; ceilings stop a hostile file
// from making the launcher allocate gigabytes or spin for minutes.
const MIN_M_COST_KIB: u32 = if cfg!(test) { 8 } else { 19 * 1024 };
const MAX_M_COST_KIB: u32 = 256 * 1024;
const MIN_T_COST: u32 = if cfg!(test) { 1 } else { 2 };
const MAX_T_COST: u32 = 6;
const MAX_P_COST: u32 = 4;

// The shipped defaults must always satisfy the production floors (checked at compile time,
// independent of the relaxed floors used by unit tests).
const _: () = assert!(KdfParams::DEFAULT.m_cost_kib >= 19 * 1024 && KdfParams::DEFAULT.t_cost >= 2);
const _: () = assert!(KdfParams::LOW_MEMORY.m_cost_kib >= 19 * 1024);
const _: () = assert!(KdfParams::LOW_MEMORY.t_cost >= 2);

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
    #[error("passphrase must be at most {MAX_PASSPHRASE_BYTES} bytes")]
    PassphraseTooLong,
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

    /// 32 MiB, 3 passes, 1 lane: used on <= 4 GiB machines to avoid paging pressure while staying
    /// above the 19 MiB production floor.
    pub const LOW_MEMORY: Self = Self {
        m_cost_kib: 32 * 1024,
        t_cost: 3,
        p_cost: 1,
    };

    pub(crate) fn for_memory_mb(total_mb: Option<u64>) -> Self {
        if total_mb.is_some_and(|m| m <= 4300) {
            Self::LOW_MEMORY
        } else {
            Self::DEFAULT
        }
    }

    pub fn for_current_machine() -> Self {
        Self::for_memory_mb(crate::system::total_memory_mb())
    }

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
        if passphrase.len() > MAX_PASSPHRASE_BYTES {
            return Err(VaultError::PassphraseTooLong);
        }
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

/// Decrypted vault payload; dropping wipes both account-id keys and `AccountSecrets` values.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct VaultSecrets {
    pub schema: u32,
    pub accounts: BTreeMap<String, AccountSecrets>,
}

impl Zeroize for VaultSecrets {
    fn zeroize(&mut self) {
        self.schema = 0;
        for (mut id, mut secret) in std::mem::take(&mut self.accounts) {
            id.zeroize();
            secret.zeroize();
        }
    }
}

impl Drop for VaultSecrets {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl ZeroizeOnDrop for VaultSecrets {}

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
    if p.len() > MAX_PASSPHRASE_BYTES {
        Err(VaultError::PassphraseTooLong)
    } else if p.chars().count() < MIN_PASSPHRASE_CHARS {
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

fn bak_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".bak");
    PathBuf::from(os)
}

fn write_encrypted(
    path: &Path,
    kdf: KdfParams,
    salt: &[u8; SALT_LEN],
    key: &[u8; KEY_LEN],
    secrets: &VaultSecrets,
    overwrite_bak: bool,
) -> Result<(), VaultError> {
    let header = Header {
        kdf,
        salt: *salt,
        nonce: random::<NONCE_LEN>()?,
    };
    let header_bytes = header.encode();
    let mut plaintext = Zeroizing::new(Vec::with_capacity(1024));
    serde_json::to_writer(&mut *plaintext, secrets)?;
    let ct = cipher(key)?
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

    let bak = bak_path(path);
    for candidate in [path, &bak] {
        match is_link(candidate) {
            Ok(true) => {
                return Err(VaultError::Io(io::Error::other(
                    "refusing to write vault through a link",
                )));
            }
            Ok(false) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(VaultError::Io(e)),
        }
    }
    // Keep one previous generation in case the new file is damaged by a yanked stick, unless
    // revoking credentials or rotating the passphrase (where `.bak` is overwritten atomically instead).
    if !overwrite_bak && fs::symlink_metadata(path).is_ok() {
        match fs::read(path) {
            Ok(prev) => {
                if let Err(e) = atomic_write(&bak, &prev) {
                    tracing::warn!(error = %e, "could not update backup vault");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not read previous vault for backup"),
        }
    }
    atomic_write(path, &out)?;
    if overwrite_bak && fs::symlink_metadata(&bak).is_ok() {
        if let Err(write_err) = atomic_write(&bak, &out) {
            if let Err(remove_err) = fs::remove_file(&bak) {
                tracing::warn!(
                    error = %remove_err,
                    "could not remove stale backup vault after overwrite failure"
                );
                return Err(VaultError::Io(write_err));
            }
        }
    }
    Ok(())
}

impl Vault {
    pub fn exists(path: &Path) -> bool {
        path.is_file()
    }

    pub fn derive_empty(path: &Path, passphrase: &str) -> Result<Self, VaultError> {
        Self::derive_empty_with(path, passphrase, KdfParams::for_current_machine())
    }

    pub(crate) fn derive_empty_with(
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
        Ok(Self {
            path: path.to_path_buf(),
            key,
            salt,
            kdf,
            secrets: VaultSecrets {
                schema: 1,
                accounts: BTreeMap::new(),
            },
        })
    }

    pub fn persist_new(&self) -> Result<(), VaultError> {
        if self.path.exists() {
            return Err(VaultError::AlreadyExists);
        }
        write_encrypted(
            &self.path,
            self.kdf,
            &self.salt,
            &self.key,
            &self.secrets,
            false,
        )
    }

    pub fn create(path: &Path, passphrase: &str) -> Result<Self, VaultError> {
        Self::create_with(path, passphrase, KdfParams::for_current_machine())
    }

    pub(crate) fn create_with(
        path: &Path,
        passphrase: &str,
        kdf: KdfParams,
    ) -> Result<Self, VaultError> {
        let v = Self::derive_empty_with(path, passphrase, kdf)?;
        v.persist_new()?;
        Ok(v)
    }

    pub fn unlock(path: &Path, passphrase: &str) -> Result<Self, VaultError> {
        let bytes = read_bounded(path, MAX_FILE_LEN).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => VaultError::Missing,
            io::ErrorKind::InvalidData => VaultError::BadFormat,
            _ => VaultError::Io(e),
        })?;
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
        if secrets.schema != 1 {
            return Err(VaultError::BadFormat);
        }
        Ok(Self {
            path: path.to_path_buf(),
            key,
            salt: header.salt,
            kdf: header.kdf,
            secrets,
        })
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
        let mut staged = self.secrets.clone();
        staged.accounts.insert(id.to_owned(), secrets);
        write_encrypted(&self.path, self.kdf, &self.salt, &self.key, &staged, false)?;
        self.secrets = staged;
        Ok(())
    }

    pub fn remove_account(&mut self, id: &str) -> Result<(), VaultError> {
        let mut staged = self.secrets.clone();
        staged.accounts.remove(id);
        write_encrypted(&self.path, self.kdf, &self.salt, &self.key, &staged, true)?;
        self.secrets = staged;
        Ok(())
    }

    /// Re-keys the vault under a new passphrase with a fresh salt.
    pub fn change_passphrase(&mut self, new_passphrase: &str) -> Result<(), VaultError> {
        check_passphrase(new_passphrase)?;
        let salt = random::<SALT_LEN>()?;
        let key = self.kdf.derive(new_passphrase.as_bytes(), &salt)?;
        write_encrypted(&self.path, self.kdf, &salt, &key, &self.secrets, true)?;
        self.salt = salt;
        self.key = key;
        Ok(())
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
    fn lowered_kdf_ceilings_reject_excessive_costs() {
        assert!(KdfParams {
            m_cost_kib: 256 * 1024,
            t_cost: 6,
            p_cost: 4,
        }
        .validate()
        .is_ok());

        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        Vault::create_with(&p, PW, FAST).unwrap();
        let template = fs::read(&p).unwrap();

        for bad in [
            KdfParams {
                m_cost_kib: 257 * 1024,
                t_cost: 1,
                p_cost: 1,
            },
            KdfParams {
                m_cost_kib: 8,
                t_cost: 7,
                p_cost: 1,
            },
            KdfParams {
                m_cost_kib: 8,
                t_cost: 1,
                p_cost: 5,
            },
        ] {
            assert!(matches!(
                Vault::create_with(&d.path().join("other.bin"), PW, bad),
                Err(VaultError::UnacceptableKdf)
            ));
            let mut tampered = template.clone();
            tampered[10..14].copy_from_slice(&bad.m_cost_kib.to_le_bytes());
            tampered[14..18].copy_from_slice(&bad.t_cost.to_le_bytes());
            tampered[18..22].copy_from_slice(&bad.p_cost.to_le_bytes());
            fs::write(&p, &tampered).unwrap();
            assert!(matches!(
                Vault::unlock(&p, PW),
                Err(VaultError::UnacceptableKdf)
            ));
        }
    }

    #[test]
    fn max_passphrase_bytes_enforced_in_create_change_and_unlock() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let exact = "a".repeat(MAX_PASSPHRASE_BYTES);
        let over = "a".repeat(MAX_PASSPHRASE_BYTES + 1);

        // Missing file and bad header are still reported before passphrase derivation.
        assert!(matches!(Vault::unlock(&p, &over), Err(VaultError::Missing)));
        assert!(matches!(
            Vault::create_with(&p, &over, FAST),
            Err(VaultError::PassphraseTooLong)
        ));
        assert!(!p.exists());

        fs::write(&p, b"not a vault").unwrap();
        assert!(matches!(
            Vault::unlock(&p, &over),
            Err(VaultError::BadFormat)
        ));
        fs::remove_file(&p).unwrap();

        let mut v = Vault::create_with(&p, &exact, FAST).unwrap();
        assert!(matches!(
            v.change_passphrase(&over),
            Err(VaultError::PassphraseTooLong)
        ));
        drop(v);

        assert!(matches!(
            Vault::unlock(&p, &over),
            Err(VaultError::PassphraseTooLong)
        ));
        let mut v = Vault::unlock(&p, &exact).unwrap();
        v.change_passphrase(PW).unwrap();
        drop(v);
        assert!(Vault::unlock(&p, PW).is_ok());
    }

    #[test]
    fn unsupported_secrets_schema_is_rejected_as_bad_format() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let salt = [7u8; SALT_LEN];
        let key = FAST.derive(PW.as_bytes(), &salt).unwrap();

        for bad_schema in [0u32, 2, 99] {
            let secrets = VaultSecrets {
                schema: bad_schema,
                accounts: BTreeMap::new(),
            };
            write_encrypted(&p, FAST, &salt, &key, &secrets, false).unwrap();
            assert!(
                matches!(Vault::unlock(&p, PW), Err(VaultError::BadFormat)),
                "schema {bad_schema} was not rejected"
            );
        }
    }

    #[test]
    fn failed_disk_write_prevents_split_brain() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data");
        let aside = d.path().join("data-aside");
        fs::create_dir_all(&data).unwrap();
        let p = data.join("vault.bin");

        let mut v = Vault::create_with(&p, PW, FAST).unwrap();
        v.set_account("acc1", secrets("tok1")).unwrap();
        let orig_gen = v.generation();

        // Replace parent directory with a regular file so `atomic_write` fails immediately.
        fs::rename(&data, &aside).unwrap();
        fs::write(&data, b"blocker").unwrap();

        assert!(matches!(
            v.set_account("acc2", secrets("tok2")),
            Err(VaultError::Io(_))
        ));
        assert!(v.account("acc2").is_none());
        assert_eq!(
            v.account("acc1").unwrap().ms_refresh_token.as_deref(),
            Some("tok1")
        );
        assert_eq!(v.generation(), orig_gen);

        assert!(matches!(v.remove_account("acc1"), Err(VaultError::Io(_))));
        assert!(v.account("acc1").is_some());

        assert!(matches!(
            v.change_passphrase("another strong passphrase"),
            Err(VaultError::Io(_))
        ));
        assert_eq!(v.generation(), orig_gen);

        // Restore directory and confirm on-disk vault matches in-memory state.
        fs::remove_file(&data).unwrap();
        fs::rename(&aside, &data).unwrap();
        assert!(Vault::unlock(&p, "another strong passphrase").is_err());
        let on_disk = Vault::unlock(&p, PW).unwrap();
        assert_eq!(on_disk.generation(), orig_gen);
        assert_eq!(
            on_disk.account("acc1").unwrap().ms_refresh_token.as_deref(),
            Some("tok1")
        );
        assert!(on_disk.account("acc2").is_none());
    }

    #[test]
    fn bak_naming_and_overwrite_on_remove_and_rekey() {
        let d = tempfile::tempdir().unwrap();

        // Non-.bin and extensionless vault filenames append `.bak` cleanly.
        for name in ["vault.dat", "vault"] {
            let p = d.path().join(name);
            let expected_bak = d.path().join(format!("{name}.bak"));
            assert_eq!(bak_path(&p), expected_bak);
            let mut v = Vault::create_with(&p, PW, FAST).unwrap();
            assert!(!expected_bak.exists());
            v.set_account("x", secrets("tok")).unwrap();
            assert!(expected_bak.is_file());
            assert!(!d.path().join("vault.bin.bak").exists());
        }

        // Removing an account or rotating passphrase overwrites `.bak` with fresh ciphertext.
        let p = d.path().join("vault.bin");
        let bak = bak_path(&p);
        let mut v = Vault::create_with(&p, PW, FAST).unwrap();
        v.set_account("a", secrets("REMOVED-SECRET")).unwrap();
        v.set_account("b", secrets("KEPT-SECRET")).unwrap();
        assert!(
            Vault::unlock(&bak, PW).unwrap().account("a").is_some(),
            "before removal, .bak holds previous generation"
        );

        v.remove_account("a").unwrap();
        let bak_after_remove = Vault::unlock(&bak, PW).unwrap();
        assert!(
            bak_after_remove.account("a").is_none(),
            "removed account secret must not linger in .bak"
        );
        assert_eq!(
            bak_after_remove
                .account("b")
                .unwrap()
                .ms_refresh_token
                .as_deref(),
            Some("KEPT-SECRET")
        );

        let new_pw = "rotated horse battery staple";
        v.change_passphrase(new_pw).unwrap();
        let old_bak_res = Vault::unlock(&bak, PW);
        assert!(
            matches!(old_bak_res, Err(VaultError::Authentication)),
            "old passphrase must no longer unlock .bak"
        );
        let bak_after_rekey = Vault::unlock(&bak, new_pw).unwrap();
        assert_eq!(
            bak_after_rekey
                .account("b")
                .unwrap()
                .ms_refresh_token
                .as_deref(),
            Some("KEPT-SECRET")
        );
    }

    #[test]
    fn adaptive_kdf_scales_for_low_memory_machines() {
        assert_eq!(KdfParams::for_memory_mb(Some(2048)), KdfParams::LOW_MEMORY);
        assert_eq!(KdfParams::for_memory_mb(Some(4096)), KdfParams::LOW_MEMORY);
        assert_eq!(KdfParams::for_memory_mb(Some(4300)), KdfParams::LOW_MEMORY);
        assert_eq!(KdfParams::for_memory_mb(Some(4301)), KdfParams::DEFAULT);
        assert_eq!(KdfParams::for_memory_mb(Some(16_384)), KdfParams::DEFAULT);
        assert_eq!(KdfParams::for_memory_mb(None), KdfParams::DEFAULT);
        assert_eq!(KdfParams::LOW_MEMORY.m_cost_kib, 32 * 1024);
        assert_eq!(KdfParams::DEFAULT.m_cost_kib, 64 * 1024);
    }

    #[test]
    fn two_phase_derive_empty_and_persist_new() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let v = Vault::derive_empty_with(&p, PW, FAST).unwrap();
        assert!(!p.exists(), "derive_empty_with must not touch disk");

        // If another writer creates the file first, persist_new refuses without overwriting.
        fs::write(&p, b"concurrent").unwrap();
        assert!(matches!(v.persist_new(), Err(VaultError::AlreadyExists)));
        assert_eq!(fs::read(&p).unwrap(), b"concurrent");

        fs::remove_file(&p).unwrap();
        v.persist_new().unwrap();
        assert!(p.is_file());
        assert!(!bak_path(&p).exists());
        let unlocked = Vault::unlock(&p, PW).unwrap();
        assert_eq!(unlocked.generation(), v.generation());
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

    #[test]
    fn write_encrypted_backs_up_atomically_and_propagates_overwrite_bak_failure() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.bin");
        let bak = bak_path(&p);
        let mut v = Vault::create_with(&p, PW, FAST).unwrap();
        v.set_account("a", secrets("first")).unwrap();
        assert!(bak.is_file(), "normal save creates atomic .bak backup");
        assert!(Vault::unlock(&bak, PW).is_ok());

        // Replace `.bak` with a non-empty directory so neither atomic overwrite nor `remove_file`
        // can clear it when rotating credentials (`overwrite_bak = true`).
        fs::remove_file(&bak).unwrap();
        fs::create_dir(&bak).unwrap();
        fs::write(bak.join("pinned"), b"block-remove").unwrap();
        let err = v.change_passphrase("new stronger passphrase");
        assert!(
            matches!(err, Err(VaultError::Io(_))),
            "overwrite_bak failure must be reported when stale backup cannot be removed: {err:?}"
        );
    }
}
