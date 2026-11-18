//! The account avatar's data: the current Minecraft skin texture, cached inside the Clade folder.
//!
//! The only source is what Minecraft services report for the account's own profile
//! (`GET /minecraft/profile`: the ACTIVE entry of `skins`). That is a public texture address on
//! `textures.minecraft.net`; no third-party skin service is ever contacted and no web page is read.
//!
//! The frontend cuts the head and the outer hat layer out of the texture itself, so this module
//! only fetches, validates and stores the PNG; it never decodes pixels. Everything lives in
//! `cache/skins/` (portable, never AppData):
//! - `<uuid>.want`: the texture id the profile reports now (the validated hex id only; the address
//!   is rebuilt from it, so nothing else the profile sent is ever stored);
//! - `<uuid>.png` + `<uuid>.have`: the texture last downloaded and the texture id it came from.
//!
//! A texture id and a public address are no secrets, and no token is ever written here. A cache
//! entry is replaced atomically when the profile reports a different skin, and anything missing,
//! damaged or unsupported simply means "no skin": the UI then draws its neutral placeholder. No
//! failure in here may ever reach, let alone break, sign-in or the account list.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use url::Url;

use crate::net::Fetcher;
use crate::portable::fsutil::{atomic_write, read_bounded};

/// The one host Mojang serves skin textures from (the profile reports `http://`; it is upgraded).
const TEXTURE_HOST: &str = "textures.minecraft.net";
/// A skin is a few kilobytes; even a 512x512 one stays far below this.
const MAX_BYTES: usize = 256 * 1024;
/// Largest edge accepted (a 64x64 skin scaled up to 16x).
const MAX_EDGE: u32 = 1024;

/// At most this many skins are fetched at once; further accounts wait for the next round.
const MAX_PARALLEL: usize = 8;

/// The address of a texture id (`parse_texture` accepted the id before it was ever stored).
fn texture_url(id: &str) -> Option<Texture> {
    parse_texture(&format!("https://{TEXTURE_HOST}/texture/{id}"))
}

/// A texture address checked to be exactly `https://textures.minecraft.net/texture/<hex id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub url: Url,
    pub id: String,
}

/// Accepts only the documented shape of a skin address: host `textures.minecraft.net`, path
/// `/texture/<16 to 128 hex digits>`, no credentials, port, query or fragment.
pub fn parse_texture(raw: &str) -> Option<Texture> {
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str()? != TEXTURE_HOST
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let id = url.path().strip_prefix("/texture/")?.to_owned();
    if !(16..=128).contains(&id.len()) || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    url.set_scheme("https").ok()?;
    Some(Texture {
        url,
        id: id.to_ascii_lowercase(),
    })
}

/// Width and height of a PNG that looks like a Minecraft skin (64 wide times a whole factor, and
/// as tall as wide, or half as tall for the old format), judged from its header only.
pub fn skin_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() > MAX_BYTES
        || png.len() < 33
        || png[..8] != SIGNATURE
        || png[8..12] != [0, 0, 0, 13]
        || &png[12..16] != b"IHDR"
    {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    let ok = (64..=MAX_EDGE).contains(&w) && w % 64 == 0 && (h == w || h == w / 2);
    ok.then_some((w, h))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (the frontend only ever shows `data:image/png;base64,` images).
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The skin cache of all accounts (`cache/skins/`).
#[derive(Debug, Clone)]
pub struct SkinCache {
    dir: PathBuf,
}

impl SkinCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// File stem for an account: its lowercase dashed UUID, nothing else (never a path piece).
    fn stem(uuid: &str) -> Option<String> {
        crate::ms_auth::dashed_uuid(uuid)
    }

    fn file(&self, uuid: &str, ext: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("{}.{ext}", Self::stem(uuid)?)))
    }

    fn read_text(&self, uuid: &str, ext: &str) -> Option<String> {
        let bytes = read_bounded(&self.file(uuid, ext)?, 1024).ok()?;
        let t = String::from_utf8(bytes).ok()?;
        Some(t.trim().to_owned())
    }

    /// Notes what the profile reports as the account's active skin (`None`: the default skin, so
    /// any cached texture is dropped). Metadata only; never touches the network.
    pub fn record(&self, uuid: &str, reported: Option<&str>) {
        let Some(want) = self.file(uuid, "want") else {
            return;
        };
        match reported.and_then(parse_texture) {
            Some(t) => {
                if self.read_text(uuid, "want").as_deref() != Some(t.id.as_str()) {
                    if let Err(e) = atomic_write(&want, t.id.as_bytes()) {
                        tracing::info!(error = %e, "could not note the skin address");
                    }
                }
            }
            None => self.forget(uuid),
        }
    }

    /// Downloads the texture reported by [`Self::record`] unless the cached one already is it.
    /// Best effort by design: every failure (offline, refused, not a skin, disk) leaves the
    /// previous cache entry alone and is only counted in the log, never carried further.
    pub async fn sync<F: Fetcher>(&self, fetcher: &F, uuid: &str) {
        let Some(want) = self.read_text(uuid, "want").and_then(|id| texture_url(&id)) else {
            return;
        };
        let png = self.file(uuid, "png");
        let have = self.file(uuid, "have");
        let (Some(png), Some(have)) = (png, have) else {
            return;
        };
        if png.is_file() && self.read_text(uuid, "have").as_deref() == Some(want.id.as_str()) {
            return;
        }
        let bytes = match fetcher.get(&want.url, MAX_BYTES).await {
            Ok(b) => b,
            Err(e) => {
                tracing::info!(error = %e, "the skin could not be downloaded");
                return;
            }
        };
        if skin_size(&bytes).is_none() {
            tracing::info!("the skin texture was not a usable image");
            return;
        }
        // The texture first, then the note saying which one it is: a crash in between leaves a
        // texture that does not match the note, which is simply fetched again.
        let want_id = want.id;
        let saved = tokio::task::spawn_blocking(move || {
            atomic_write(&png, &bytes).and_then(|()| atomic_write(&have, want_id.as_bytes()))
        })
        .await;
        match saved {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::info!(error = %e, "the skin could not be saved"),
            Err(e) => tracing::info!(error = %e, "the skin save task failed"),
        }
    }

    /// A skin is noted but the cached texture is missing or is another one (worth a download).
    pub fn pending(&self, uuid: &str) -> bool {
        match self.read_text(uuid, "want").and_then(|id| texture_url(&id)) {
            Some(want) => self.read_text(uuid, "have").as_deref() != Some(want.id.as_str()),
            None => false,
        }
    }

    /// Fetches the waiting skins of several accounts at the same time, all within ONE overall
    /// `budget`: the wait never grows with the number of accounts (at most [`MAX_PARALLEL`] run at
    /// once). A skin that is not done in time is still pending, never half-written. Returns
    /// whether anything was waiting.
    pub async fn sync_all<F: Fetcher>(
        &self,
        fetcher: &F,
        uuids: &[String],
        budget: Duration,
    ) -> bool {
        let waiting: Vec<&String> = uuids
            .iter()
            .filter(|u| self.pending(u))
            .take(MAX_PARALLEL)
            .collect();
        if waiting.is_empty() {
            return false;
        }
        let all = futures_util::future::join_all(waiting.iter().map(|u| self.sync(fetcher, u)));
        let _ = tokio::time::timeout(budget, all).await;
        true
    }

    /// The account's current skin as an inline PNG, or `None` (no skin, stale, damaged).
    pub fn data_uri(&self, uuid: &str) -> Option<String> {
        let want = texture_url(&self.read_text(uuid, "want")?)?;
        if self.read_text(uuid, "have")? != want.id {
            return None; // the profile reports a newer skin than the one cached
        }
        let bytes = read_bounded(&self.file(uuid, "png")?, MAX_BYTES as u64).ok()?;
        skin_size(&bytes)?;
        Some(format!("data:image/png;base64,{}", base64(&bytes)))
    }

    /// Deletes everything cached for an account (it was removed, or its skin is the default one).
    pub fn forget(&self, uuid: &str) {
        for ext in ["want", "have", "png"] {
            if let Some(f) = self.file(uuid, ext) {
                let _ = fs::remove_file(f);
            }
        }
    }
}

#[cfg(test)]
mod tests;
