//! Mod *loader* support: official sources only, loaders only.
//!
//! Scope (product rules, see docs/TECHNICAL.md): the launcher prepares Minecraft
//! and, for Modded instances, exactly one loader taken from that loader's own official host.
//! It never searches, lists, recommends or downloads mods, modpacks, shaders or resource
//! packs. After setup the user adds their own content to the instance folders by hand.
//!
//! This module is pure: it builds official URLs, parses official catalogs, orders and picks
//! compatible versions, and describes how a loader would be installed. It performs no network
//! or process I/O; the (later) download/launch pipeline does that using these plans.
//!
//! Official sources
//! - Fabric: `meta.fabricmc.net` (loader list per Minecraft version; version-profile JSON with
//!   per-library hashes and sizes; libraries from `maven.fabricmc.net`).
//! - Quilt: `meta.quiltmc.org` (same shape; libraries from `maven.quiltmc.org`).
//! - Forge: `files.minecraftforge.net` promotions + installer from `maven.minecraftforge.net`.
//! - NeoForge: `maven.neoforged.net` version list + installer from the same Maven.
//!
//! Trust model
//! - Fabric/Quilt: a profile JSON is data, not code. Every library it names is downloaded over
//!   HTTPS from the host in the allow-list and verified against the hash/size in the profile.
//! - Forge/NeoForge: the official installer jar must be fetched, verified against the `.sha256`
//!   published next to it on the same official Maven, and only then run with the
//!   launcher-managed Java in a scratch directory under `cache/`. Residual trust: the checksum
//!   comes from the same host as the jar (it detects corruption and tampering of the file in
//!   transit or at rest, not a compromised loader project), and the installer's own library
//!   downloads are made by that official program, not by our allow-listed downloader.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::download_policy::{DownloadPolicy, LOADER_HOSTS};
use crate::versions::{LoaderVersion, McVersion, VersionId};

/// Catalogs are small; anything bigger is hostile or broken.
pub const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;
/// A long list is noise for a user choosing a loader; keep the newest few.
const MAX_OFFERS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderKind {
    Fabric,
    Quilt,
    Forge,
    #[serde(rename = "neoforge")]
    NeoForge,
}

impl LoaderKind {
    pub const ALL: [LoaderKind; 4] = [
        LoaderKind::Fabric,
        LoaderKind::Forge,
        LoaderKind::NeoForge,
        LoaderKind::Quilt,
    ];

    pub fn name(self) -> &'static str {
        match self {
            LoaderKind::Fabric => "Fabric",
            LoaderKind::Quilt => "Quilt",
            LoaderKind::Forge => "Forge",
            LoaderKind::NeoForge => "NeoForge",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error("loader catalog is too large")]
    TooLarge,
    #[error("loader catalog is not in the expected format: {0}")]
    BadFormat(&'static str),
    #[error("could not build an official URL")]
    Url,
    #[error("{0}")]
    Token(#[from] crate::versions::InvalidToken),
}

/// How trustworthy/mature a loader version is, best first (the derive order is the preference order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Marked recommended by the loader project (Forge).
    Recommended,
    /// A normal release (Fabric `stable`, NeoForge without a `-beta` suffix, Quilt without a pre-release tag).
    Stable,
    /// The newest build when it is not the recommended one (Forge `latest`); may be less tested.
    Latest,
    Prerelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferedVersion {
    pub version: LoaderVersion,
    pub channel: Channel,
}

/// The policy used for every loader request: loader hosts only.
pub fn official_policy() -> DownloadPolicy {
    DownloadPolicy::new(LOADER_HOSTS)
}

fn url_from_segments(base: &str, segments: &[&str]) -> Result<Url, LoaderError> {
    let mut url = Url::parse(base).map_err(|_| LoaderError::Url)?;
    {
        let mut s = url.path_segments_mut().map_err(|_| LoaderError::Url)?;
        s.pop_if_empty();
        for seg in segments {
            s.push(seg);
        }
    }
    official_policy()
        .check(&url)
        .map_err(|_| LoaderError::Url)?;
    Ok(url)
}

/// Where to read the official catalog of versions for this loader and Minecraft version.
pub fn catalog_url(kind: LoaderKind, mc: &McVersion) -> Result<Url, LoaderError> {
    match kind {
        LoaderKind::Fabric => url_from_segments(
            "https://meta.fabricmc.net/v2/versions/loader",
            &[mc.as_str()],
        ),
        LoaderKind::Quilt => url_from_segments(
            "https://meta.quiltmc.org/v3/versions/loader",
            &[mc.as_str()],
        ),
        LoaderKind::Forge => url_from_segments(
            "https://files.minecraftforge.net/net/minecraftforge/forge",
            &["promotions_slim.json"],
        ),
        LoaderKind::NeoForge => url_from_segments(
            "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged",
            &["neoforge"],
        ),
    }
}

/// How an installation of one loader version would be performed (executed by the later pipeline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPlan {
    /// Fabric/Quilt: fetch this version-profile JSON, then its libraries (verified by the hashes
    /// inside it). No code is executed.
    Profile { profile_url: Url },
    /// Forge/NeoForge: fetch the official installer, verify it against the published SHA-256,
    /// then run it with the managed Java in a scratch directory.
    Installer { installer_url: Url, sha256_url: Url },
}

pub fn install_plan(
    kind: LoaderKind,
    mc: &McVersion,
    loader: &LoaderVersion,
) -> Result<InstallPlan, LoaderError> {
    let (mc, lv) = (mc.as_str(), loader.as_str());
    Ok(match kind {
        LoaderKind::Fabric => InstallPlan::Profile {
            profile_url: url_from_segments(
                "https://meta.fabricmc.net/v2/versions/loader",
                &[mc, lv, "profile", "json"],
            )?,
        },
        LoaderKind::Quilt => InstallPlan::Profile {
            profile_url: url_from_segments(
                "https://meta.quiltmc.org/v3/versions/loader",
                &[mc, lv, "profile", "json"],
            )?,
        },
        LoaderKind::Forge => {
            let combo = format!("{mc}-{lv}");
            let jar = format!("forge-{combo}-installer.jar");
            let base = "https://maven.minecraftforge.net/net/minecraftforge/forge";
            InstallPlan::Installer {
                installer_url: url_from_segments(base, &[&combo, &jar])?,
                sha256_url: url_from_segments(base, &[&combo, &format!("{jar}.sha256")])?,
            }
        }
        LoaderKind::NeoForge => {
            let jar = format!("neoforge-{lv}-installer.jar");
            let base = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
            InstallPlan::Installer {
                installer_url: url_from_segments(base, &[lv, &jar])?,
                sha256_url: url_from_segments(base, &[lv, &format!("{jar}.sha256")])?,
            }
        }
    })
}

/// The id the loader's profile is expected to have once installed. Only a hint: the real id is
/// read from the installed profile and that is what gets recorded in the instance.
pub fn expected_version_id(
    kind: LoaderKind,
    mc: &McVersion,
    loader: &LoaderVersion,
) -> Result<VersionId, LoaderError> {
    let id = match kind {
        LoaderKind::Fabric => format!("fabric-loader-{loader}-{mc}"),
        LoaderKind::Quilt => format!("quilt-loader-{loader}-{mc}"),
        LoaderKind::Forge => format!("{mc}-forge-{loader}"),
        LoaderKind::NeoForge => format!("neoforge-{loader}"),
    };
    Ok(VersionId::new(&id)?)
}

/// How NeoForge version numbers relate to Minecraft versions (a convention, not data in the catalog):
/// - Minecraft `1.A.B` -> NeoForge `A.B.x` (1.21.4 -> `21.4.x`, 1.21 -> `21.0.x`), from 1.20.2 on;
///   1.20.1 and earlier used a legacy artifact that is intentionally unsupported.
/// - The new Minecraft numbering (26.1, 26.1.2, 26.3, ...) -> NeoForge `<mc, zero-padded to three
///   parts>.x` (26.3 -> `26.3.0.x`, 26.1.2 -> `26.1.2.x`). Checked against the official data: every
///   real 26.x release has exactly that series on NeoForge's Maven, and NeoForge's own POM for
///   `26.3.0.48-beta` depends on Minecraft `26.3`.
///
/// Returns the series prefix (e.g. `21.4.`), or `None` where the convention does not apply
/// (snapshots, betas, older versions, or a scheme not seen yet). `None` means "no compatible
/// NeoForge known", never a guess.
pub fn neoforge_series(mc: &McVersion) -> Option<String> {
    let nums: Vec<u32> = mc
        .as_str()
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    match nums.as_slice() {
        [1, minor, rest @ ..] if rest.len() <= 1 => {
            let patch = rest.first().copied().unwrap_or(0);
            if *minor < 20 || (*minor == 20 && patch < 2) {
                return None;
            }
            Some(format!("{minor}.{patch}."))
        }
        [major, minor, rest @ ..] if *major >= 26 && rest.len() <= 1 => Some(format!(
            "{major}.{minor}.{}.",
            rest.first().copied().unwrap_or(0)
        )),
        _ => None,
    }
}

/// Parses an official catalog body into versions compatible with `mc`, newest first.
/// An empty result means "no compatible version", not an error.
pub fn parse_catalog(
    kind: LoaderKind,
    mc: &McVersion,
    body: &[u8],
) -> Result<Vec<OfferedVersion>, LoaderError> {
    if body.len() > MAX_CATALOG_BYTES {
        return Err(LoaderError::TooLarge);
    }
    let json: Value =
        serde_json::from_slice(body).map_err(|_| LoaderError::BadFormat("not JSON"))?;
    let mut offers = match kind {
        LoaderKind::Fabric => parse_meta_list(&json, |loader| {
            if loader
                .get("stable")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                Channel::Stable
            } else {
                Channel::Prerelease
            }
        })?,
        LoaderKind::Quilt => parse_meta_list(&json, |_| Channel::Stable)?, // channel refined from the version text below
        LoaderKind::Forge => parse_forge(&json, mc)?,
        LoaderKind::NeoForge => parse_neoforge(&json, mc)?,
    };
    if kind == LoaderKind::Quilt || kind == LoaderKind::NeoForge {
        for o in &mut offers {
            if o.channel != Channel::Recommended && is_prerelease_text(o.version.as_str()) {
                o.channel = Channel::Prerelease;
            }
        }
    }
    offers.sort_by(|a, b| {
        cmp_versions(b.version.as_str(), a.version.as_str())
            .then_with(|| a.channel.cmp(&b.channel))
            .then_with(|| b.version.as_str().cmp(a.version.as_str()))
    });
    offers.dedup_by(|next, kept| {
        if next.version == kept.version {
            kept.channel = kept.channel.min(next.channel);
            true
        } else {
            false
        }
    });
    if offers.len() > MAX_OFFERS {
        let rec_idx = offers
            .iter()
            .position(|o| o.channel == Channel::Recommended);
        let stable_idx = offers.iter().position(|o| o.channel == Channel::Stable);
        let mut extra: Vec<usize> = [rec_idx, stable_idx]
            .into_iter()
            .flatten()
            .filter(|&i| i >= MAX_OFFERS)
            .collect();
        extra.sort_unstable();
        extra.dedup();
        if extra.is_empty() {
            offers.truncate(MAX_OFFERS);
        } else {
            let mut head: Vec<OfferedVersion> = offers.iter().take(MAX_OFFERS).cloned().collect();
            let mut to_drop = extra.len();
            for idx in (0..head.len()).rev() {
                if to_drop == 0 {
                    break;
                }
                if Some(idx) != rec_idx && Some(idx) != stable_idx {
                    head.remove(idx);
                    to_drop -= 1;
                }
            }
            for idx in extra {
                head.push(offers[idx].clone());
            }
            offers = head;
        }
    }
    Ok(offers)
}

/// Fabric and Quilt: `[ { "loader": { "version": "...", ... }, ... }, ... ]`, already filtered
/// by the server to the requested Minecraft version.
fn parse_meta_list(
    json: &Value,
    channel: impl Fn(&Value) -> Channel,
) -> Result<Vec<OfferedVersion>, LoaderError> {
    let arr = json
        .as_array()
        .ok_or(LoaderError::BadFormat("expected an array"))?;
    let mut out = Vec::new();
    for item in arr {
        let Some(loader) = item.get("loader") else {
            continue;
        };
        let Some(v) = loader.get("version").and_then(Value::as_str) else {
            continue;
        };
        if let Ok(version) = LoaderVersion::new(v) {
            out.push(OfferedVersion {
                version,
                channel: channel(loader),
            });
        }
    }
    Ok(out)
}

/// True when `mc` uses the modern v2+ Forge installer CLI (`--installClient <dir>`), starting at
/// Minecraft 1.12.2 (and 26.x+). Pre-1.12.2 v1 installers lack working headless target redirection.
fn forge_installer_supported(mc: &McVersion) -> bool {
    let core = mc.as_str().split('-').next().unwrap_or("");
    let Some(nums) = core
        .split('.')
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<Vec<u32>>>()
    else {
        return false;
    };
    match nums.as_slice() {
        [1, minor, rest @ ..] if rest.len() <= 1 => {
            let patch = rest.first().copied().unwrap_or(0);
            *minor > 12 || (*minor == 12 && patch >= 2)
        }
        [major, _, rest @ ..] if *major >= 26 && rest.len() <= 1 => true,
        _ => false,
    }
}

/// Forge: `{ "promos": { "1.21.4-recommended": "54.1.14", "1.21.4-latest": "54.1.18", ... } }`.
fn parse_forge(json: &Value, mc: &McVersion) -> Result<Vec<OfferedVersion>, LoaderError> {
    let promos = json
        .get("promos")
        .and_then(Value::as_object)
        .ok_or(LoaderError::BadFormat("missing promos"))?;
    if !forge_installer_supported(mc) {
        return Ok(Vec::new());
    }
    let get = |suffix: &str| {
        promos
            .get(&format!("{mc}-{suffix}"))
            .and_then(Value::as_str)
            .and_then(|v| LoaderVersion::new(v).ok())
    };
    let recommended = get("recommended");
    let latest = get("latest");
    let mut out = Vec::new();
    if let Some(r) = recommended.clone() {
        out.push(OfferedVersion {
            version: r,
            channel: Channel::Recommended,
        });
    }
    if let Some(l) = latest {
        if recommended.as_ref() != Some(&l) {
            out.push(OfferedVersion {
                version: l,
                channel: Channel::Latest,
            });
        }
    }
    Ok(out)
}

/// NeoForge: `{ "versions": ["21.4.0-beta", ..., "21.4.158"] }` covering every Minecraft version.
fn parse_neoforge(json: &Value, mc: &McVersion) -> Result<Vec<OfferedVersion>, LoaderError> {
    let versions = json
        .get("versions")
        .and_then(Value::as_array)
        .ok_or(LoaderError::BadFormat("missing versions"))?;
    let Some(prefix) = neoforge_series(mc) else {
        return Ok(Vec::new());
    };
    Ok(versions
        .iter()
        .filter_map(Value::as_str)
        .filter(|v| v.starts_with(&prefix))
        .filter_map(|v| LoaderVersion::new(v).ok())
        .map(|version| OfferedVersion {
            version,
            channel: Channel::Stable,
        })
        .collect())
}

fn is_prerelease_text(v: &str) -> bool {
    v.contains('-')
}

/// The default pick: the best channel (recommended, then stable, then latest, then prerelease),
/// newest within it. `offers` must be newest-first, as returned by `parse_catalog`.
pub fn choose_auto(offers: &[OfferedVersion]) -> Option<&OfferedVersion> {
    offers.iter().min_by_key(|o| o.channel) // min_by_key returns the first minimum: the newest of that channel
}

/// Orders loader version strings numerically (`54.1.9 < 54.1.14`), ignoring `+` build
/// metadata and sorting pre-releases (`-beta.2`, `-rc1`) before their release with numeric
/// pre-release tokens before alphanumeric ones.
pub fn cmp_versions(a: &str, b: &str) -> Ordering {
    fn split(v: &str) -> (Vec<u64>, Option<&str>) {
        // SemVer `+` build metadata does not affect precedence and may contain `-` or dots.
        let without_build = v.split_once('+').map_or(v, |(head, _)| head);
        let (core, pre) = match without_build.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (without_build, None),
        };
        (
            core.split('.')
                .map(|p| p.parse::<u64>().unwrap_or(0))
                .collect(),
            pre,
        )
    }
    fn pre_tokens(p: &str) -> Vec<&str> {
        p.split(['.', '-']).collect()
    }
    let ((ca, pa), (cb, pb)) = (split(a), split(b));
    let n = ca.len().max(cb.len());
    for i in 0..n {
        let (x, y) = (
            ca.get(i).copied().unwrap_or(0),
            cb.get(i).copied().unwrap_or(0),
        );
        match x.cmp(&y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let (tx, ty) = (pre_tokens(x), pre_tokens(y));
            for i in 0..tx.len().max(ty.len()) {
                match (tx.get(i), ty.get(i)) {
                    (Some(p), Some(q)) => {
                        let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                            (Ok(m), Ok(n)) => m.cmp(&n),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => p.cmp(q),
                        };
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    (Some(_), None) => return Ordering::Greater,
                    (None, Some(_)) => return Ordering::Less,
                    (None, None) => {}
                }
            }
            Ordering::Equal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mc(s: &str) -> McVersion {
        McVersion::new(s).unwrap()
    }
    fn lv(s: &str) -> LoaderVersion {
        LoaderVersion::new(s).unwrap()
    }
    fn versions(offers: &[OfferedVersion]) -> Vec<&str> {
        offers.iter().map(|o| o.version.as_str()).collect()
    }

    // Trimmed copies of the shapes the official endpoints return.
    const FABRIC: &str = r#"[
      {"loader":{"separator":".","build":5,"maven":"net.fabricmc:fabric-loader:0.19.5","version":"0.19.5","stable":true},"intermediary":{"version":"1.21.4"}},
      {"loader":{"separator":".","build":6,"maven":"net.fabricmc:fabric-loader:0.20.0-beta.1","version":"0.20.0-beta.1","stable":false},"intermediary":{"version":"1.21.4"}},
      {"loader":{"separator":".","build":4,"version":"0.18.0","stable":true}},
      {"nonsense":true},
      {"loader":{"version":"../../evil","stable":true}}
    ]"#;
    const QUILT: &str = r#"[
      {"loader":{"maven":"org.quiltmc:quilt-loader:0.20.0-beta.9","version":"0.20.0-beta.9","build":9}},
      {"loader":{"version":"0.20.0-beta.7","build":7}},
      {"loader":{"version":"0.24.0","build":24}},
      {"loader":{"version":"0.20.0-beta.8","build":8}}
    ]"#;
    const FORGE: &str = r#"{"homepage":"https://files.minecraftforge.net/net/minecraftforge/forge/","promos":{
      "1.21.4-latest":"54.1.18","1.21.4-recommended":"54.1.14","1.21.5-latest":"55.1.14","1.20.1-recommended":"47.4.10","1.20.1-latest":"47.4.26","1.12.2-latest":"14.23.5.2860","1.12.2-recommended":"14.23.5.2860","1.12.1-latest":"14.22.1.2485","1.8.9-recommended":"11.15.1.2318","1.7.10-latest":"10.13.4.1614","1.7.10-recommended":"10.13.4.1614"}}"#;
    const NEOFORGE: &str = r#"{"isSnapshot":false,"versions":["0.25w14craftmine.3-beta","20.2.3-beta","20.4.237","21.0.2-beta","21.0.167","21.1.172","21.4.0-beta","21.4.9-beta","21.4.10","21.4.158","21.40.1","26.1.2.7","26.1.2.10-beta","26.3.0.9-beta","26.3.0.48-beta"]}"#;

    #[test]
    fn urls_are_official_and_pass_the_loader_only_policy() {
        let m = mc("1.21.4");
        let hosts = [
            (LoaderKind::Fabric, "meta.fabricmc.net"),
            (LoaderKind::Quilt, "meta.quiltmc.org"),
            (LoaderKind::Forge, "files.minecraftforge.net"),
            (LoaderKind::NeoForge, "maven.neoforged.net"),
        ];
        for (kind, host) in hosts {
            let u = catalog_url(kind, &m).unwrap();
            assert_eq!(
                (u.scheme(), u.host_str().unwrap()),
                ("https", host),
                "{kind:?}"
            );
        }
        assert_eq!(
            catalog_url(LoaderKind::Fabric, &m).unwrap().as_str(),
            "https://meta.fabricmc.net/v2/versions/loader/1.21.4"
        );
        // Version strings are validated tokens, so they cannot inject path segments or queries.
        assert!(McVersion::new("1.21.4/../../x").is_err());
    }

    #[test]
    fn fabric_catalog_is_filtered_labelled_and_sorted() {
        let o = parse_catalog(LoaderKind::Fabric, &mc("1.21.4"), FABRIC.as_bytes()).unwrap();
        assert_eq!(
            versions(&o),
            ["0.20.0-beta.1", "0.19.5", "0.18.0"],
            "hostile/odd entries are skipped, newest first"
        );
        assert_eq!(o[0].channel, Channel::Prerelease);
        assert_eq!(
            choose_auto(&o).unwrap().version.as_str(),
            "0.19.5",
            "auto prefers the newest stable over a newer beta"
        );
    }

    #[test]
    fn quilt_unordered_catalog_is_sorted_and_betas_are_labelled() {
        let o = parse_catalog(LoaderKind::Quilt, &mc("1.21.4"), QUILT.as_bytes()).unwrap();
        assert_eq!(
            versions(&o),
            ["0.24.0", "0.20.0-beta.9", "0.20.0-beta.8", "0.20.0-beta.7"]
        );
        assert_eq!(o[0].channel, Channel::Stable);
        assert!(o[1..].iter().all(|x| x.channel == Channel::Prerelease));
        assert_eq!(choose_auto(&o).unwrap().version.as_str(), "0.24.0");
    }

    #[test]
    fn forge_offers_recommended_and_latest_per_minecraft_version() {
        let o = parse_catalog(LoaderKind::Forge, &mc("1.21.4"), FORGE.as_bytes()).unwrap();
        assert_eq!(versions(&o), ["54.1.18", "54.1.14"]);
        assert_eq!(
            choose_auto(&o).unwrap().version.as_str(),
            "54.1.14",
            "recommended wins over a newer latest"
        );
        // Only a "latest" exists: it is still chosen (nothing better available).
        let o = parse_catalog(LoaderKind::Forge, &mc("1.21.5"), FORGE.as_bytes()).unwrap();
        assert_eq!((o.len(), o[0].channel), (1, Channel::Latest));
        // Recommended == latest collapses to one entry; 1.12.2 is the oldest supported v2 installer.
        let o = parse_catalog(LoaderKind::Forge, &mc("1.12.2"), FORGE.as_bytes()).unwrap();
        assert_eq!((o.len(), o[0].channel), (1, Channel::Recommended));
        assert_eq!(o[0].version.as_str(), "14.23.5.2860");
        // Pre-1.12.2 legacy v1 installers lack working headless `--installClient <dir>` CLI.
        for legacy in ["1.7.10", "1.8.9", "1.12", "1.12.1", "b1.7.3"] {
            assert!(
                parse_catalog(LoaderKind::Forge, &mc(legacy), FORGE.as_bytes())
                    .unwrap()
                    .is_empty(),
                "{legacy}"
            );
        }
        // Unknown Minecraft version: no compatible loader, not an error.
        assert!(
            parse_catalog(LoaderKind::Forge, &mc("1.2.3"), FORGE.as_bytes())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn neoforge_maps_minecraft_versions_by_convention_and_never_guesses() {
        let o = parse_catalog(LoaderKind::NeoForge, &mc("1.21.4"), NEOFORGE.as_bytes()).unwrap();
        assert_eq!(
            versions(&o),
            ["21.4.158", "21.4.10", "21.4.9-beta", "21.4.0-beta"],
            "21.40.x must not match 21.4."
        );
        assert_eq!(choose_auto(&o).unwrap().version.as_str(), "21.4.158");
        let o = parse_catalog(LoaderKind::NeoForge, &mc("1.21"), NEOFORGE.as_bytes()).unwrap();
        assert_eq!(versions(&o), ["21.0.167", "21.0.2-beta"]);
        let o = parse_catalog(LoaderKind::NeoForge, &mc("1.20.4"), NEOFORGE.as_bytes()).unwrap();
        assert_eq!(versions(&o), ["20.4.237"]);
        for unsupported in [
            "1.20.1",
            "1.19.4",
            "1.12.2",
            "24w10a",
            "b1.7.3",
            "25.1",
            "26.4-snapshot-2",
            "1.21.4-pre1",
        ] {
            assert!(
                parse_catalog(LoaderKind::NeoForge, &mc(unsupported), NEOFORGE.as_bytes())
                    .unwrap()
                    .is_empty(),
                "{unsupported}"
            );
        }
        // Only betas available: still offered, labelled, and chosen as the best available.
        let o = parse_catalog(LoaderKind::NeoForge, &mc("1.20.2"), NEOFORGE.as_bytes()).unwrap();
        assert_eq!(
            (versions(&o), o[0].channel),
            (vec!["20.2.3-beta"], Channel::Prerelease)
        );
        assert!(choose_auto(&o).is_some());
    }

    #[test]
    fn neoforge_maps_the_new_minecraft_numbering_from_real_data() {
        let series = |v: &str| neoforge_series(&mc(v));
        assert_eq!(series("26.3").as_deref(), Some("26.3.0."));
        assert_eq!(series("26.1").as_deref(), Some("26.1.0."));
        assert_eq!(series("26.1.2").as_deref(), Some("26.1.2."));
        assert_eq!(series("1.21.4").as_deref(), Some("21.4."));
        assert_eq!(series("26.4-snapshot-2"), None);
        assert_eq!(series("25.1"), None);
        let o = parse_catalog(LoaderKind::NeoForge, &mc("26.3"), NEOFORGE.as_bytes()).unwrap();
        assert_eq!(versions(&o), ["26.3.0.48-beta", "26.3.0.9-beta"]);
        assert_eq!(o[0].channel, Channel::Prerelease);
        let o = parse_catalog(LoaderKind::NeoForge, &mc("26.1.2"), NEOFORGE.as_bytes()).unwrap();
        assert_eq!(versions(&o), ["26.1.2.10-beta", "26.1.2.7"]);
        assert_eq!(
            choose_auto(&o).unwrap().version.as_str(),
            "26.1.2.7",
            "stable wins over a newer beta"
        );
        // A different 26.x release has its own, separate series.
        assert!(
            parse_catalog(LoaderKind::NeoForge, &mc("26.2"), NEOFORGE.as_bytes())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn version_ordering_is_numeric_and_prereleases_sort_before_releases() {
        use Ordering::*;
        assert_eq!(cmp_versions("54.1.9", "54.1.14"), Less);
        assert_eq!(cmp_versions("0.20.0-beta.9", "0.20.0-beta.10"), Less);
        assert_eq!(cmp_versions("0.20.0-beta.9", "0.20.0"), Less);
        assert_eq!(cmp_versions("0.24.0", "0.20.0"), Greater);
        assert_eq!(cmp_versions("21.4.158", "21.4.158"), Equal);
        assert_eq!(cmp_versions("26.3.0.48-beta", "21.4.158"), Greater);
    }

    #[test]
    fn version_ordering_ignores_build_metadata_and_is_strict_weak_order() {
        use Ordering::*;

        // `+` build metadata must not corrupt core or pre-release numeric parsing.
        assert_eq!(cmp_versions("21.0.5+11", "21.0.1"), Greater);
        assert_eq!(cmp_versions("0.16.9+build.1", "0.16.8"), Greater);
        assert_eq!(cmp_versions("0.16.9+build.1", "0.16.9"), Equal);
        assert_eq!(cmp_versions("0.16.9+build-1", "0.16.9"), Equal);
        assert_eq!(cmp_versions("0.16.9+build-1", "0.16.9-rc.1"), Greater);
        assert_eq!(
            cmp_versions("0.20.0-beta.2+build.1", "0.20.0-beta.10+build.2"),
            Less
        );

        // Numeric pre-release tokens sort strictly before alphanumeric ones.
        assert_eq!(cmp_versions("0.1.0-2", "0.1.0-10"), Less);
        assert_eq!(cmp_versions("0.1.0-10", "0.1.0-1a"), Less);
        assert_eq!(cmp_versions("0.1.0-2", "0.1.0-1a"), Less);
        assert_eq!(cmp_versions("0.1.0-01", "0.1.0-1"), Equal);
        assert_eq!(cmp_versions("0.1.0-01", "0.1.0-0a"), Less);
        assert_eq!(cmp_versions("0.1.0-1", "0.1.0-0a"), Less);

        // Exhaustive strict-weak-order check across mixed numeric/alphanumeric/metadata tokens.
        let samples = [
            "0.1.0-01",
            "0.1.0-1",
            "0.1.0-2",
            "0.1.0-10",
            "0.1.0-0a",
            "0.1.0-1a",
            "0.1.0-alpha.1",
            "0.1.0-alpha.beta",
            "0.1.0-beta",
            "0.1.0-rc.1+build.4",
            "0.1.0",
            "0.1.0+build-1",
            "0.16.9+build.1",
            "21.0.1",
            "21.0.5+11",
        ];
        for &a in &samples {
            assert_eq!(cmp_versions(a, a), Equal, "reflexivity for {a}");
            for &b in &samples {
                assert_eq!(
                    cmp_versions(a, b),
                    cmp_versions(b, a).reverse(),
                    "antisymmetry for ({a}, {b})"
                );
                for &c in &samples {
                    let ab = cmp_versions(a, b);
                    let bc = cmp_versions(b, c);
                    let ac = cmp_versions(a, c);
                    if ab == Less && bc == Less {
                        assert_eq!(ac, Less, "transitivity (<) for ({a}, {b}, {c})");
                    }
                    if ab == Equal {
                        assert_eq!(ac, bc, "equivalence congruence for ({a}, {b}, {c})");
                    }
                }
            }
        }

        let mut reversed: Vec<&str> = samples.iter().copied().rev().collect();
        reversed.sort_by(|a, b| cmp_versions(a, b).then_with(|| a.cmp(b)));
        assert_eq!(reversed, samples);
    }

    #[test]
    fn bad_catalogs_are_rejected_safely() {
        for (kind, body) in [
            (LoaderKind::Fabric, "{}"),
            (LoaderKind::Forge, "[]"),
            (LoaderKind::NeoForge, "{\"x\":1}"),
            (LoaderKind::Quilt, "not json"),
        ] {
            assert!(
                parse_catalog(kind, &mc("1.21.4"), body.as_bytes()).is_err(),
                "{kind:?}"
            );
        }
        let huge = vec![b' '; MAX_CATALOG_BYTES + 1];
        assert!(matches!(
            parse_catalog(LoaderKind::Fabric, &mc("1.21.4"), &huge),
            Err(LoaderError::TooLarge)
        ));
        assert!(choose_auto(&[]).is_none());
    }

    #[test]
    fn install_plans_use_official_hosts_and_verify_installers() {
        let (m, f, q) = (mc("1.21.4"), lv("0.16.9"), lv("0.26.0"));
        match install_plan(LoaderKind::Fabric, &m, &f).unwrap() {
            InstallPlan::Profile { profile_url } => assert_eq!(
                profile_url.as_str(),
                "https://meta.fabricmc.net/v2/versions/loader/1.21.4/0.16.9/profile/json"
            ),
            other => panic!("{other:?}"),
        }
        match install_plan(LoaderKind::Quilt, &m, &q).unwrap() {
            InstallPlan::Profile { profile_url } => assert_eq!(
                profile_url.as_str(),
                "https://meta.quiltmc.org/v3/versions/loader/1.21.4/0.26.0/profile/json"
            ),
            other => panic!("{other:?}"),
        }
        match install_plan(LoaderKind::Forge, &m, &lv("54.1.14")).unwrap() {
            InstallPlan::Installer {
                installer_url,
                sha256_url,
            } => {
                assert_eq!(installer_url.as_str(), "https://maven.minecraftforge.net/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-installer.jar");
                assert_eq!(sha256_url.as_str(), format!("{installer_url}.sha256"));
            }
            other => panic!("{other:?}"),
        }
        match install_plan(LoaderKind::NeoForge, &m, &lv("21.4.158")).unwrap() {
            InstallPlan::Installer {
                installer_url,
                sha256_url,
            } => {
                assert_eq!(installer_url.as_str(), "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.4.158/neoforge-21.4.158-installer.jar");
                assert_eq!(sha256_url.as_str(), format!("{installer_url}.sha256"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn loader_policy_rejects_everything_that_is_not_a_loader_host() {
        let p = official_policy();
        for u in [
            "https://piston-data.mojang.com/x",
            "https://example.org/x",
            "http://meta.fabricmc.net/x",
            "https://meta.fabricmc.net.evil.example/x",
        ] {
            assert!(p.check(&Url::parse(u).unwrap()).is_err(), "{u}");
        }
        assert!(p
            .check(&Url::parse("https://maven.fabricmc.net/net/fabricmc/x.jar").unwrap())
            .is_ok());
    }

    #[test]
    fn expected_ids_match_what_each_loader_installs() {
        let m = mc("1.21.4");
        assert_eq!(
            expected_version_id(LoaderKind::Fabric, &m, &lv("0.16.9"))
                .unwrap()
                .as_str(),
            "fabric-loader-0.16.9-1.21.4"
        );
        assert_eq!(
            expected_version_id(LoaderKind::Quilt, &m, &lv("0.26.0"))
                .unwrap()
                .as_str(),
            "quilt-loader-0.26.0-1.21.4"
        );
        assert_eq!(
            expected_version_id(LoaderKind::Forge, &m, &lv("54.1.14"))
                .unwrap()
                .as_str(),
            "1.21.4-forge-54.1.14"
        );
        assert_eq!(
            expected_version_id(LoaderKind::NeoForge, &m, &lv("21.4.158"))
                .unwrap()
                .as_str(),
            "neoforge-21.4.158"
        );
    }

    #[test]
    fn truncation_never_drops_the_newest_stable_release_behind_many_betas() {
        let mut versions_list = vec!["21.4.10".to_owned()];
        for i in 1..=50 {
            versions_list.push(format!("21.4.{}-beta", 100 + i));
        }
        let body = serde_json::json!({ "versions": versions_list }).to_string();
        let o = parse_catalog(LoaderKind::NeoForge, &mc("1.21.4"), body.as_bytes()).unwrap();
        assert_eq!(o.len(), MAX_OFFERS);
        assert_eq!(o[0].version.as_str(), "21.4.150-beta");
        assert_eq!(o.last().unwrap().version.as_str(), "21.4.10");
        assert_eq!(o.last().unwrap().channel, Channel::Stable);
        assert_eq!(
            choose_auto(&o).unwrap().version.as_str(),
            "21.4.10",
            "50 newer betas must not push out the newest Stable release"
        );
        for pair in o.windows(2) {
            assert_eq!(
                cmp_versions(pair[0].version.as_str(), pair[1].version.as_str()),
                Ordering::Greater,
                "newest-first order is preserved"
            );
        }

        // Duplicate versions in Fabric catalog keep the best (Stable) channel regardless of input order.
        let dup = r#"[
          {"loader":{"version":"0.19.5","stable":false}},
          {"loader":{"version":"0.19.5","stable":true}}
        ]"#;
        let d = parse_catalog(LoaderKind::Fabric, &mc("1.21.4"), dup.as_bytes()).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].channel, Channel::Stable);
    }
}
