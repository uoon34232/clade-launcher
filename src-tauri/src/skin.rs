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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use url::Url;

use crate::net::Fetcher;
use crate::portable::fsutil::{atomic_write, read_bounded, remove_file_with_retry};

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

fn lock_mutex<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SyncOutcome {
    /// Nothing needed (invalid UUID, no `.want`, already cached, or `.want` changed/cleared mid-fetch).
    Skipped,
    /// `<uuid>.png` and `<uuid>.have` were updated on disk.
    Updated,
    /// Download, image validation, or disk save failed for `want_id`.
    Failed { want_id: String },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SyncBatchReport {
    pub waiting: bool,
    pub changed: bool,
    pub succeeded: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// The skin cache of all accounts (`cache/skins/`).
#[derive(Debug, Clone)]
pub struct SkinCache {
    dir: PathBuf,
    locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
}

impl SkinCache {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// File stem for an account: its lowercase dashed UUID, nothing else (never a path piece).
    fn stem(uuid: &str) -> Option<String> {
        crate::ms_auth::dashed_uuid(uuid)
    }

    fn path_for(&self, stem: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{stem}.{ext}"))
    }

    #[cfg(test)]
    fn file(&self, uuid: &str, ext: &str) -> Option<PathBuf> {
        Some(self.path_for(&Self::stem(uuid)?, ext))
    }

    fn lock_for(&self, stem: &str) -> Arc<Mutex<()>> {
        let mut guard = lock_mutex(&self.locks);
        if let Some(existing) = guard.get(stem).and_then(Weak::upgrade) {
            return existing;
        }
        if guard.len() > 64 {
            guard.retain(|_, v| v.strong_count() > 0);
        }
        let created = Arc::new(Mutex::new(()));
        guard.insert(stem.to_owned(), Arc::downgrade(&created));
        created
    }

    fn read_text_path(path: &Path) -> Option<String> {
        let bytes = read_bounded(path, 1024).ok()?;
        let t = String::from_utf8(bytes).ok()?;
        Some(t.trim().to_owned())
    }

    fn pending_want_stem(&self, stem: &str) -> Option<Texture> {
        let lock = self.lock_for(stem);
        let _guard = lock_mutex(&lock);
        let want =
            Self::read_text_path(&self.path_for(stem, "want")).and_then(|id| texture_url(&id))?;
        let have_ok =
            Self::read_text_path(&self.path_for(stem, "have")).as_deref() == Some(want.id.as_str());
        let png_ok = self.path_for(stem, "png").is_file();
        if have_ok && png_ok {
            None
        } else {
            Some(want)
        }
    }

    fn commit_skin(&self, stem: &str, want_id: &str, bytes: &[u8]) -> std::io::Result<bool> {
        let lock = self.lock_for(stem);
        let _guard = lock_mutex(&lock);
        // Re-check `.want` after the network fetch so a newer `record` or a `forget` is never
        // overwritten or resurrected by a slow or in-flight download.
        let current_want =
            Self::read_text_path(&self.path_for(stem, "want")).and_then(|id| texture_url(&id));
        if current_want.as_ref().map(|t| t.id.as_str()) != Some(want_id) {
            return Ok(false);
        }
        let png = self.path_for(stem, "png");
        let have = self.path_for(stem, "have");
        if png.is_file() && Self::read_text_path(&have).as_deref() == Some(want_id) {
            return Ok(false);
        }
        // The texture first, then the note saying which one it is: a crash in between leaves a
        // texture that does not match the note, which is simply fetched again.
        atomic_write(&png, bytes)?;
        atomic_write(&have, want_id.as_bytes())?;
        Ok(true)
    }

    fn forget_stem(&self, stem: &str) {
        let lock = self.lock_for(stem);
        let _guard = lock_mutex(&lock);
        for ext in ["want", "have", "png"] {
            let f = self.path_for(stem, ext);
            if let Err(e) = remove_file_with_retry(&f) {
                tracing::info!(error = %e, "could not remove cached skin file");
            }
        }
    }

    /// Notes what the profile reports as the account's active skin (`None`: the default skin, so
    /// any cached texture is dropped). Metadata only; never touches the network.
    pub fn record(&self, uuid: &str, reported: Option<&str>) {
        let Some(stem) = Self::stem(uuid) else {
            return;
        };
        let Some(t) = reported.and_then(parse_texture) else {
            self.forget_stem(&stem);
            return;
        };
        let lock = self.lock_for(&stem);
        let _guard = lock_mutex(&lock);
        let want = self.path_for(&stem, "want");
        if Self::read_text_path(&want).as_deref() != Some(t.id.as_str()) {
            if let Err(e) = atomic_write(&want, t.id.as_bytes()) {
                tracing::info!(error = %e, "could not note the skin address");
            }
        }
    }

    async fn sync_planned_with_progress<F: Fetcher>(
        &self,
        fetcher: &F,
        stem: &str,
        want: Texture,
        record: impl FnOnce(&SyncOutcome) + Send + 'static,
    ) -> SyncOutcome {
        let bytes = match fetcher.get(&want.url, MAX_BYTES).await {
            Ok(b) => b,
            Err(e) => {
                tracing::info!(error = %e, "the skin could not be downloaded");
                let out = SyncOutcome::Failed { want_id: want.id };
                record(&out);
                return out;
            }
        };
        if skin_size(&bytes).is_none() {
            tracing::info!("the skin texture was not a usable image");
            let out = SyncOutcome::Failed { want_id: want.id };
            record(&out);
            return out;
        }
        let cache = self.clone();
        let stem = stem.to_owned();
        let want_id = want.id;
        let want_id_for_task = want_id.clone();
        let saved = tokio::task::spawn_blocking(move || {
            let outcome = match cache.commit_skin(&stem, &want_id_for_task, &bytes) {
                Ok(true) => SyncOutcome::Updated,
                Ok(false) => SyncOutcome::Skipped,
                Err(e) => {
                    tracing::info!(error = %e, "the skin could not be saved");
                    SyncOutcome::Failed {
                        want_id: want_id_for_task,
                    }
                }
            };
            record(&outcome);
            outcome
        })
        .await;
        match saved {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::info!(error = %e, "the skin save task failed");
                SyncOutcome::Failed { want_id }
            }
        }
    }

    /// Downloads the texture reported by [`Self::record`] unless the cached one already is it.
    /// Best effort by design: every failure (offline, refused, not a skin, disk) leaves the
    /// previous cache entry alone and is only counted in the log, never carried further.
    pub async fn sync<F: Fetcher>(&self, fetcher: &F, uuid: &str) {
        let Some(stem) = Self::stem(uuid) else {
            return;
        };
        let cache = self.clone();
        let stem_for_plan = stem.clone();
        let plan = tokio::task::spawn_blocking(move || cache.pending_want_stem(&stem_for_plan))
            .await
            .ok()
            .flatten();
        let Some(want) = plan else {
            return;
        };
        let _ = self
            .sync_planned_with_progress(fetcher, &stem, want, |_| {})
            .await;
    }

    /// A skin is noted but the cached texture is missing or is another one (worth a download).
    pub fn pending(&self, uuid: &str) -> bool {
        Self::stem(uuid).is_some_and(|stem| self.pending_want_stem(&stem).is_some())
    }

    pub(crate) fn pending_unless_skipped(
        &self,
        uuid: &str,
        skip: &HashMap<String, String>,
    ) -> bool {
        let Some(stem) = Self::stem(uuid) else {
            return false;
        };
        let Some(want) = self.pending_want_stem(&stem) else {
            return false;
        };
        !skip.get(&stem).is_some_and(|id| id == &want.id)
    }

    pub(crate) async fn sync_batch<F: Fetcher>(
        &self,
        fetcher: &F,
        uuids: &[String],
        budget: Duration,
        skip: &HashMap<String, String>,
    ) -> SyncBatchReport {
        #[derive(Clone)]
        enum Attempt {
            Pending,
            Done(SyncOutcome),
        }

        struct Progress {
            waiting: AtomicBool,
            changed: AtomicBool,
            attempts: Mutex<Vec<(String, String, Attempt)>>,
        }

        let progress = Arc::new(Progress {
            waiting: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            attempts: Mutex::new(Vec::new()),
        });

        let cache = self.clone();
        let owned_uuids = uuids.to_vec();
        let skip_owned = skip.clone();
        let progress_for_work = Arc::clone(&progress);

        let work = async move {
            let waiting = tokio::task::spawn_blocking(move || {
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for u in owned_uuids {
                    let Some(stem) = Self::stem(&u) else {
                        continue;
                    };
                    if !seen.insert(stem.clone()) {
                        continue;
                    }
                    if let Some(want) = cache.pending_want_stem(&stem) {
                        if skip_owned.get(&stem).is_some_and(|id| id == &want.id) {
                            continue;
                        }
                        out.push((stem, want));
                        if out.len() >= MAX_PARALLEL {
                            break;
                        }
                    }
                }
                out
            })
            .await
            .unwrap_or_default();

            if waiting.is_empty() {
                return;
            }

            progress_for_work.waiting.store(true, Ordering::Release);
            {
                let mut attempts = lock_mutex(&progress_for_work.attempts);
                *attempts = waiting
                    .iter()
                    .map(|(stem, want)| (stem.clone(), want.id.clone(), Attempt::Pending))
                    .collect();
            }

            futures_util::future::join_all(waiting.into_iter().enumerate().map(
                |(idx, (stem, want))| {
                    let progress_for_item = Arc::clone(&progress_for_work);
                    async move {
                        self.sync_planned_with_progress(fetcher, &stem, want, move |outcome| {
                            if matches!(outcome, SyncOutcome::Updated) {
                                progress_for_item.changed.store(true, Ordering::Release);
                            }
                            let mut attempts = lock_mutex(&progress_for_item.attempts);
                            if let Some(slot) = attempts.get_mut(idx) {
                                slot.2 = Attempt::Done(outcome.clone());
                            }
                        })
                        .await;
                    }
                },
            ))
            .await;
        };

        let _ = tokio::time::timeout(budget, work).await;

        let waiting = progress.waiting.load(Ordering::Acquire);
        let changed = progress.changed.load(Ordering::Acquire);
        let mut succeeded = Vec::new();
        let mut failed = Vec::new();
        for (stem, want_id, state) in lock_mutex(&progress.attempts).iter() {
            match state {
                Attempt::Done(SyncOutcome::Updated) => succeeded.push(stem.clone()),
                Attempt::Done(SyncOutcome::Skipped) => {}
                Attempt::Done(SyncOutcome::Failed { want_id: failed_id }) => {
                    failed.push((stem.clone(), failed_id.clone()));
                }
                Attempt::Pending => {
                    failed.push((stem.clone(), want_id.clone()));
                }
            }
        }

        SyncBatchReport {
            waiting,
            changed,
            succeeded,
            failed,
        }
    }

    /// Fetches the waiting skins of several accounts at the same time, all within ONE overall
    /// `budget`: the wait never grows with the number of accounts (at most [`MAX_PARALLEL`] run at
    /// once). A skin that is not done in time is still pending, never half-written. Returns
    /// whether anything was waiting (see [`Self::sync_all_changed`] to check whether any cache
    /// entry changed on disk).
    pub async fn sync_all<F: Fetcher>(
        &self,
        fetcher: &F,
        uuids: &[String],
        budget: Duration,
    ) -> bool {
        self.sync_batch(fetcher, uuids, budget, &HashMap::new())
            .await
            .waiting
    }

    /// Like [`Self::sync_all`], returning whether at least one skin cache entry was updated on
    /// disk (`.have` and `.png` written).
    pub async fn sync_all_changed<F: Fetcher>(
        &self,
        fetcher: &F,
        uuids: &[String],
        budget: Duration,
    ) -> bool {
        self.sync_batch(fetcher, uuids, budget, &HashMap::new())
            .await
            .changed
    }

    /// The account's current skin as an inline PNG, or `None` (no skin, stale, damaged).
    pub fn data_uri(&self, uuid: &str) -> Option<String> {
        let stem = Self::stem(uuid)?;
        let lock = self.lock_for(&stem);
        let _guard = lock_mutex(&lock);
        let want = texture_url(&Self::read_text_path(&self.path_for(&stem, "want"))?)?;
        if Self::read_text_path(&self.path_for(&stem, "have"))? != want.id {
            return None; // the profile reports a newer skin than the one cached
        }
        let bytes = read_bounded(&self.path_for(&stem, "png"), MAX_BYTES as u64).ok()?;
        skin_size(&bytes)?;
        Some(format!("data:image/png;base64,{}", base64(&bytes)))
    }

    /// Deletes everything cached for an account (it was removed, or its skin is the default one).
    pub fn forget(&self, uuid: &str) {
        if let Some(stem) = Self::stem(uuid) {
            self.forget_stem(&stem);
        }
    }
}

#[cfg(test)]
use std::fs;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::net::testing::FakeFetcher;
    use crate::net::FetchError;
    use tokio::sync::Notify;

    const UUID: &str = "00000000-0000-4000-8000-00000000a001";
    const ID_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const ID_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn url(id: &str) -> String {
        format!("https://textures.minecraft.net/texture/{id}")
    }

    fn png(w: u32, h: u32, tag: u8) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, tag]);
        b
    }

    struct GatedFetcher {
        gate_id: String,
        entered: Arc<Notify>,
        release: Arc<Notify>,
        inner: FakeFetcher,
    }

    impl Fetcher for GatedFetcher {
        fn get(
            &self,
            url: &Url,
            max_bytes: usize,
        ) -> impl std::future::Future<Output = Result<Vec<u8>, FetchError>> + Send {
            let should_wait = url.path().ends_with(&self.gate_id);
            let entered = Arc::clone(&self.entered);
            let release = Arc::clone(&self.release);
            let inner_fut = self.inner.get(url, max_bytes);
            async move {
                if should_wait {
                    entered.notify_one();
                    release.notified().await;
                }
                inner_fut.await
            }
        }
    }

    #[test]
    fn pending_detects_missing_png_and_sync_redownloads_it() {
        let d = tempfile::tempdir().unwrap();
        let c = SkinCache::new(d.path().join("cache").join("skins"));
        let f = FakeFetcher::new();
        f.ok(&url(ID_A), png(64, 64, 1));
        c.record(UUID, Some(&url(ID_A)));
        assert!(c.pending(UUID));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        assert!(rt.block_on(c.sync_all_changed(&f, &[UUID.to_owned()], Duration::from_secs(5))));
        assert!(!c.pending(UUID));

        fs::remove_file(c.file(UUID, "png").unwrap()).unwrap();
        assert!(
            c.pending(UUID),
            "missing .png must make pending() return true"
        );
        assert!(c.data_uri(UUID).is_none());

        assert!(rt.block_on(c.sync_all_changed(&f, &[UUID.to_owned()], Duration::from_secs(5))));
        assert!(!c.pending(UUID));
        assert!(c.data_uri(UUID).is_some());
    }

    #[test]
    fn sync_all_changed_reports_false_when_downloads_fail_or_time_out() {
        let d = tempfile::tempdir().unwrap();
        let c = SkinCache::new(d.path().join("cache").join("skins"));
        let f = FakeFetcher::new();
        c.record(UUID, Some(&url(ID_A)));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        // Offline/error: waiting is true (legacy sync_all), changed is false.
        assert!(rt.block_on(c.sync_all(&f, &[UUID.to_owned()], Duration::from_secs(5))));
        assert!(!rt.block_on(c.sync_all_changed(&f, &[UUID.to_owned()], Duration::from_secs(5))));

        // Invalid image payload: changed is still false.
        f.ok(&url(ID_A), b"not-a-png".as_slice());
        assert!(!rt.block_on(c.sync_all_changed(&f, &[UUID.to_owned()], Duration::from_secs(5))));
    }

    #[test]
    fn stale_sync_cannot_overwrite_newer_skin_or_resurrect_forgotten_cache() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            // 1. Newer `record` + `sync` supersedes an in-flight stale download.
            let d = tempfile::tempdir().unwrap();
            let c = SkinCache::new(d.path().join("cache").join("skins"));
            let inner = FakeFetcher::new();
            inner.ok(&url(ID_A), png(64, 64, 1));
            inner.ok(&url(ID_B), png(128, 128, 2));
            let fetcher = GatedFetcher {
                gate_id: ID_A.to_owned(),
                entered: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
                inner,
            };

            c.record(UUID, Some(&url(ID_A)));
            let c1 = c.clone();
            let f1 = &fetcher;
            let stale_task = tokio::spawn({
                let entered = Arc::clone(&fetcher.entered);
                let release = Arc::clone(&fetcher.release);
                let inner = FakeFetcher::new();
                inner.ok(&url(ID_A), png(64, 64, 1));
                let gated = GatedFetcher {
                    gate_id: ID_A.to_owned(),
                    entered,
                    release,
                    inner,
                };
                async move {
                    c1.sync(&gated, UUID).await;
                }
            });

            fetcher.entered.notified().await;
            c.record(UUID, Some(&url(ID_B)));
            c.sync(f1, UUID).await;
            let uri_b = c.data_uri(UUID).unwrap();

            fetcher.release.notify_one();
            stale_task.await.unwrap();

            assert_eq!(c.data_uri(UUID).as_deref(), Some(uri_b.as_str()));
            assert_eq!(
                fs::read_to_string(c.file(UUID, "have").unwrap()).unwrap(),
                ID_B
            );

            // 2. `forget` while a download is in flight leaves zero files behind.
            let d2 = tempfile::tempdir().unwrap();
            let c2 = SkinCache::new(d2.path().join("cache").join("skins"));
            let entered2 = Arc::new(Notify::new());
            let release2 = Arc::new(Notify::new());
            c2.record(UUID, Some(&url(ID_A)));
            let task2 = tokio::spawn({
                let c2 = c2.clone();
                let entered2 = Arc::clone(&entered2);
                let release2 = Arc::clone(&release2);
                let inner2 = FakeFetcher::new();
                inner2.ok(&url(ID_A), png(64, 64, 1));
                let gated2 = GatedFetcher {
                    gate_id: ID_A.to_owned(),
                    entered: entered2,
                    release: release2,
                    inner: inner2,
                };
                async move {
                    c2.sync(&gated2, UUID).await;
                }
            });

            entered2.notified().await;
            c2.forget(UUID);
            release2.notify_one();
            task2.await.unwrap();

            assert!(c2.data_uri(UUID).is_none());
            assert!(!c2.pending(UUID));
            assert!(!c2.file(UUID, "want").unwrap().exists());
            assert!(!c2.file(UUID, "have").unwrap().exists());
            assert!(!c2.file(UUID, "png").unwrap().exists());
        });
    }

    #[test]
    fn sync_batch_reports_partial_success_and_records_timed_out_entries_as_failed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let d = tempfile::tempdir().unwrap();
            let c = SkinCache::new(d.path().join("cache").join("skins"));
            let uuid_ok = "00000000-0000-4000-8000-00000000a001".to_owned();
            let uuid_hung = "00000000-0000-4000-8000-00000000b002".to_owned();
            c.record(&uuid_ok, Some(&url(ID_B)));
            c.record(&uuid_hung, Some(&url(ID_A)));

            let inner = FakeFetcher::new();
            inner.ok(&url(ID_B), png(64, 64, 7));
            let fetcher = GatedFetcher {
                gate_id: ID_A.to_owned(),
                entered: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()), // never released before budget expires
                inner,
            };

            let report = c
                .sync_batch(
                    &fetcher,
                    &[uuid_ok.clone(), uuid_hung.clone()],
                    Duration::from_millis(80),
                    &HashMap::new(),
                )
                .await;
            assert!(report.waiting);
            assert!(report.changed);
            assert_eq!(report.succeeded, vec![uuid_ok.clone()]);
            assert_eq!(report.failed, vec![(uuid_hung.clone(), ID_A.to_owned())]);
            assert!(c.data_uri(&uuid_ok).is_some());
            assert!(c.data_uri(&uuid_hung).is_none());

            // Skipping `uuid_hung` for `ID_A` causes sync_batch to see nothing eligible.
            let mut skip = HashMap::new();
            skip.insert(uuid_hung.clone(), ID_A.to_owned());
            let second = c
                .sync_batch(
                    &fetcher,
                    &[uuid_ok, uuid_hung],
                    Duration::from_millis(80),
                    &skip,
                )
                .await;
            assert!(!second.waiting);
            assert!(!second.changed);
        });
    }
}
