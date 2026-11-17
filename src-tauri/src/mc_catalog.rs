//! The official Minecraft version catalog.
//!
//! Source: Mojang's own version manifest (`piston-meta.mojang.com`, v2). Nothing else is ever
//! consulted. The catalog covers every kind Mojang publishes (releases, snapshots, and the old
//! beta and alpha versions), so the launcher is never artificially limited to modern versions;
//! which kinds the UI shows by default is a presentation choice, not a restriction.
//!
//! Caching (portable, no AppData, no WebView storage): the last valid catalog is stored in
//! `cache/catalogs/version_manifest.json` inside the launcher folder, written atomically.
//! - A fresh enough cache (under an hour old) is returned without touching the network.
//! - Otherwise the network is tried. A response is only accepted if it parses strictly and does
//!   not look truncated (it must keep at least 90% of the versions already cached; Mojang only
//!   ever adds versions), and only then replaces the cache.
//! - If the network is unavailable, or the response is malformed or suspicious, the last valid
//!   cache is used and marked [`Origin::CacheStale`], with the reason. A bad response can never
//!   destroy a good cache because nothing is written until validation has passed.
//! - With neither network nor cache there is an explicit error; versions are never invented.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::net::{FetchError, Fetcher};
use crate::portable::fsutil::{atomic_write, read_bounded};
use crate::versions::McVersion;

pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const SCHEMA: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_CACHE_BYTES: u64 = 8 * 1024 * 1024;
/// How long a cached catalog counts as fresh enough to skip the network.
pub const FRESH_FOR_SECS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
}

impl VersionKind {
    fn from_manifest(t: &str) -> Option<Self> {
        Some(match t {
            "release" => Self::Release,
            "snapshot" => Self::Snapshot,
            "old_beta" => Self::OldBeta,
            "old_alpha" => Self::OldAlpha,
            _ => return None, // a kind we do not know: skipped, never guessed
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: McVersion,
    pub kind: VersionKind,
    /// ISO-8601 timestamp exactly as Mojang publishes it (display and sorting only).
    pub release_time: String,
    /// Where Mojang publishes this version's metadata JSON, and that file's SHA-1. Absent only in
    /// caches written before preparation existed (the next refresh fills them in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionCatalog {
    pub schema: u32,
    /// Unix seconds when this catalog was fetched from Mojang.
    pub fetched_at: i64,
    pub latest_release: Option<McVersion>,
    pub latest_snapshot: Option<McVersion>,
    /// Newest first, as the manifest lists them.
    pub entries: Vec<CatalogEntry>,
}

impl VersionCatalog {
    pub fn contains(&self, id: &McVersion) -> bool {
        self.entries.iter().any(|e| &e.id == id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("the version list is not in the expected format: {0}")]
    BadFormat(&'static str),
    #[error("no version list is available: {0}")]
    Unavailable(String),
}

pub struct Parsed {
    pub catalog: VersionCatalog,
    /// Entries ignored because their id or kind was not recognised.
    pub skipped: usize,
}

/// Parses and strictly validates Mojang's `version_manifest_v2.json`.
pub fn parse_manifest(body: &[u8], now: i64) -> Result<Parsed, CatalogError> {
    if body.len() > MAX_MANIFEST_BYTES {
        return Err(CatalogError::BadFormat("too large"));
    }
    let json: Value =
        serde_json::from_slice(body).map_err(|_| CatalogError::BadFormat("not JSON"))?;
    let versions = json
        .get("versions")
        .and_then(Value::as_array)
        .ok_or(CatalogError::BadFormat("missing versions"))?;

    let mut entries: Vec<CatalogEntry> = Vec::with_capacity(versions.len());
    let mut seen: HashSet<McVersion> = HashSet::with_capacity(versions.len());
    let mut skipped = 0;
    for v in versions {
        let parsed = (|| {
            let id = McVersion::new(v.get("id")?.as_str()?).ok()?;
            let kind = VersionKind::from_manifest(v.get("type")?.as_str()?)?;
            let release_time = v.get("releaseTime")?.as_str()?;
            (release_time.len() <= 40).then(|| CatalogEntry {
                id,
                kind,
                release_time: release_time.to_owned(),
                url: v
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|u| u.len() <= 300 && u.starts_with("https://"))
                    .map(str::to_owned),
                sha1: v
                    .get("sha1")
                    .and_then(Value::as_str)
                    .filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                    .map(str::to_ascii_lowercase),
            })
        })();
        match parsed {
            Some(e) if seen.insert(e.id.clone()) => entries.push(e),
            Some(_) => {} // duplicate id: the first (newest) wins
            None => skipped += 1,
        }
    }
    if !entries.iter().any(|e| e.kind == VersionKind::Release) {
        return Err(CatalogError::BadFormat("no release versions"));
    }
    let pick = |key: &str| {
        let id = McVersion::new(json.get("latest")?.get(key)?.as_str()?).ok()?;
        seen.contains(&id).then_some(id)
    };
    let (latest_release, latest_snapshot) = (pick("release"), pick("snapshot"));
    Ok(Parsed {
        catalog: VersionCatalog {
            schema: SCHEMA,
            fetched_at: now,
            latest_release,
            latest_snapshot,
            entries,
        },
        skipped,
    })
}

/// Reads the cache. Anything unreadable or inconsistent counts as "no cache" and is left alone.
pub fn load_cache(path: &Path) -> Option<VersionCatalog> {
    let bytes = read_bounded(path, MAX_CACHE_BYTES).ok()?;
    let c: VersionCatalog = serde_json::from_slice(&bytes).ok()?;
    (c.schema == SCHEMA && c.entries.iter().any(|e| e.kind == VersionKind::Release)).then_some(c)
}

/// Mojang only ever adds versions, so a "new" catalog that lost a tenth or more of what we
/// already hold is treated as a truncated or tampered response.
fn plausible_update(cached_len: usize, new: &VersionCatalog) -> bool {
    new.entries.len() * 10 >= cached_len * 9
}

/// Where the returned catalog came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Just downloaded from Mojang.
    Network,
    /// From the portable cache, recent enough that the network was not needed.
    CacheFresh,
    /// From the portable cache because the network was unavailable or its answer was rejected
    /// (or the cache is simply old). Possibly out of date.
    CacheStale,
}

#[derive(Debug)]
pub struct CatalogResult {
    pub catalog: VersionCatalog,
    pub origin: Origin,
    /// Why the network was not used, when `origin` is `CacheStale` because of a failure.
    pub network_error: Option<String>,
}

fn why(e: &FetchError) -> String {
    match e {
        FetchError::Network(_)
        | FetchError::Status(_)
        | FetchError::TooLarge(_)
        | FetchError::Local(_) => "Mojang could not be reached".to_owned(),
        FetchError::Policy(_) => "blocked by the network policy".to_owned(),
    }
}

/// Returns the best available catalog. `force` skips the freshness shortcut (a manual refresh).
pub async fn load<F: Fetcher>(
    fetcher: &F,
    cache_path: &Path,
    now: i64,
    force: bool,
) -> Result<CatalogResult, CatalogError> {
    let cache_in = cache_path.to_path_buf();
    let cached = tokio::task::spawn_blocking(move || load_cache(&cache_in))
        .await
        .ok()
        .flatten();

    if !force {
        if let Some(c) = &cached {
            let age = now - c.fetched_at;
            if (0..FRESH_FOR_SECS).contains(&age) {
                return Ok(CatalogResult {
                    catalog: c.clone(),
                    origin: Origin::CacheFresh,
                    network_error: None,
                });
            }
        }
    }

    let url = Url::parse(MANIFEST_URL).map_err(|_| CatalogError::BadFormat("bad manifest url"))?;
    let failure: String = match fetcher.get(&url, MAX_MANIFEST_BYTES).await {
        Ok(body) => {
            let cache_out = cache_path.to_path_buf();
            let cached_len = cached.as_ref().map(|c| c.entries.len());
            let outcome = tokio::task::spawn_blocking(
                move || -> Result<Option<VersionCatalog>, CatalogError> {
                    let p = parse_manifest(&body, now)?;
                    if cached_len.is_some_and(|old_len| !plausible_update(old_len, &p.catalog)) {
                        return Ok(None);
                    }
                    // Validated: only now may the cache change. A failed write does not fail the request.
                    if let Ok(bytes) = serde_json::to_vec(&p.catalog) {
                        let _ = atomic_write(&cache_out, &bytes);
                    }
                    Ok(Some(p.catalog))
                },
            )
            .await;

            match outcome {
                Ok(Ok(Some(catalog))) => {
                    return Ok(CatalogResult {
                        catalog,
                        origin: Origin::Network,
                        network_error: None,
                    });
                }
                Ok(Ok(None)) => "Mojang's answer looked incomplete".to_owned(),
                Ok(Err(_)) | Err(_) => "Mojang's answer was not understood".to_owned(),
            }
        }
        Err(e) => why(&e),
    };

    match cached {
        Some(c) => Ok(CatalogResult {
            catalog: c,
            origin: Origin::CacheStale,
            network_error: Some(failure),
        }),
        None => Err(CatalogError::Unavailable(format!(
            "{failure}, and no saved copy exists yet. Connect once to download the version list."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::testing::FakeFetcher;
    use std::fs;

    // Trimmed from the real manifest, including its awkward ids and every type.
    const MANIFEST: &str = r#"{
      "latest": {"release": "26.3", "snapshot": "26.4-snapshot-2"},
      "versions": [
        {"id":"26.4-snapshot-2","type":"snapshot","url":"https://piston-meta.mojang.com/v1/packages/a/26.4-snapshot-2.json","time":"2026-09-29T11:39:25+00:00","releaseTime":"2026-09-29T11:32:57+00:00","sha1":"a","complianceLevel":1},
        {"id":"26.3","type":"release","url":"https://piston-meta.mojang.com/v1/packages/b/26.3.json","time":"t","releaseTime":"2026-09-01T10:00:00+00:00","sha1":"b","complianceLevel":1},
        {"id":"1.21.4","type":"release","url":"u","time":"t","releaseTime":"2024-12-03T10:12:57+00:00","sha1":"c","complianceLevel":1},
        {"id":"1.14.2 Pre-Release 4","type":"snapshot","url":"u","time":"t","releaseTime":"2019-06-04T09:00:00+00:00","sha1":"d","complianceLevel":0},
        {"id":"3D Shareware v1.34","type":"snapshot","url":"u","time":"t","releaseTime":"2019-04-05T12:00:00+00:00","sha1":"e","complianceLevel":0},
        {"id":"b1.7.3","type":"old_beta","url":"u","time":"t","releaseTime":"2011-07-08T00:00:00+00:00","sha1":"f","complianceLevel":0},
        {"id":"rd-132211","type":"old_alpha","url":"u","time":"t","releaseTime":"2009-05-13T20:11:00+00:00","sha1":"g","complianceLevel":0}
      ]}"#;

    fn id(s: &str) -> McVersion {
        McVersion::new(s).unwrap()
    }

    fn cache_path(d: &tempfile::TempDir) -> std::path::PathBuf {
        d.path()
            .join("cache")
            .join("catalogs")
            .join("version_manifest.json")
    }

    #[test]
    fn parses_every_kind_and_the_awkward_real_ids() {
        let p = parse_manifest(MANIFEST.as_bytes(), 1000).unwrap();
        assert_eq!(p.skipped, 0);
        let c = p.catalog;
        assert_eq!(c.entries.len(), 7);
        let kinds: Vec<_> = c.entries.iter().map(|e| e.kind).collect();
        for k in [
            VersionKind::Release,
            VersionKind::Snapshot,
            VersionKind::OldBeta,
            VersionKind::OldAlpha,
        ] {
            assert!(kinds.contains(&k), "{k:?}");
        }
        assert!(
            c.contains(&id("1.14.2 Pre-Release 4")) && c.contains(&id("3D Shareware v1.34")),
            "ids with spaces are real versions"
        );
        assert!(
            c.contains(&id("rd-132211")) && c.contains(&id("b1.7.3")),
            "the oldest versions are not hidden"
        );
        assert_eq!(
            (c.latest_release, c.latest_snapshot),
            (Some(id("26.3")), Some(id("26.4-snapshot-2")))
        );
        assert_eq!(c.fetched_at, 1000);
    }

    #[test]
    fn unknown_kinds_and_invalid_ids_are_skipped_not_guessed_and_latest_must_exist() {
        let body = r#"{"latest":{"release":"9.9","snapshot":"1.21.4"},"versions":[
          {"id":"1.21.4","type":"release","releaseTime":"2024-12-03T10:12:57+00:00"},
          {"id":"2.0-weird","type":"hologram","releaseTime":"2030-01-01T00:00:00+00:00"},
          {"id":"../evil","type":"release","releaseTime":"2030-01-01T00:00:00+00:00"},
          {"id":"-rf","type":"release","releaseTime":"2030-01-01T00:00:00+00:00"},
          {"id":"1.21.4","type":"release","releaseTime":"2024-12-03T10:12:57+00:00"},
          {"id":"1.20","type":"release"}]}"#;
        let p = parse_manifest(body.as_bytes(), 1).unwrap();
        assert_eq!(p.catalog.entries.len(), 1, "duplicates collapse");
        assert_eq!(p.skipped, 4);
        assert_eq!(
            p.catalog.latest_release, None,
            "a 'latest' that is not in the list is dropped"
        );
        assert_eq!(p.catalog.latest_snapshot, Some(id("1.21.4")));
    }

    #[test]
    fn malformed_manifests_are_rejected() {
        for bad in [
            "",
            "not json",
            "[]",
            "{}",
            r#"{"versions":[]}"#,
            r#"{"versions":[{"id":"x1","type":"snapshot","releaseTime":"t"}]}"#,
        ] {
            assert!(parse_manifest(bad.as_bytes(), 1).is_err(), "{bad:?}");
        }
        assert!(parse_manifest(&vec![b' '; MAX_MANIFEST_BYTES + 1], 1).is_err());
    }

    #[tokio::test]
    async fn first_use_fetches_from_mojang_and_saves_the_portable_cache() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new();
        f.ok(MANIFEST_URL, MANIFEST);
        let r = load(&f, &cache_path(&d), 5000, false).await.unwrap();
        assert_eq!(r.origin, Origin::Network);
        assert_eq!(f.calls(), [MANIFEST_URL]);
        let saved = load_cache(&cache_path(&d)).unwrap();
        assert_eq!(saved, r.catalog);
        let text = fs::read_to_string(cache_path(&d)).unwrap();
        assert!(
            !text.contains(":\\") && !text.contains("C:/"),
            "no absolute paths in the cache file"
        );
    }

    #[tokio::test]
    async fn a_fresh_cache_skips_the_network_and_force_refreshes() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new();
        f.ok(MANIFEST_URL, MANIFEST);
        load(&f, &cache_path(&d), 5000, false).await.unwrap();
        let r = load(&f, &cache_path(&d), 5000 + 600, false).await.unwrap();
        assert_eq!(r.origin, Origin::CacheFresh);
        assert_eq!(
            f.calls().len(),
            1,
            "no second request inside the freshness window"
        );
        let r = load(&f, &cache_path(&d), 5000 + 600, true).await.unwrap();
        assert_eq!(r.origin, Origin::Network);
        assert_eq!(f.calls().len(), 2);
    }

    #[tokio::test]
    async fn offline_uses_the_last_valid_cache_and_says_so() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new();
        f.ok(MANIFEST_URL, MANIFEST);
        let first = load(&f, &cache_path(&d), 5000, false).await.unwrap();
        f.fail(MANIFEST_URL); // the network goes away
        let later = 5000 + FRESH_FOR_SECS + 10;
        let r = load(&f, &cache_path(&d), later, false).await.unwrap();
        assert_eq!(r.origin, Origin::CacheStale);
        assert_eq!(r.catalog, first.catalog);
        assert!(r.network_error.is_some());
        // The cache itself is untouched by the failed attempt.
        assert_eq!(load_cache(&cache_path(&d)).unwrap(), first.catalog);
    }

    #[tokio::test]
    async fn malformed_or_truncated_responses_never_destroy_a_valid_cache() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new();
        f.ok(MANIFEST_URL, MANIFEST);
        let good = load(&f, &cache_path(&d), 5000, false)
            .await
            .unwrap()
            .catalog;
        let before = fs::read(cache_path(&d)).unwrap();
        let later = 5000 + FRESH_FOR_SECS + 10;

        // Garbage, an error page, valid-but-wrong JSON, and a "valid" manifest that lost most versions.
        let truncated = r#"{"latest":{"release":"26.3"},"versions":[{"id":"26.3","type":"release","releaseTime":"2026-09-01T10:00:00+00:00"}]}"#;
        for evil in [
            "<html>captive portal</html>",
            "{\"versions\": 5}",
            "{}",
            truncated,
        ] {
            f.ok(MANIFEST_URL, evil);
            let r = load(&f, &cache_path(&d), later, false).await.unwrap();
            assert_eq!(r.origin, Origin::CacheStale, "{evil:?}");
            assert_eq!(r.catalog, good);
            assert!(r.network_error.is_some());
            assert_eq!(
                fs::read(cache_path(&d)).unwrap(),
                before,
                "cache bytes must be unchanged by {evil:?}"
            );
        }
    }

    #[tokio::test]
    async fn no_network_and_no_cache_is_an_explicit_error_not_invented_data() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new();
        f.fail(MANIFEST_URL);
        let e = load(&f, &cache_path(&d), 1, false).await.unwrap_err();
        assert!(matches!(e, CatalogError::Unavailable(_)));
        assert!(e.to_string().contains("Connect once"));
        assert!(!cache_path(&d).exists());
    }

    #[tokio::test]
    async fn a_corrupt_cache_file_is_ignored_and_replaced_by_a_good_download() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(cache_path(&d).parent().unwrap()).unwrap();
        fs::write(cache_path(&d), b"{ truncated").unwrap();
        let f = FakeFetcher::new();
        f.ok(MANIFEST_URL, MANIFEST);
        let r = load(&f, &cache_path(&d), 9000, false).await.unwrap();
        assert_eq!(r.origin, Origin::Network);
        assert!(load_cache(&cache_path(&d)).is_some());
    }

    #[tokio::test]
    async fn a_cache_dated_in_the_future_is_not_trusted_as_fresh() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new();
        f.ok(MANIFEST_URL, MANIFEST);
        load(&f, &cache_path(&d), 50_000, false).await.unwrap();
        // The clock went backwards (another PC): do not treat the cache as fresh.
        let r = load(&f, &cache_path(&d), 100, false).await.unwrap();
        assert_eq!(r.origin, Origin::Network);
    }

    #[tokio::test]
    async fn real_growth_is_accepted_even_when_it_adds_new_versions() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeFetcher::new();
        f.ok(MANIFEST_URL, MANIFEST);
        load(&f, &cache_path(&d), 1, false).await.unwrap();
        let grown = MANIFEST.replace(r#""versions": ["#, r#""versions": [{"id":"26.5","type":"release","url":"u","time":"t","releaseTime":"2026-12-01T00:00:00+00:00","sha1":"z","complianceLevel":1},"#);
        f.ok(MANIFEST_URL, grown);
        let r = load(&f, &cache_path(&d), 99_999, false).await.unwrap();
        assert_eq!(r.origin, Origin::Network);
        assert!(r.catalog.contains(&id("26.5")));
    }

    #[test]
    fn duplicate_entries_keep_first_occurrence_without_incrementing_skipped() {
        let body = r#"{
          "latest": {"release": "1.21.4", "snapshot": "24w10a"},
          "versions": [
            {"id":"1.21.4","type":"release","releaseTime":"2024-12-03T10:12:57+00:00","url":"https://piston-meta.mojang.com/first.json"},
            {"id":"24w10a","type":"snapshot","releaseTime":"2024-03-06T12:00:00+00:00","url":"https://piston-meta.mojang.com/snap-new.json"},
            {"id":"1.21.4","type":"release","releaseTime":"2020-01-01T00:00:00+00:00","url":"https://piston-meta.mojang.com/older-dup.json"},
            {"id":"24w10a","type":"snapshot","releaseTime":"2020-01-01T00:00:00+00:00","url":"https://piston-meta.mojang.com/snap-dup.json"}
          ]
        }"#;
        let parsed = parse_manifest(body.as_bytes(), 42).unwrap();
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.catalog.entries.len(), 2);
        assert_eq!(
            parsed.catalog.entries[0].url.as_deref(),
            Some("https://piston-meta.mojang.com/first.json")
        );
        assert_eq!(
            parsed.catalog.entries[1].url.as_deref(),
            Some("https://piston-meta.mojang.com/snap-new.json")
        );
        assert_eq!(parsed.catalog.latest_release, Some(id("1.21.4")));
        assert_eq!(parsed.catalog.latest_snapshot, Some(id("24w10a")));
    }

    #[test]
    fn load_cache_rejects_files_exceeding_max_cache_bytes() {
        let d = tempfile::tempdir().unwrap();
        let p = cache_path(&d);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let f = fs::File::create(&p).unwrap();
        f.set_len(MAX_CACHE_BYTES + 1).unwrap();
        drop(f);
        assert!(load_cache(&p).is_none());
    }
}
