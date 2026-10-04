//! Profile (instance) management exposed to the UI, plus version and loader discovery.
//!
//! The frontend never fetches Minecraft or loader metadata and never touches the filesystem: it
//! asks these commands. Network policy, caching, validation and file ownership all stay in Rust.
//! Creating a profile uses the existing instance architecture (`instances`): the new instance is
//! `Pending` until the (later) install step, fully isolated, and Vanilla stays completely clean.
//!
//! Creation is verified here, not trusted from the UI:
//! - the Minecraft version must exist in Mojang's official catalog (live or saved copy);
//! - for a Modded profile the loader version must really be compatible with that Minecraft
//!   version according to the loader's official catalog (live or saved copy);
//! - if that cannot be checked at all (offline and never cached) creation is refused rather than
//!   guessed;
//! - names are validated and must be unique (case-insensitive); the instance id is allocated
//!   race-free by the store, so identifiers cannot conflict.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::State;

use crate::coded_error::{param, params, CodedError};
use crate::instances::{
    Instance, InstanceError, InstanceId, InstanceStore, LoaderVersionSpec, NewInstance, Profile,
    SetupState,
};
use crate::loader_options::{self, LoaderOptions, Status};
use crate::loaders::{Channel, LoaderKind};
use crate::mc_catalog::{self, CatalogError, Origin, VersionCatalog, VersionKind};
use crate::net::{Fetcher, HttpFetcher};
use crate::playtime::PlaytimeStore;
use crate::portable::PortableRoot;
use crate::ui_settings::SettingsState;
use crate::versions::McVersion;

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Everything the profile commands need, managed by Tauri.
pub struct ProfilesState {
    pub root: PortableRoot,
    /// Mojang and the global allow-list.
    pub global: Result<HttpFetcher, String>,
    /// Loader hosts only.
    pub loaders: Result<HttpFetcher, String>,
}

fn catalogs_dir(root: &PortableRoot) -> PathBuf {
    root.cache_dir().join("catalogs")
}

fn manifest_cache(root: &PortableRoot) -> PathBuf {
    catalogs_dir(root).join("version_manifest.json")
}

// ------------------------------------------------------------------------------ DTOs

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub id: String,
    pub name: String,
    pub minecraft: String,
    /// `vanilla` or `modded`.
    pub kind: &'static str,
    pub loader: Option<LoaderKind>,
    /// The pinned or installed loader version, if known yet.
    pub loader_version: Option<String>,
    pub ready: bool,
    pub playtime_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesDto {
    pub profiles: Vec<ProfileDto>,
    pub selected: Option<String>,
    pub total_playtime_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    pub id: String,
    pub kind: VersionKind,
    pub release_time: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDto {
    pub entries: Vec<EntryDto>,
    pub latest_release: Option<String>,
    pub latest_snapshot: Option<String>,
    pub fetched_at: i64,
    pub origin: Origin,
    /// True when this is a saved copy used because Mojang could not be reached (or the copy is old).
    pub stale: bool,
    pub network_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferDto {
    pub version: String,
    pub channel: Channel,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderOptionsDto {
    pub loader: LoaderKind,
    pub status: Status,
    pub versions: Vec<OfferDto>,
    pub recommended: Option<String>,
    pub stale: bool,
    pub error: Option<String>,
}

impl LoaderOptionsDto {
    fn from_options(o: LoaderOptions) -> Self {
        Self {
            loader: o.loader,
            status: o.status,
            versions: o
                .versions
                .into_iter()
                .map(|v| OfferDto {
                    version: v.version.to_string(),
                    channel: v.channel,
                })
                .collect(),
            recommended: o.recommended.map(|v| v.to_string()),
            stale: o.origin == Some(Origin::CacheStale),
            error: o.error,
        }
    }
}

fn catalog_dto(c: VersionCatalog, origin: Origin, network_error: Option<String>) -> CatalogDto {
    CatalogDto {
        latest_release: c.latest_release.map(|v| v.to_string()),
        latest_snapshot: c.latest_snapshot.map(|v| v.to_string()),
        fetched_at: c.fetched_at,
        entries: c
            .entries
            .into_iter()
            .map(|e| EntryDto {
                id: e.id.to_string(),
                kind: e.kind,
                release_time: e.release_time,
            })
            .collect(),
        stale: origin == Origin::CacheStale,
        origin,
        network_error,
    }
}

// ------------------------------------------------------------------------------ errors

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("A profile named \u{201c}{0}\u{201d} already exists.")]
    DuplicateName(String),
    #[error("Minecraft {0} is not in Mojang's official version list.")]
    UnknownMinecraft(String),
    /// The official lists could not be reached (and none is saved). `Some((loader, minecraft))`
    /// when it was a loader's list that was needed.
    #[error("Can't check {} right now.", match .0 { Some((l, m)) => format!("which {l} versions fit Minecraft {m}"), None => "Minecraft versions".to_owned() })]
    CannotVerify(Option<(&'static str, String)>),
    #[error("{loader} has no version for Minecraft {minecraft}.")]
    NoCompatibleLoader {
        loader: &'static str,
        minecraft: String,
    },
    #[error("{loader} {version} is not a compatible version for Minecraft {minecraft}.")]
    LoaderVersionNotOffered {
        loader: &'static str,
        version: String,
        minecraft: String,
    },
    #[error("{0}")]
    Instance(#[from] InstanceError),
    #[error("Could not save the profile: {0}")]
    Io(#[from] std::io::Error),
}

impl ProfileError {
    /// The stable code and facts the UI words in the selected language.
    pub fn coded(&self) -> CodedError {
        let message = self.to_string();
        match self {
            Self::DuplicateName(n) => {
                CodedError::new("duplicate_name", message).with(param("name", n.clone()))
            }
            Self::UnknownMinecraft(v) => {
                CodedError::new("unknown_minecraft", message).with(param("minecraft", v.clone()))
            }
            Self::CannotVerify(None) => CodedError::new("cannot_verify", message),
            Self::CannotVerify(Some((loader, mc))) => {
                CodedError::new("cannot_verify_loader", message).with(params([
                    ("loader", (*loader).into()),
                    ("minecraft", mc.clone().into()),
                ]))
            }
            Self::NoCompatibleLoader { loader, minecraft } => {
                CodedError::new("no_compatible_loader", message).with(params([
                    ("loader", (*loader).into()),
                    ("minecraft", minecraft.clone().into()),
                ]))
            }
            Self::LoaderVersionNotOffered {
                loader,
                version,
                minecraft,
            } => CodedError::new("loader_version_not_offered", message).with(params([
                ("loader", (*loader).into()),
                ("version", version.clone().into()),
                ("minecraft", minecraft.clone().into()),
            ])),
            Self::Instance(InstanceError::BadName(_)) => CodedError::new("invalid_name", message),
            Self::Instance(InstanceError::NotFound(_) | InstanceError::BadId(_)) => {
                CodedError::new("not_found", message)
            }
            Self::Instance(_) | Self::Io(_) => CodedError::new("storage", message),
        }
    }
}

// ------------------------------------------------------------------------------ core logic

fn label(i: &Instance) -> (Option<String>, bool) {
    let loader_version = match (&i.profile, &i.setup) {
        (_, SetupState::Ready { loader_version, .. }) => {
            loader_version.as_ref().map(|v| v.to_string())
        }
        (
            Profile::Modded {
                loader_version: LoaderVersionSpec::Pinned { version },
                ..
            },
            _,
        ) => Some(version.to_string()),
        _ => None,
    };
    (loader_version, i.is_ready())
}

/// Lists the profiles with their playtime and the resolved selection.
pub fn snapshot(root: &PortableRoot, selected: Option<InstanceId>) -> std::io::Result<ProfilesDto> {
    let listing = InstanceStore::new(root).list()?;
    let playtime = PlaytimeStore::new(root);
    let profiles: Vec<ProfileDto> = listing
        .instances
        .iter()
        .map(|i| {
            let (loader_version, ready) = label(i);
            ProfileDto {
                id: i.id.to_string(),
                name: i.name.clone(),
                minecraft: i.minecraft.to_string(),
                kind: if i.is_vanilla() { "vanilla" } else { "modded" },
                loader: i.loader(),
                loader_version,
                ready,
                playtime_seconds: playtime
                    .instance_total(&i.id)
                    .map(|t| t.total_seconds)
                    .unwrap_or(0),
            }
        })
        .collect();
    // A selection that no longer exists (deleted, or a folder from another launcher) falls back to the first profile.
    let selected = selected
        .map(|s| s.to_string())
        .filter(|s| profiles.iter().any(|p| &p.id == s))
        .or_else(|| profiles.first().map(|p| p.id.clone()));
    Ok(ProfilesDto {
        profiles,
        selected,
        total_playtime_seconds: playtime
            .lifetime_total()
            .map(|t| t.total_seconds)
            .unwrap_or(0),
    })
}

pub async fn version_catalog<F: Fetcher>(
    fetcher: &F,
    root: &PortableRoot,
    force: bool,
    now: i64,
) -> Result<CatalogDto, CatalogError> {
    let r = mc_catalog::load(fetcher, &manifest_cache(root), now, force).await?;
    Ok(catalog_dto(r.catalog, r.origin, r.network_error))
}

pub async fn loader_options_all<F: Fetcher>(
    fetcher: &F,
    root: &PortableRoot,
    minecraft: &McVersion,
    now: i64,
) -> Vec<LoaderOptionsDto> {
    loader_options::all_options(fetcher, &catalogs_dir(root), minecraft, now)
        .await
        .into_iter()
        .map(LoaderOptionsDto::from_options)
        .collect()
}

/// Creates a profile after verifying it against the official catalogs. Returns the new snapshot
/// with the new profile selected.
pub async fn create_verified<G: Fetcher, L: Fetcher>(
    global: &G,
    loaders: &L,
    root: &PortableRoot,
    settings: &SettingsState,
    request: NewInstance,
    now: i64,
) -> Result<ProfilesDto, ProfileError> {
    let store = InstanceStore::new(root);

    // 1. A friendly, case-insensitive duplicate check (the store would also keep ids unique).
    let wanted = request.name.trim().to_lowercase();
    if store
        .list()?
        .instances
        .iter()
        .any(|i| i.name.trim().to_lowercase() == wanted)
    {
        return Err(ProfileError::DuplicateName(request.name.trim().to_owned()));
    }

    // 2. The Minecraft version must be in Mojang's official catalog (live or saved).
    let catalog = mc_catalog::load(global, &manifest_cache(root), now, false)
        .await
        .map_err(|e| {
            tracing::info!(error = %e, "the Minecraft version list could not be checked");
            ProfileError::CannotVerify(None)
        })?;
    if !catalog.catalog.contains(&request.minecraft) {
        return Err(ProfileError::UnknownMinecraft(
            request.minecraft.to_string(),
        ));
    }

    // 3. Modded: the loader version must be compatible according to the loader's official catalog.
    if let Profile::Modded {
        loader,
        loader_version,
    } = &request.profile
    {
        let opts = loader_options::options_for(
            loaders,
            &catalogs_dir(root),
            *loader,
            &request.minecraft,
            now,
        )
        .await;
        let name = loader.name();
        match opts.status {
            Status::Unavailable => {
                return Err(ProfileError::CannotVerify(Some((
                    name,
                    request.minecraft.to_string(),
                ))))
            }
            Status::None => {
                return Err(ProfileError::NoCompatibleLoader {
                    loader: name,
                    minecraft: request.minecraft.to_string(),
                })
            }
            Status::Ok => {}
        }
        if let LoaderVersionSpec::Pinned { version } = loader_version {
            if !opts.versions.iter().any(|o| &o.version == version) {
                return Err(ProfileError::LoaderVersionNotOffered {
                    loader: name,
                    version: version.to_string(),
                    minecraft: request.minecraft.to_string(),
                });
            }
        }
    }

    // 4. Create it (isolated, Pending) and select it.
    let created = store.create(request, now)?;
    if let Err(e) = settings.set_last_instance(Some(created.id.clone())) {
        tracing::warn!(error = %e, "could not remember the new profile as selected");
    }
    Ok(snapshot(root, Some(created.id))?)
}

// ------------------------------------------------------------------------------ Tauri commands

fn fetcher(f: &Result<HttpFetcher, String>) -> Result<&HttpFetcher, CodedError> {
    f.as_ref().map_err(|e| {
        CodedError::new(
            "network_unavailable",
            format!("networking is unavailable: {e}"),
        )
    })
}

#[tauri::command]
pub fn list_profiles(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
) -> Result<ProfilesDto, CodedError> {
    snapshot(&state.root, settings.last_instance()).map_err(CodedError::storage)
}

#[tauri::command]
pub fn select_profile(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
) -> Result<(), CodedError> {
    let id = InstanceId::new(&id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    InstanceStore::new(&state.root)
        .get(&id)
        .map_err(|e| CodedError::new("not_found", e.to_string()))?;
    settings
        .set_last_instance(Some(id))
        .map_err(CodedError::storage)
}

#[tauri::command]
pub async fn get_version_catalog(
    state: State<'_, ProfilesState>,
    force: Option<bool>,
) -> Result<CatalogDto, CodedError> {
    let f = fetcher(&state.global)?;
    version_catalog(f, &state.root, force.unwrap_or(false), now_unix())
        .await
        .map_err(|e| CodedError::new("catalog_unavailable", e.to_string()))
}

#[tauri::command]
pub async fn get_loader_options(
    state: State<'_, ProfilesState>,
    minecraft: String,
) -> Result<Vec<LoaderOptionsDto>, CodedError> {
    let mc = McVersion::new(&minecraft).map_err(|e| {
        CodedError::new("unknown_minecraft", e.to_string())
            .with(param("minecraft", minecraft.clone()))
    })?;
    let f = fetcher(&state.loaders)?;
    Ok(loader_options_all(f, &state.root, &mc, now_unix()).await)
}

#[tauri::command]
pub async fn create_profile(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    request: NewInstance,
) -> Result<ProfilesDto, CodedError> {
    let (g, l) = (fetcher(&state.global)?, fetcher(&state.loaders)?);
    create_verified(g, l, &state.root, &settings, request, now_unix())
        .await
        .map_err(|e| e.coded())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::{JavaChoice, LoaderVersionSpec};

    #[test]
    fn profile_errors_become_stable_codes_with_named_parameters() {
        let dup = ProfileError::DuplicateName("Survival".into()).coded();
        assert_eq!(dup.code, "duplicate_name");
        assert_eq!(dup.params.as_ref().unwrap()["name"], "Survival");
        let none = ProfileError::NoCompatibleLoader {
            loader: "Fabric",
            minecraft: "1.20.1".into(),
        }
        .coded();
        assert_eq!(none.code, "no_compatible_loader");
        assert_eq!(none.params.as_ref().unwrap()["loader"], "Fabric");
        assert_eq!(none.params.as_ref().unwrap()["minecraft"], "1.20.1");
        let offline = ProfileError::CannotVerify(Some(("Forge", "1.21.4".into()))).coded();
        assert_eq!(offline.code, "cannot_verify_loader");
        assert_eq!(
            ProfileError::CannotVerify(None).coded().code,
            "cannot_verify"
        );
        let io = ProfileError::Io(std::io::Error::other("disk on fire")).coded();
        assert_eq!(io.code, "storage");
        // The English detail stays available for logs, but the code is what the UI uses.
        assert!(io.message.contains("disk on fire"));
    }

    use crate::net::testing::FakeFetcher;
    use crate::settings::LauncherSettings;
    use crate::versions::LoaderVersion;
    use std::fs;

    const MANIFEST: &str = r#"{"latest":{"release":"1.21.4","snapshot":"25w01a"},"versions":[
      {"id":"25w01a","type":"snapshot","releaseTime":"2025-01-01T00:00:00+00:00"},
      {"id":"1.21.4","type":"release","releaseTime":"2024-12-03T10:12:57+00:00"},
      {"id":"1.20.1","type":"release","releaseTime":"2023-06-12T10:00:00+00:00"},
      {"id":"1.14 Pre-Release 1","type":"snapshot","releaseTime":"2019-04-01T00:00:00+00:00"},
      {"id":"b1.7.3","type":"old_beta","releaseTime":"2011-07-08T00:00:00+00:00"}]}"#;

    fn mc_url() -> &'static str {
        mc_catalog::MANIFEST_URL
    }

    struct Env {
        _d: tempfile::TempDir,
        root: PortableRoot,
        settings: SettingsState,
        global: FakeFetcher,
        loaders: FakeFetcher,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("Launcher"));
        root.ensure_layout().unwrap();
        let settings = SettingsState::new(root.settings_file(), LauncherSettings::default());
        let global = FakeFetcher::new();
        global.ok(mc_url(), MANIFEST);
        let loaders = FakeFetcher::new();
        loaders.ok("https://meta.fabricmc.net/v2/versions/loader/1.21.4", r#"[{"loader":{"version":"0.19.5","stable":true}},{"loader":{"version":"0.18.0","stable":true}}]"#);
        loaders.ok("https://meta.fabricmc.net/v2/versions/loader/1.20.1", "[]");
        loaders.ok(
            "https://meta.quiltmc.org/v3/versions/loader/1.21.4",
            r#"[{"loader":{"version":"0.24.0"}}]"#,
        );
        loaders.ok("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json", r#"{"promos":{"1.21.4-recommended":"54.1.14","1.21.4-latest":"54.1.18","1.20.1-recommended":"47.4.10"}}"#);
        loaders.ok(
            "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge",
            r#"{"versions":["21.4.158","21.4.9-beta"]}"#,
        );
        Env {
            _d: d,
            root,
            settings,
            global,
            loaders,
        }
    }

    fn vanilla(name: &str, mc: &str) -> NewInstance {
        NewInstance::vanilla(name, mc).unwrap()
    }
    fn modded(name: &str, mc: &str, kind: LoaderKind, spec: LoaderVersionSpec) -> NewInstance {
        NewInstance {
            name: name.into(),
            minecraft: McVersion::new(mc).unwrap(),
            profile: Profile::Modded {
                loader: kind,
                loader_version: spec,
            },
            java: JavaChoice::Auto,
            memory_mb: None,
            extra_jvm_args: vec![],
        }
    }
    async fn create(e: &Env, req: NewInstance) -> Result<ProfilesDto, ProfileError> {
        create_verified(&e.global, &e.loaders, &e.root, &e.settings, req, 1000).await
    }

    #[tokio::test]
    async fn vanilla_creation_is_pending_clean_selected_and_never_touches_loaders() {
        let e = env();
        let snap = create(&e, vanilla("Survival", "1.21.4")).await.unwrap();
        assert_eq!(snap.profiles.len(), 1);
        let p = &snap.profiles[0];
        assert_eq!(
            (
                p.name.as_str(),
                p.minecraft.as_str(),
                p.kind,
                p.loader,
                p.ready
            ),
            ("Survival", "1.21.4", "vanilla", None, false)
        );
        assert_eq!(snap.selected.as_deref(), Some(p.id.as_str()));
        assert!(
            e.loaders.calls().is_empty(),
            "Vanilla never asks about loaders"
        );
        let game = e.root.instances_dir().join(&p.id).join("game");
        assert!(
            game.is_dir() && fs::read_dir(&game).unwrap().count() == 0,
            "vanilla stays clean"
        );
        assert_eq!(e.settings.last_instance().unwrap().as_str(), p.id);
        let saved = LauncherSettings::load_or_default(&e.root.settings_file()).unwrap();
        assert_eq!(
            saved.last_instance.unwrap().as_str(),
            p.id,
            "the selection is saved in the portable settings file"
        );
    }

    #[tokio::test]
    async fn all_four_loaders_can_be_created_with_auto_or_a_pinned_compatible_version() {
        let e = env();
        for (i, kind) in LoaderKind::ALL.into_iter().enumerate() {
            let auto = create(
                &e,
                modded(
                    &format!("Auto {i}"),
                    "1.21.4",
                    kind,
                    LoaderVersionSpec::Auto,
                ),
            )
            .await
            .unwrap();
            assert_eq!(auto.profiles.last().unwrap().loader, Some(kind));
            assert_eq!(auto.profiles.last().unwrap().kind, "modded");
        }
        let pinned = create(
            &e,
            modded(
                "Pinned",
                "1.21.4",
                LoaderKind::Fabric,
                LoaderVersionSpec::Pinned {
                    version: LoaderVersion::new("0.18.0").unwrap(),
                },
            ),
        )
        .await
        .unwrap();
        let last = pinned.profiles.last().unwrap();
        assert_eq!(last.loader_version.as_deref(), Some("0.18.0"));
        // Every modded instance got its own five local folders and nothing shared.
        for p in &pinned.profiles {
            let game = e.root.instances_dir().join(&p.id).join("game");
            for d in ["mods", "config", "resourcepacks", "shaderpacks", "saves"] {
                assert!(game.join(d).is_dir(), "{} / {d}", p.name);
            }
        }
        assert!(
            !e.root.path().join("mods").exists() && !e.root.minecraft_dir().join("mods").exists()
        );
    }

    #[tokio::test]
    async fn creation_is_verified_against_the_official_catalogs_not_trusted() {
        let e = env();
        // Unknown Minecraft version.
        assert!(matches!(
            create(&e, vanilla("X", "9.9.9")).await,
            Err(ProfileError::UnknownMinecraft(_))
        ));
        // No compatible loader for this Minecraft version (Fabric answered with an empty list).
        let r = create(
            &e,
            modded("F", "1.20.1", LoaderKind::Fabric, LoaderVersionSpec::Auto),
        )
        .await;
        assert!(
            matches!(r, Err(ProfileError::NoCompatibleLoader { .. })),
            "{r:?}"
        );
        // NeoForge has nothing for 1.20.1 (it starts at 1.20.2).
        assert!(matches!(
            create(
                &e,
                modded("N", "1.20.1", LoaderKind::NeoForge, LoaderVersionSpec::Auto)
            )
            .await,
            Err(ProfileError::NoCompatibleLoader { .. })
        ));
        // A pinned version the catalog does not offer is refused.
        let bad = modded(
            "P",
            "1.21.4",
            LoaderKind::Fabric,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("9.9.9").unwrap(),
            },
        );
        assert!(matches!(
            create(&e, bad).await,
            Err(ProfileError::LoaderVersionNotOffered { .. })
        ));
        // Nothing was created by any refusal.
        assert!(snapshot(&e.root, None).unwrap().profiles.is_empty());
        assert_eq!(fs::read_dir(e.root.instances_dir()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn names_are_validated_and_duplicates_are_refused_case_insensitively() {
        let e = env();
        create(&e, vanilla("Survival", "1.21.4")).await.unwrap();
        for dup in ["Survival", "survival", "  SURVIVAL  "] {
            let r = create(&e, vanilla(dup, "1.20.1")).await;
            assert!(matches!(r, Err(ProfileError::DuplicateName(_))), "{dup:?}");
        }
        assert!(matches!(
            create(&e, vanilla("   ", "1.21.4")).await,
            Err(ProfileError::Instance(InstanceError::BadName(_)))
        ));
        assert!(matches!(
            create(&e, vanilla(&"x".repeat(65), "1.21.4")).await,
            Err(ProfileError::Instance(InstanceError::BadName(_)))
        ));
        // Names that slug to the same folder still get distinct, conflict-free ids.
        let a = create(&e, vanilla("My World!", "1.21.4")).await.unwrap();
        let b = create(&e, vanilla("My-World?", "1.21.4")).await.unwrap();
        let ids: Vec<_> = b.profiles.iter().map(|p| p.id.clone()).collect();
        assert_eq!(ids.len(), a.profiles.len() + 1);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            ids.len()
        );
    }

    #[tokio::test]
    async fn works_offline_from_saved_catalogs_and_refuses_when_nothing_was_ever_saved() {
        let e = env();
        // Online once: both catalogs get saved in the portable cache.
        let loaders = e.loaders.calls().len();
        create(
            &e,
            modded(
                "Online",
                "1.21.4",
                LoaderKind::Fabric,
                LoaderVersionSpec::Auto,
            ),
        )
        .await
        .unwrap();
        assert!(e.loaders.calls().len() > loaders);
        assert!(manifest_cache(&e.root).is_file());

        // Now offline: saved copies are good enough to verify and create.
        e.global.fail(mc_url());
        e.loaders.fail_all();
        let r = create_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &e.settings,
            modded(
                "Offline",
                "1.21.4",
                LoaderKind::Fabric,
                LoaderVersionSpec::Auto,
            ),
            999_999,
        )
        .await;
        assert!(r.is_ok(), "{r:?}");
        // But a Minecraft version / loader combination never seen online cannot be verified.
        let r = create_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &e.settings,
            modded(
                "Never seen",
                "1.21.4",
                LoaderKind::Quilt,
                LoaderVersionSpec::Auto,
            ),
            999_999,
        )
        .await;
        assert!(matches!(r, Err(ProfileError::CannotVerify(_))), "{r:?}");

        // A fresh launcher folder, offline, with no saved catalog: refused, not guessed.
        let fresh = env();
        fresh.global.fail_all();
        let r = create(&fresh, vanilla("Nope", "1.21.4")).await;
        assert!(matches!(r, Err(ProfileError::CannotVerify(_))));
        assert!(snapshot(&fresh.root, None).unwrap().profiles.is_empty());
    }

    #[tokio::test]
    async fn offline_creation_never_accepts_a_loader_that_was_only_checked_for_another_minecraft_version(
    ) {
        // Regression for a real bug: Forge/NeoForge saved an already-filtered list for the first
        // Minecraft version asked about and reused it for every other version while offline.
        let e = env();
        create(
            &e,
            modded(
                "Online",
                "1.21.4",
                LoaderKind::NeoForge,
                LoaderVersionSpec::Auto,
            ),
        )
        .await
        .unwrap();
        create(
            &e,
            modded(
                "Online forge",
                "1.21.4",
                LoaderKind::Forge,
                LoaderVersionSpec::Auto,
            ),
        )
        .await
        .unwrap();
        e.global.fail(mc_url());
        e.loaders.fail_all();
        let offline = |req: NewInstance| {
            create_verified(&e.global, &e.loaders, &e.root, &e.settings, req, 999_999)
        };

        // NeoForge has nothing for 1.20.1, so it must be refused rather than given 1.21.4's version.
        let r = offline(modded(
            "N",
            "1.20.1",
            LoaderKind::NeoForge,
            LoaderVersionSpec::Auto,
        ))
        .await;
        assert!(
            matches!(r, Err(ProfileError::NoCompatibleLoader { .. })),
            "{r:?}"
        );
        // Forge's own 1.20.1 version is accepted, 1.21.4's is not.
        let wrong = LoaderVersionSpec::Pinned {
            version: LoaderVersion::new("54.1.14").unwrap(),
        };
        assert!(matches!(
            offline(modded("F1", "1.20.1", LoaderKind::Forge, wrong)).await,
            Err(ProfileError::LoaderVersionNotOffered { .. })
        ));
        let right = LoaderVersionSpec::Pinned {
            version: LoaderVersion::new("47.4.10").unwrap(),
        };
        assert!(offline(modded("F2", "1.20.1", LoaderKind::Forge, right))
            .await
            .is_ok());
        // A per-version loader never checked for 1.20.1 (Fabric) cannot be verified at all.
        assert!(matches!(
            offline(modded(
                "Fa",
                "1.20.1",
                LoaderKind::Fabric,
                LoaderVersionSpec::Auto
            ))
            .await,
            Err(ProfileError::CannotVerify(_))
        ));
    }

    #[tokio::test]
    async fn snapshot_reports_real_playtime_and_repairs_a_stale_selection() {
        let e = env();
        let a = create(&e, vanilla("First", "1.21.4")).await.unwrap();
        let b = create(&e, vanilla("Second", "1.20.1")).await.unwrap();
        let (id_a, id_b) = (a.profiles[0].id.clone(), b.profiles[1].id.clone());

        // Real playtime through the real playtime store.
        let store = PlaytimeStore::new(&e.root);
        let t0 = std::time::Instant::now();
        let mut s = store
            .begin_session(&InstanceId::new(&id_a).unwrap(), 1, 0, t0)
            .unwrap();
        for m in 1..=3 {
            s.tick(0, t0 + std::time::Duration::from_secs(60 * m))
                .unwrap();
        }
        s.finish(&store, 0, t0 + std::time::Duration::from_secs(200))
            .unwrap();

        let snap = snapshot(&e.root, Some(InstanceId::new(&id_b).unwrap())).unwrap();
        assert_eq!(snap.selected.as_deref(), Some(id_b.as_str()));
        assert_eq!(snap.total_playtime_seconds, 200);
        assert_eq!(
            snap.profiles
                .iter()
                .find(|p| p.id == id_a)
                .unwrap()
                .playtime_seconds,
            200
        );
        assert_eq!(
            snap.profiles
                .iter()
                .find(|p| p.id == id_b)
                .unwrap()
                .playtime_seconds,
            0
        );

        // A selection that does not exist falls back to the first profile instead of failing.
        let snap = snapshot(&e.root, Some(InstanceId::new("deleted-one").unwrap())).unwrap();
        assert_eq!(snap.selected.as_deref(), Some(id_a.as_str()));
        // And with no profiles there is simply no selection and zero playtime.
        let empty = env();
        let snap = snapshot(&empty.root, None).unwrap();
        assert_eq!(
            (
                snap.profiles.len(),
                snap.selected,
                snap.total_playtime_seconds
            ),
            (0, None, 0)
        );
    }

    #[tokio::test]
    async fn old_and_odd_minecraft_ids_are_creatable_when_they_are_in_the_official_catalog() {
        let e = env();
        for (n, v) in [
            ("Old", "b1.7.3"),
            ("Spaced", "1.14 Pre-Release 1"),
            ("Snap", "25w01a"),
        ] {
            create(&e, vanilla(n, v)).await.unwrap();
        }
        assert_eq!(snapshot(&e.root, None).unwrap().profiles.len(), 3);
    }

    #[test]
    fn dto_wire_format_is_camel_case_with_snake_case_enums() {
        let json = serde_json::to_value(ProfileDto {
            id: "a".into(),
            name: "n".into(),
            minecraft: "1.21.4".into(),
            kind: "modded",
            loader: Some(LoaderKind::NeoForge),
            loader_version: Some("21.4.158".into()),
            ready: false,
            playtime_seconds: 5,
        })
        .unwrap();
        assert_eq!(json["playtimeSeconds"], 5);
        assert_eq!(json["loader"], "neoforge");
        assert_eq!(json["loaderVersion"], "21.4.158");
        let cat = serde_json::to_value(catalog_dto(
            mc_catalog::parse_manifest(MANIFEST.as_bytes(), 1)
                .unwrap()
                .catalog,
            Origin::CacheStale,
            Some("x".into()),
        ))
        .unwrap();
        assert_eq!(cat["origin"], "cache_stale");
        assert_eq!(cat["stale"], true);
        assert_eq!(cat["latestRelease"], "1.21.4");
        assert!(cat["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "old_beta" && e["releaseTime"].is_string()));
    }
}
