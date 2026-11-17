//! Which mod loaders can actually be used with a given Minecraft version, and which loader
//! versions. Sourced only from each loader's official catalog (see `loaders`), never guessed.
//!
//! For each loader the answer is one of:
//! - `Ok`: at least one compatible version exists (with a recommended default chosen by the
//!   same Recommended, Stable, Latest, Prerelease preference as `loaders::choose_auto`);
//! - `None`: the official catalog was read and has no compatible version for this Minecraft version;
//! - `Unavailable`: the catalog could not be read and no saved copy exists, so compatibility is
//!   unknown and the loader must not be offered.
//!
//! Caching (portable, in `cache/catalogs/`), always validated first: nothing is saved unless the
//! official response parsed successfully. When the network fails, the saved copy is used and
//! flagged stale.
//! - Fabric and Quilt answer per Minecraft version, so the small validated offer list is saved
//!   per version (and pruned so the cache cannot grow without bound).
//! - Forge and NeoForge publish ONE catalog for every Minecraft version, so that catalog itself
//!   is saved (raw) and filtered for the requested version every time it is read. Saving the
//!   already-filtered result would silently hand one version's loaders to another version.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::loaders::{self, LoaderKind, OfferedVersion};
use crate::mc_catalog::Origin;
use crate::net::Fetcher;
use crate::portable::fsutil::{atomic_write, read_bounded, remove_file_with_retry};
use crate::versions::{LoaderVersion, McVersion};

const SCHEMA: u32 = 1;
const MAX_CACHE_BYTES: u64 = 4 * 1024 * 1024;
/// How many per-Minecraft-version loader lists to keep on disk.
const KEEP_PER_MC_FILES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    None,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoaderOptions {
    pub loader: LoaderKind,
    pub status: Status,
    pub versions: Vec<OfferedVersion>,
    pub recommended: Option<LoaderVersion>,
    pub origin: Option<Origin>,
    pub error: Option<String>,
}

/// Fabric/Quilt: the server already filtered by Minecraft version.
#[derive(Serialize, Deserialize)]
struct CachedOffers {
    schema: u32,
    fetched_at: i64,
    offers: Vec<OfferedVersion>,
}

/// Forge/NeoForge: the whole official catalog, valid for every Minecraft version.
#[derive(Serialize, Deserialize)]
struct CachedRaw {
    schema: u32,
    fetched_at: i64,
    body: serde_json::Value,
}

fn is_global(kind: LoaderKind) -> bool {
    matches!(kind, LoaderKind::Forge | LoaderKind::NeoForge)
}

/// File name (inside `cache/catalogs/`) for one loader and Minecraft version. Forge and NeoForge
/// publish a single catalog for every Minecraft version; Fabric and Quilt one per version.
fn cache_name(kind: LoaderKind, mc: &McVersion) -> String {
    match kind {
        LoaderKind::Forge => "loader-forge.json".to_owned(),
        LoaderKind::NeoForge => "loader-neoforge.json".to_owned(),
        LoaderKind::Fabric => format!("loader-fabric--{mc}.json"),
        LoaderKind::Quilt => format!("loader-quilt--{mc}.json"),
    }
}

fn read_cache(path: &Path) -> Option<CachedOffers> {
    let bytes = read_bounded(path, MAX_CACHE_BYTES).ok()?;
    let c: CachedOffers = serde_json::from_slice(&bytes).ok()?;
    (c.schema == SCHEMA).then_some(c)
}

fn read_raw(path: &Path) -> Option<CachedRaw> {
    let bytes = read_bounded(path, MAX_CACHE_BYTES).ok()?;
    let c: CachedRaw = serde_json::from_slice(&bytes).ok()?;
    (c.schema == SCHEMA).then_some(c)
}

/// Keeps only the newest `keep` files whose names start with `prefix`.
fn prune(dir: &Path, prefix: &str, keep: usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(keep) {
        let _ = remove_file_with_retry(&p);
    }
}

fn build(
    kind: LoaderKind,
    offers: Vec<OfferedVersion>,
    origin: Origin,
    error: Option<String>,
) -> LoaderOptions {
    let recommended = loaders::choose_auto(&offers).map(|o| o.version.clone());
    LoaderOptions {
        loader: kind,
        status: if offers.is_empty() {
            Status::None
        } else {
            Status::Ok
        },
        versions: offers,
        recommended,
        origin: Some(origin),
        error,
    }
}

/// Compatible versions of one loader for `mc`, from the official catalog or its saved copy.
pub async fn options_for<F: Fetcher>(
    fetcher: &F,
    cache_dir: &Path,
    kind: LoaderKind,
    mc: &McVersion,
    now: i64,
) -> LoaderOptions {
    let path = cache_dir.join(cache_name(kind, mc));
    let failure = match loaders::catalog_url(kind, mc) {
        Err(e) => e.to_string(),
        Ok(url) => match fetcher.get(&url, loaders::MAX_CATALOG_BYTES).await {
            Ok(body) => match loaders::parse_catalog(kind, mc, &body) {
                Ok(offers) => {
                    // Validated: now (and only now) save. Saving never fails the request.
                    let serialized = if is_global(kind) {
                        serde_json::from_slice::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|value| {
                                serde_json::to_vec(&CachedRaw {
                                    schema: SCHEMA,
                                    fetched_at: now,
                                    body: value,
                                })
                                .ok()
                            })
                    } else {
                        serde_json::to_vec(&CachedOffers {
                            schema: SCHEMA,
                            fetched_at: now,
                            offers: offers.clone(),
                        })
                        .ok()
                    };
                    if let Some(bytes) = serialized {
                        let write_path = path.clone();
                        let prune_dir = cache_dir.to_path_buf();
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = atomic_write(&write_path, &bytes);
                            if !is_global(kind) {
                                prune(
                                    &prune_dir,
                                    if kind == LoaderKind::Fabric {
                                        "loader-fabric--"
                                    } else {
                                        "loader-quilt--"
                                    },
                                    KEEP_PER_MC_FILES,
                                );
                            }
                        })
                        .await;
                    }
                    return build(kind, offers, Origin::Network, None);
                }
                Err(_) => "the loader's answer was not understood".to_owned(),
            },
            Err(_) => "the loader's site could not be reached".to_owned(),
        },
    };

    let mc_clone = mc.clone();
    let saved: Option<Vec<OfferedVersion>> = tokio::task::spawn_blocking(move || {
        if is_global(kind) {
            // Filter the saved catalog for THIS Minecraft version, exactly as a live answer would be.
            read_raw(&path)
                .and_then(|c| serde_json::to_vec(&c.body).ok())
                .and_then(|bytes| loaders::parse_catalog(kind, &mc_clone, &bytes).ok())
        } else {
            read_cache(&path).map(|c| c.offers)
        }
    })
    .await
    .ok()
    .flatten();
    match saved {
        Some(offers) => build(kind, offers, Origin::CacheStale, Some(failure)),
        None => LoaderOptions {
            loader: kind,
            status: Status::Unavailable,
            versions: Vec::new(),
            recommended: None,
            origin: None,
            error: Some(failure),
        },
    }
}

/// All four loaders for one Minecraft version, fetched concurrently, in a stable display order.
pub async fn all_options<F: Fetcher>(
    fetcher: &F,
    cache_dir: &Path,
    mc: &McVersion,
    now: i64,
) -> Vec<LoaderOptions> {
    let (fabric, forge, neoforge, quilt) = futures::join!(
        options_for(fetcher, cache_dir, LoaderKind::Fabric, mc, now),
        options_for(fetcher, cache_dir, LoaderKind::Forge, mc, now),
        options_for(fetcher, cache_dir, LoaderKind::NeoForge, mc, now),
        options_for(fetcher, cache_dir, LoaderKind::Quilt, mc, now),
    );
    vec![fabric, forge, neoforge, quilt]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::testing::FakeFetcher;

    const FABRIC: &str = r#"[{"loader":{"version":"0.19.5","stable":true}},{"loader":{"version":"0.20.0-beta.1","stable":false}},{"loader":{"version":"0.18.0","stable":true}}]"#;
    const QUILT: &str =
        r#"[{"loader":{"version":"0.24.0"}},{"loader":{"version":"0.20.0-beta.9"}}]"#;
    const FORGE: &str = r#"{"promos":{"1.21.4-latest":"54.1.18","1.21.4-recommended":"54.1.14","1.20.1-latest":"47.4.26","1.20.1-recommended":"47.4.10"}}"#;
    const NEOFORGE: &str = r#"{"versions":["21.4.10","21.4.158","21.4.9-beta","20.4.237"]}"#;

    fn mc(s: &str) -> McVersion {
        McVersion::new(s).unwrap()
    }

    fn routes(f: &FakeFetcher) {
        f.ok(
            "https://meta.fabricmc.net/v2/versions/loader/1.21.4",
            FABRIC,
        );
        f.ok("https://meta.fabricmc.net/v2/versions/loader/1.20.1", "[]");
        f.ok("https://meta.quiltmc.org/v3/versions/loader/1.21.4", QUILT);
        f.ok("https://meta.quiltmc.org/v3/versions/loader/1.20.1", QUILT);
        f.ok(
            "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json",
            FORGE,
        );
        f.ok(
            "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge",
            NEOFORGE,
        );
    }

    fn dir() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cache").join("catalogs");
        (d, p)
    }

    #[tokio::test]
    async fn only_loaders_with_real_compatible_versions_are_ok_and_recommendations_follow_the_channel_rules(
    ) {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        let all = all_options(&f, &cache, &mc("1.21.4"), 100).await;
        let by = |k: LoaderKind| all.iter().find(|o| o.loader == k).unwrap();

        assert_eq!(by(LoaderKind::Fabric).status, Status::Ok);
        assert_eq!(
            by(LoaderKind::Fabric)
                .recommended
                .as_ref()
                .unwrap()
                .as_str(),
            "0.19.5",
            "stable beats a newer beta"
        );
        assert_eq!(
            by(LoaderKind::Forge).recommended.as_ref().unwrap().as_str(),
            "54.1.14",
            "recommended beats latest"
        );
        assert_eq!(
            by(LoaderKind::NeoForge)
                .recommended
                .as_ref()
                .unwrap()
                .as_str(),
            "21.4.158"
        );
        assert_eq!(
            by(LoaderKind::Quilt).recommended.as_ref().unwrap().as_str(),
            "0.24.0"
        );
        assert!(all
            .iter()
            .all(|o| o.origin == Some(Origin::Network) && o.error.is_none()));
        assert_eq!(
            all.iter().map(|o| o.loader).collect::<Vec<_>>(),
            [
                LoaderKind::Fabric,
                LoaderKind::Forge,
                LoaderKind::NeoForge,
                LoaderKind::Quilt
            ]
        );
    }

    #[tokio::test]
    async fn a_loader_without_a_compatible_version_is_reported_as_none_never_guessed() {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        let all = all_options(&f, &cache, &mc("1.20.1"), 100).await;
        let by = |k: LoaderKind| all.iter().find(|o| o.loader == k).unwrap();
        assert_eq!(by(LoaderKind::Fabric).status, Status::None); // official catalog answered with an empty list
        assert_eq!(by(LoaderKind::NeoForge).status, Status::None); // 1.20.1 predates NeoForge
        assert_eq!(by(LoaderKind::Forge).status, Status::Ok);
        assert_eq!(by(LoaderKind::Quilt).status, Status::Ok);
        assert!(
            by(LoaderKind::Fabric).recommended.is_none()
                && by(LoaderKind::Fabric).versions.is_empty()
        );
    }

    #[tokio::test]
    async fn offline_uses_the_saved_list_flagged_stale_and_unknown_when_nothing_was_ever_saved() {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        all_options(&f, &cache, &mc("1.21.4"), 100).await; // populates the cache
        f.fail_all(); // offline
        let all = all_options(&f, &cache, &mc("1.21.4"), 200).await;
        assert!(all.iter().all(|o| o.status == Status::Ok
            && o.origin == Some(Origin::CacheStale)
            && o.error.is_some()));
        assert_eq!(all[0].recommended.as_ref().unwrap().as_str(), "0.19.5");
        // A Minecraft version never checked while online: compatibility is unknown, so not offered.
        let unknown = options_for(&f, &cache, LoaderKind::Fabric, &mc("1.19.4"), 300).await;
        assert_eq!(unknown.status, Status::Unavailable);
        assert!(unknown.versions.is_empty() && unknown.error.is_some());
    }

    #[tokio::test]
    async fn offline_answers_for_forge_and_neoforge_are_computed_for_the_version_asked_not_the_one_cached_first(
    ) {
        // Regression: the single shared catalog used to be saved already filtered for whichever
        // Minecraft version was asked first, then reused for every other version while offline.
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        // Online, only 1.21.4 is ever looked at.
        let online = all_options(&f, &cache, &mc("1.21.4"), 1).await;
        assert!(online.iter().all(|o| o.status == Status::Ok));
        f.fail_all(); // now offline

        let forge_1201 = options_for(&f, &cache, LoaderKind::Forge, &mc("1.20.1"), 2).await;
        assert_eq!(forge_1201.status, Status::Ok);
        assert_eq!(
            forge_1201.recommended.as_ref().unwrap().as_str(),
            "47.4.10",
            "1.20.1's own Forge, not 1.21.4's 54.1.14"
        );
        assert_eq!(forge_1201.origin, Some(Origin::CacheStale));

        let neo_1201 = options_for(&f, &cache, LoaderKind::NeoForge, &mc("1.20.1"), 2).await;
        assert_eq!(
            neo_1201.status,
            Status::None,
            "NeoForge has nothing for 1.20.1, online or offline"
        );
        assert!(neo_1201.versions.is_empty());

        let neo_1204 = options_for(&f, &cache, LoaderKind::NeoForge, &mc("1.20.4"), 2).await;
        assert_eq!(neo_1204.recommended.as_ref().unwrap().as_str(), "20.4.237");
        let neo_1214 = options_for(&f, &cache, LoaderKind::NeoForge, &mc("1.21.4"), 2).await;
        assert_eq!(neo_1214.recommended.as_ref().unwrap().as_str(), "21.4.158");

        // A Minecraft version the saved catalog has no entry for: no compatible loader, never a neighbour's.
        let forge_unknown = options_for(&f, &cache, LoaderKind::Forge, &mc("1.2.3"), 2).await;
        assert_eq!(forge_unknown.status, Status::None);

        // Per-version loaders (Fabric/Quilt) were never checked for 1.20.1, so they are unknown offline.
        assert_eq!(
            options_for(&f, &cache, LoaderKind::Fabric, &mc("1.20.1"), 2)
                .await
                .status,
            Status::Unavailable
        );
    }

    #[tokio::test]
    async fn the_saved_global_catalog_is_the_raw_catalog_not_a_filtered_list() {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        options_for(&f, &cache, LoaderKind::Forge, &mc("1.21.4"), 5).await;
        let saved: CachedRaw =
            serde_json::from_slice(&fs::read(cache.join("loader-forge.json")).unwrap()).unwrap();
        assert_eq!(saved.fetched_at, 5);
        let promos = saved
            .body
            .get("promos")
            .and_then(|p| p.as_object())
            .expect("raw promos are kept");
        assert!(
            promos.contains_key("1.20.1-recommended") && promos.contains_key("1.21.4-latest"),
            "every Minecraft version is kept"
        );
    }

    #[tokio::test]
    async fn an_old_filtered_cache_file_from_an_earlier_build_is_ignored_not_trusted() {
        let (_d, cache) = dir();
        fs::create_dir_all(&cache).unwrap();
        // The previous (buggy) format: an already-filtered offer list under the shared name.
        fs::write(cache.join("loader-forge.json"), r#"{"schema":1,"fetched_at":1,"offers":[{"version":"66.0.9","channel":"recommended"}]}"#).unwrap();
        let f = FakeFetcher::new();
        f.fail_all();
        let r = options_for(&f, &cache, LoaderKind::Forge, &mc("1.20.1"), 2).await;
        assert_eq!(r.status, Status::Unavailable, "unknown, not a stale guess");
    }

    #[tokio::test]
    async fn garbage_responses_never_overwrite_a_good_saved_list() {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        options_for(&f, &cache, LoaderKind::Forge, &mc("1.21.4"), 1).await;
        let file = cache.join("loader-forge.json");
        let before = fs::read(&file).unwrap();
        f.ok(
            "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json",
            "<html>blocked</html>",
        );
        let r = options_for(&f, &cache, LoaderKind::Forge, &mc("1.21.4"), 2).await;
        assert_eq!(r.origin, Some(Origin::CacheStale));
        assert_eq!(r.recommended.as_ref().unwrap().as_str(), "54.1.14");
        assert_eq!(fs::read(&file).unwrap(), before);
    }

    #[tokio::test]
    async fn forge_and_neoforge_share_one_cache_file_while_fabric_and_quilt_are_per_version_and_pruned(
    ) {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        options_for(&f, &cache, LoaderKind::Forge, &mc("1.21.4"), 1).await;
        options_for(&f, &cache, LoaderKind::Forge, &mc("1.20.1"), 1).await;
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);

        for i in 0..40 {
            let v = format!("1.{i}.0");
            f.ok(
                &format!("https://meta.fabricmc.net/v2/versions/loader/{v}"),
                FABRIC,
            );
            options_for(&f, &cache, LoaderKind::Fabric, &mc(&v), 1).await;
        }
        let fabric_files = fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("loader-fabric--")
            })
            .count();
        assert!(fabric_files <= KEEP_PER_MC_FILES, "{fabric_files}");
    }

    #[tokio::test]
    async fn cache_files_hold_only_validated_data_and_no_absolute_paths() {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        routes(&f);
        options_for(&f, &cache, LoaderKind::Fabric, &mc("1.21.4"), 7).await;
        let text = fs::read_to_string(cache.join("loader-fabric--1.21.4.json")).unwrap();
        assert!(text.contains("0.19.5") && !text.contains(":\\") && !text.contains("C:/"));
        let saved: CachedOffers = serde_json::from_str(&text).unwrap();
        assert_eq!(
            (saved.schema, saved.fetched_at, saved.offers.len()),
            (1, 7, 3)
        );
    }

    #[tokio::test]
    async fn space_containing_minecraft_ids_work_for_per_version_loaders() {
        let (_d, cache) = dir();
        let f = FakeFetcher::new();
        f.ok(
            "https://meta.fabricmc.net/v2/versions/loader/1.14%20Pre-Release%201",
            FABRIC,
        );
        let r = options_for(&f, &cache, LoaderKind::Fabric, &mc("1.14 Pre-Release 1"), 1).await;
        assert_eq!(
            r.status,
            Status::Ok,
            "the URL is percent-encoded, not rejected"
        );
    }

    #[tokio::test]
    async fn oversized_cache_files_are_rejected_without_reading_past_the_limit() {
        let (_d, cache) = dir();
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join("loader-fabric--1.21.4.json"),
            vec![b' '; (MAX_CACHE_BYTES as usize) + 16],
        )
        .unwrap();
        fs::write(
            cache.join("loader-forge.json"),
            vec![b' '; (MAX_CACHE_BYTES as usize) + 16],
        )
        .unwrap();
        let f = FakeFetcher::new();
        f.fail_all();
        let fab = options_for(&f, &cache, LoaderKind::Fabric, &mc("1.21.4"), 1).await;
        let frg = options_for(&f, &cache, LoaderKind::Forge, &mc("1.21.4"), 1).await;
        assert_eq!(fab.status, Status::Unavailable);
        assert_eq!(frg.status, Status::Unavailable);
    }
}
