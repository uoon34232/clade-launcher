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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

use crate::coded_error::{param, params, CodedError};
use crate::instances::{
    Instance, InstanceError, InstanceId, InstanceStore, LoaderVersionSpec, NewInstance, Profile,
    Resolution, SetupState, MAX_RESOLUTION_HEIGHT, MAX_RESOLUTION_WIDTH, MIN_RESOLUTION_HEIGHT,
    MIN_RESOLUTION_WIDTH,
};
use crate::loader_options::{self, LoaderOptions, Status};
use crate::loaders::{Channel, LoaderKind};
use crate::mc_catalog::{self, CatalogError, Origin, VersionCatalog, VersionKind};
use crate::net::{Fetcher, HttpFetcher};
use crate::playtime::PlaytimeStore;
use crate::portable::PortableRoot;
use crate::profile_lock::{Activity, ProfileLocks};
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
    /// Serialises concurrent `create_profile` and `rename_profile` calls.
    pub create_lock: tokio::sync::Mutex<()>,
    /// Per-profile activity locks shared with preparation and launch.
    pub locks: Arc<ProfileLocks>,
}

impl ProfilesState {
    pub fn new(
        root: PortableRoot,
        global: Result<HttpFetcher, String>,
        loaders: Result<HttpFetcher, String>,
    ) -> Self {
        Self {
            root,
            global,
            loaders,
            create_lock: tokio::sync::Mutex::new(()),
            locks: Arc::new(ProfileLocks::default()),
        }
    }

    pub fn with_locks(mut self, locks: Arc<ProfileLocks>) -> Self {
        self.locks = locks;
        self
    }
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
    pub memory_mb: Option<u32>,
    pub resolution: Option<Resolution>,
    pub jvm_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedProfileDto {
    pub folder: String,
    pub name: String,
    pub minecraft: String,
    pub kind: &'static str,
    pub loader: Option<LoaderKind>,
    pub loader_version: Option<String>,
    pub playtime_seconds: u64,
    pub removed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesDto {
    pub profiles: Vec<ProfileDto>,
    pub selected: Option<String>,
    pub total_playtime_seconds: u64,
    pub unreadable: Vec<String>,
    pub removed: Vec<RemovedProfileDto>,
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
            Self::Instance(InstanceError::BadJvmArg(_)) => {
                CodedError::new("invalid_jvm_args", message)
            }
            Self::Instance(InstanceError::BadResolution(_)) => {
                CodedError::new("invalid_resolution", message).with(params([
                    ("minWidth", MIN_RESOLUTION_WIDTH.into()),
                    ("maxWidth", MAX_RESOLUTION_WIDTH.into()),
                    ("minHeight", MIN_RESOLUTION_HEIGHT.into()),
                    ("maxHeight", MAX_RESOLUTION_HEIGHT.into()),
                ]))
            }
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
                memory_mb: i.memory_mb,
                resolution: i.resolution,
                jvm_args: i.extra_jvm_args.clone(),
            }
        })
        .collect();
    let mut unreadable: Vec<String> = listing
        .problems
        .into_iter()
        .map(|(folder, _)| folder)
        .collect();
    unreadable.sort();
    let retained_dir = root.instances_dir().join(crate::instances::RETAINED_DIR);
    let removed: Vec<RemovedProfileDto> = store_list_retained(root)
        .into_iter()
        .map(|r| {
            let (loader_version, _) = label(&r.instance);
            let playtime_seconds = PlaytimeStore::dir_total(&retained_dir.join(&r.folder))
                .map(|t| t.total_seconds)
                .unwrap_or(0);
            RemovedProfileDto {
                folder: r.folder,
                name: r.instance.name.clone(),
                minecraft: r.instance.minecraft.to_string(),
                kind: if r.instance.is_vanilla() {
                    "vanilla"
                } else {
                    "modded"
                },
                loader: r.instance.loader(),
                loader_version,
                playtime_seconds,
                removed_at: r.removed_at,
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
        unreadable,
        removed,
    })
}

fn store_list_retained(root: &PortableRoot) -> Vec<crate::instances::RetainedInstance> {
    match InstanceStore::new(root).list_retained() {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error = %e, "could not list retained profiles");
            Vec::new()
        }
    }
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

fn check_name_and_problems(
    listing: &crate::instances::Listing,
    raw_name: &str,
) -> Result<(), ProfileError> {
    let trimmed = raw_name.trim();
    let wanted = trimmed.to_lowercase();
    if listing
        .instances
        .iter()
        .any(|i| i.name.trim().to_lowercase() == wanted)
    {
        return Err(ProfileError::DuplicateName(trimmed.to_owned()));
    }
    if !trimmed.is_empty() {
        let slug = crate::instances::slugify(trimmed);
        if listing
            .problems
            .iter()
            .any(|(folder, _)| folder.eq_ignore_ascii_case(slug.as_str()))
        {
            return Err(ProfileError::Instance(InstanceError::Io(
                std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("profile folder '{slug}' already exists and could not be read"),
                ),
            )));
        }
    }
    Ok(())
}

async fn verify_version_and_loader<G: Fetcher, L: Fetcher>(
    global: &G,
    loaders: &L,
    root: &PortableRoot,
    minecraft: &McVersion,
    profile: &Profile,
    now: i64,
) -> Result<(), ProfileError> {
    let catalog = mc_catalog::load(global, &manifest_cache(root), now, false)
        .await
        .map_err(|e| {
            tracing::info!(error = %e, "the Minecraft version list could not be checked");
            ProfileError::CannotVerify(None)
        })?;
    if !catalog.catalog.contains(minecraft) {
        return Err(ProfileError::UnknownMinecraft(minecraft.to_string()));
    }

    if let Profile::Modded {
        loader,
        loader_version,
    } = profile
    {
        let opts =
            loader_options::options_for(loaders, &catalogs_dir(root), *loader, minecraft, now)
                .await;
        let name = loader.name();
        match opts.status {
            Status::Unavailable => {
                return Err(ProfileError::CannotVerify(Some((
                    name,
                    minecraft.to_string(),
                ))))
            }
            Status::None => {
                return Err(ProfileError::NoCompatibleLoader {
                    loader: name,
                    minecraft: minecraft.to_string(),
                })
            }
            Status::Ok => {}
        }
        if let LoaderVersionSpec::Pinned { version } = loader_version {
            if !opts.versions.iter().any(|o| &o.version == version) {
                return Err(ProfileError::LoaderVersionNotOffered {
                    loader: name,
                    version: version.to_string(),
                    minecraft: minecraft.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Strips a trailing `" (N)"` (`N >= 2`) from `name` so duplicating `"Survival (2)"` yields
/// `"Survival (3)"` rather than `"Survival (2) (2)"`.
fn strip_copy_suffix(name: &str) -> &str {
    let trimmed = name.trim();
    if let Some(open) = trimmed.rfind(" (") {
        let rest = &trimmed[open + 2..];
        if let Some(digits) = rest.strip_suffix(')') {
            if !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && digits.parse::<u32>().is_ok_and(|n| n >= 2)
            {
                let stem = trimmed[..open].trim_end();
                if !stem.is_empty() {
                    return stem;
                }
            }
        }
    }
    trimmed
}

/// Allocates the next conflict-free `"Name (2)"`, `"Name (3)"`, ... within the 64-character limit.
pub(crate) fn next_duplicate_name(
    source_name: &str,
    listing: &crate::instances::Listing,
) -> String {
    let stem = strip_copy_suffix(source_name);
    for n in 2u32..=9_999 {
        let suffix = format!(" ({n})");
        let max_stem = 64usize.saturating_sub(suffix.chars().count());
        let head: String = stem.chars().take(max_stem).collect();
        let head = head.trim_end();
        let candidate = if head.is_empty() {
            format!("Profile{suffix}")
        } else {
            format!("{head}{suffix}")
        };
        if check_name_and_problems(listing, &candidate).is_ok() {
            return candidate;
        }
    }
    "Profile (copy)".to_owned()
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
    // 1. Friendly case-insensitive duplicate check and unreadable-folder collision check.
    let root_for_check = root.clone();
    let name_for_check = request.name.clone();
    tokio::task::spawn_blocking(move || -> Result<(), ProfileError> {
        let listing = InstanceStore::new(&root_for_check).list()?;
        check_name_and_problems(&listing, &name_for_check)
    })
    .await
    .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))??;

    // 2 & 3. Official Minecraft and (when Modded) loader catalog checks.
    verify_version_and_loader(
        global,
        loaders,
        root,
        &request.minecraft,
        &request.profile,
        now,
    )
    .await?;

    // 4. Re-check, create (isolated, Pending), persist selection and snapshot off Tokio workers.
    let root_for_create = root.clone();
    let settings_for_create = settings.clone();
    tokio::task::spawn_blocking(move || -> Result<ProfilesDto, ProfileError> {
        let store = InstanceStore::new(&root_for_create);
        let listing = store.list()?;
        check_name_and_problems(&listing, &request.name)?;
        let created = store.create(request, now)?;
        if let Err(e) = settings_for_create.set_last_instance(Some(created.id.clone())) {
            tracing::warn!(error = %e, "could not remember the new profile as selected");
        }
        Ok(snapshot(&root_for_create, Some(created.id))?)
    })
    .await
    .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))?
}

pub struct VersionUpdate<'a> {
    pub id: &'a str,
    pub minecraft: &'a str,
    pub loader_version: Option<LoaderVersionSpec>,
}

/// Verifies and updates an existing profile's Minecraft (and optional loader) version while
/// keeping its `Vanilla` vs `Modded` type and all `game/` files intact.
pub async fn set_version_verified<G: Fetcher, L: Fetcher>(
    global: &G,
    loaders: &L,
    root: &PortableRoot,
    locks: &Arc<ProfileLocks>,
    selected: Option<InstanceId>,
    update: VersionUpdate<'_>,
    now: i64,
) -> Result<ProfilesDto, CodedError> {
    let iid =
        InstanceId::new(update.id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let mc = McVersion::new(update.minecraft).map_err(|e| {
        CodedError::new("unknown_minecraft", e.to_string())
            .with(param("minecraft", update.minecraft.to_owned()))
    })?;
    let held = claim_profile_lock(locks, &iid)?;

    let root_read = root.clone();
    let iid_read = iid.clone();
    let current = tokio::task::spawn_blocking(move || {
        InstanceStore::new(&root_read)
            .get(&iid_read)
            .map_err(|e| ProfileError::Instance(e).coded())
    })
    .await
    .map_err(|e| CodedError::storage(format!("profile read task failed: {e}")))??;

    let target_profile = match (&current.profile, &update.loader_version) {
        (Profile::Vanilla, None) => Profile::Vanilla,
        (Profile::Vanilla, Some(_)) => {
            return Err(CodedError::storage(
                "a Vanilla profile cannot specify a loader version",
            ))
        }
        (Profile::Modded { loader, .. }, spec) => Profile::Modded {
            loader: *loader,
            loader_version: spec.clone().unwrap_or(LoaderVersionSpec::Auto),
        },
    };

    verify_version_and_loader(global, loaders, root, &mc, &target_profile, now)
        .await
        .map_err(|e| e.coded())?;

    let root_write = root.clone();
    let loader_version = update.loader_version;
    tokio::task::spawn_blocking(move || {
        let _held = held;
        InstanceStore::new(&root_write)
            .set_version(&iid, mc, loader_version)
            .map_err(|e| ProfileError::Instance(e).coded())?;
        snapshot(&root_write, selected).map_err(CodedError::storage)
    })
    .await
    .map_err(|e| CodedError::storage(format!("profile version task failed: {e}")))?
}

pub(crate) struct HeldProfileLock {
    locks: Arc<ProfileLocks>,
    id: String,
}

impl Drop for HeldProfileLock {
    fn drop(&mut self) {
        self.locks.release(&self.id);
    }
}

pub(crate) fn claim_profile_lock(
    locks: &Arc<ProfileLocks>,
    id: &InstanceId,
) -> Result<HeldProfileLock, CodedError> {
    match locks.claim(id.as_str(), Activity::Preparing) {
        Ok(()) => Ok(HeldProfileLock {
            locks: locks.clone(),
            id: id.as_str().to_owned(),
        }),
        Err(Activity::Launching) => Err(CodedError::new(
            "already_running",
            "This profile is starting or running.",
        )),
        Err(Activity::Preparing) => Err(CodedError::new("busy", "This profile is being set up.")),
    }
}

pub(crate) fn rename_in_store(
    root: &PortableRoot,
    iid: &InstanceId,
    name: &str,
    selected: Option<InstanceId>,
) -> Result<ProfilesDto, CodedError> {
    let store = InstanceStore::new(root);
    store
        .get(iid)
        .map_err(|e| ProfileError::Instance(e).coded())?;
    let listing = store.list().map_err(CodedError::storage)?;
    let trimmed = name.trim();
    let wanted = trimmed.to_lowercase();
    if listing
        .instances
        .iter()
        .filter(|i| i.id != *iid)
        .any(|i| i.name.trim().to_lowercase() == wanted)
    {
        return Err(ProfileError::DuplicateName(trimmed.to_owned()).coded());
    }
    store
        .rename(iid, name)
        .map_err(|e| ProfileError::Instance(e).coded())?;
    snapshot(root, selected).map_err(CodedError::storage)
}

pub(crate) fn set_memory_in_store(
    root: &PortableRoot,
    iid: &InstanceId,
    mb: Option<u32>,
    total_mb: Option<u64>,
    selected: Option<InstanceId>,
) -> Result<ProfilesDto, CodedError> {
    if let Some(m) = mb {
        let (min, max) = crate::system::memory_limits_mb(total_mb);
        if m < min || m > max || !m.is_multiple_of(256) {
            return Err(
                CodedError::new("invalid_memory", "memory is outside the safe range")
                    .with(params([("min", min.into()), ("max", max.into())])),
            );
        }
    }
    InstanceStore::new(root)
        .set_memory(iid, mb)
        .map_err(|e| ProfileError::Instance(e).coded())?;
    snapshot(root, selected).map_err(CodedError::storage)
}

pub(crate) fn resolve_profile_game_dir(
    root: &PortableRoot,
    iid: &InstanceId,
) -> Result<PathBuf, CodedError> {
    let store = InstanceStore::new(root);
    store
        .get(iid)
        .map_err(|e| ProfileError::Instance(e).coded())?;
    let dir = store.dirs(iid).game_dir();
    crate::portable::fsutil::reject_links_between(root.path(), &dir)
        .map_err(CodedError::storage)?;
    std::fs::create_dir_all(&dir).map_err(CodedError::storage)?;
    Ok(dir)
}

pub(crate) fn resolve_instances_dir(root: &PortableRoot) -> Result<PathBuf, CodedError> {
    let dir = root.instances_dir();
    crate::portable::fsutil::reject_links_between(root.path(), &dir)
        .map_err(CodedError::storage)?;
    std::fs::create_dir_all(&dir).map_err(CodedError::storage)?;
    Ok(dir)
}

pub(crate) fn resolve_removed_profiles_dir(root: &PortableRoot) -> Result<PathBuf, CodedError> {
    let inst = resolve_instances_dir(root)?;
    let dir = inst.join(crate::instances::RETAINED_DIR);
    std::fs::create_dir_all(&dir).map_err(CodedError::storage)?;
    crate::portable::fsutil::reject_links_between(root.path(), &dir)
        .map_err(CodedError::storage)?;
    if crate::portable::fsutil::is_link(&dir).map_err(CodedError::storage)? {
        return Err(CodedError::storage("removed profiles folder is a link"));
    }
    Ok(dir)
}

pub(crate) async fn restore_profile_inner(
    state: &ProfilesState,
    settings: &SettingsState,
    folder: &str,
) -> Result<ProfilesDto, CodedError> {
    let _guard = state.create_lock.lock().await;
    let root = state.root.clone();
    let folder = folder.to_owned();
    let settings_for_restore = settings.clone();
    tokio::task::spawn_blocking(move || {
        let store = InstanceStore::new(&root);
        let retained = store.read_retained(&folder).map_err(|e| match e {
            InstanceError::NotFound(_) | InstanceError::BadId(_) => {
                CodedError::new("not_found", e.to_string())
            }
            _ => CodedError::new("restore_failed", e.to_string()),
        })?;
        let listing = store.list().map_err(CodedError::storage)?;
        let final_name = if listing.instances.iter().any(|i| {
            i.name
                .trim()
                .eq_ignore_ascii_case(retained.instance.name.trim())
        }) {
            next_duplicate_name(&retained.instance.name, &listing)
        } else {
            retained.instance.name.clone()
        };
        let restored = store.restore(&folder, &final_name).map_err(|e| match e {
            InstanceError::NotFound(_) | InstanceError::BadId(_) => {
                CodedError::new("not_found", e.to_string())
            }
            _ => CodedError::new("restore_failed", e.to_string()),
        })?;
        if let Err(e) = settings_for_restore.set_last_instance(Some(restored.id.clone())) {
            tracing::warn!(error = %e, "could not remember the restored profile as selected");
        }
        snapshot(&root, Some(restored.id)).map_err(CodedError::storage)
    })
    .await
    .map_err(|e| {
        CodedError::new(
            "restore_failed",
            format!("profile restore task failed: {e}"),
        )
    })?
}

pub(crate) async fn rename_profile_inner(
    state: &ProfilesState,
    settings: &SettingsState,
    id: &str,
    name: &str,
) -> Result<ProfilesDto, CodedError> {
    let _guard = state.create_lock.lock().await;
    let iid = InstanceId::new(id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let held = claim_profile_lock(&state.locks, &iid)?;
    let root = state.root.clone();
    let name = name.to_owned();
    let selected = settings.last_instance();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        rename_in_store(&root, &iid, &name, selected)
    })
    .await
    .map_err(|e| CodedError::storage(format!("profile rename task failed: {e}")))?
}

pub(crate) async fn set_profile_memory_inner(
    state: &ProfilesState,
    settings: &SettingsState,
    id: &str,
    mb: Option<u32>,
) -> Result<ProfilesDto, CodedError> {
    let iid = InstanceId::new(id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let held = claim_profile_lock(&state.locks, &iid)?;
    let root = state.root.clone();
    let selected = settings.last_instance();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        let total = crate::system::total_memory_mb();
        set_memory_in_store(&root, &iid, mb, total, selected)
    })
    .await
    .map_err(|e| CodedError::storage(format!("profile memory task failed: {e}")))?
}

pub(crate) async fn set_profile_resolution_inner(
    state: &ProfilesState,
    settings: &SettingsState,
    id: &str,
    resolution: Option<Resolution>,
) -> Result<ProfilesDto, CodedError> {
    let iid = InstanceId::new(id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let held = claim_profile_lock(&state.locks, &iid)?;
    let root = state.root.clone();
    let selected = settings.last_instance();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        InstanceStore::new(&root)
            .set_resolution(&iid, resolution)
            .map_err(|e| ProfileError::Instance(e).coded())?;
        snapshot(&root, selected).map_err(CodedError::storage)
    })
    .await
    .map_err(|e| CodedError::storage(format!("profile resolution task failed: {e}")))?
}

pub(crate) async fn set_profile_jvm_args_inner(
    state: &ProfilesState,
    settings: &SettingsState,
    id: &str,
    args: Vec<String>,
) -> Result<ProfilesDto, CodedError> {
    let iid = InstanceId::new(id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let held = claim_profile_lock(&state.locks, &iid)?;
    let root = state.root.clone();
    let selected = settings.last_instance();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        InstanceStore::new(&root)
            .set_jvm_args(&iid, args)
            .map_err(|e| ProfileError::Instance(e).coded())?;
        snapshot(&root, selected).map_err(CodedError::storage)
    })
    .await
    .map_err(|e| CodedError::storage(format!("profile jvm args task failed: {e}")))?
}

pub(crate) async fn duplicate_profile_inner(
    state: &ProfilesState,
    settings: &SettingsState,
    id: &str,
    now: i64,
) -> Result<ProfilesDto, CodedError> {
    let _guard = state.create_lock.lock().await;
    let iid = InstanceId::new(id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let held = claim_profile_lock(&state.locks, &iid)?;
    let root = state.root.clone();
    let settings_for_dup = settings.clone();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        let store = InstanceStore::new(&root);
        let source = store
            .get(&iid)
            .map_err(|e| ProfileError::Instance(e).coded())?;
        let listing = store.list().map_err(CodedError::storage)?;
        let copy_name = next_duplicate_name(&source.name, &listing);
        let created = store
            .duplicate(&iid, &copy_name, now)
            .map_err(|e| match e {
                InstanceError::NotFound(_) | InstanceError::BadId(_) => {
                    CodedError::new("not_found", e.to_string())
                }
                _ => CodedError::new("duplicate_failed", e.to_string()),
            })?;
        if let Err(e) = settings_for_dup.set_last_instance(Some(created.id.clone())) {
            tracing::warn!(error = %e, "could not remember the duplicated profile as selected");
        }
        snapshot(&root, Some(created.id)).map_err(CodedError::storage)
    })
    .await
    .map_err(|e| {
        CodedError::new(
            "duplicate_failed",
            format!("profile duplicate task failed: {e}"),
        )
    })?
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
pub async fn list_profiles(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
) -> Result<ProfilesDto, CodedError> {
    let root = state.root.clone();
    let selected = settings.last_instance();
    tokio::task::spawn_blocking(move || snapshot(&root, selected))
        .await
        .map_err(|e| CodedError::storage(format!("profile listing task failed: {e}")))?
        .map_err(CodedError::storage)
}

#[tauri::command]
pub async fn select_profile(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
) -> Result<(), CodedError> {
    let id = InstanceId::new(&id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let root = state.root.clone();
    let st = settings.inner().clone();
    tokio::task::spawn_blocking(move || {
        InstanceStore::new(&root)
            .get(&id)
            .map_err(|e| CodedError::new("not_found", e.to_string()))?;
        st.set_last_instance(Some(id)).map_err(CodedError::storage)
    })
    .await
    .map_err(|e| CodedError::storage(format!("profile selection task failed: {e}")))?
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
    let _guard = state.create_lock.lock().await;
    let (g, l) = (fetcher(&state.global)?, fetcher(&state.loaders)?);
    create_verified(g, l, &state.root, &settings, request, now_unix())
        .await
        .map_err(|e| e.coded())
}

#[tauri::command]
pub async fn rename_profile(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
    name: String,
) -> Result<ProfilesDto, CodedError> {
    rename_profile_inner(&state, &settings, &id, &name).await
}

#[tauri::command]
pub async fn set_profile_memory(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
    mb: Option<u32>,
) -> Result<ProfilesDto, CodedError> {
    set_profile_memory_inner(&state, &settings, &id, mb).await
}

#[tauri::command]
pub async fn set_profile_version(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
    minecraft: String,
    loader_version: Option<LoaderVersionSpec>,
) -> Result<ProfilesDto, CodedError> {
    let (g, l) = (fetcher(&state.global)?, fetcher(&state.loaders)?);
    set_version_verified(
        g,
        l,
        &state.root,
        &state.locks,
        settings.last_instance(),
        VersionUpdate {
            id: &id,
            minecraft: &minecraft,
            loader_version,
        },
        now_unix(),
    )
    .await
}

#[tauri::command]
pub async fn duplicate_profile(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
) -> Result<ProfilesDto, CodedError> {
    duplicate_profile_inner(&state, &settings, &id, now_unix()).await
}

#[tauri::command]
pub async fn set_profile_resolution(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
    resolution: Option<Resolution>,
) -> Result<ProfilesDto, CodedError> {
    set_profile_resolution_inner(&state, &settings, &id, resolution).await
}

#[tauri::command]
pub async fn set_profile_jvm_args(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    id: String,
    args: Vec<String>,
) -> Result<ProfilesDto, CodedError> {
    set_profile_jvm_args_inner(&state, &settings, &id, args).await
}

#[tauri::command]
pub async fn open_profile_folder(
    app: tauri::AppHandle,
    state: State<'_, ProfilesState>,
    id: String,
) -> Result<(), CodedError> {
    let iid = InstanceId::new(&id).map_err(|e| CodedError::new("not_found", e.to_string()))?;
    let root = state.root.clone();
    tokio::task::spawn_blocking(move || {
        let dir = resolve_profile_game_dir(&root, &iid)?;
        app.opener()
            .open_path(dir.to_string_lossy(), None::<&str>)
            .map_err(|e| CodedError::new("open_failed", e.to_string()))
    })
    .await
    .map_err(|e| CodedError::new("open_failed", e.to_string()))?
}

#[tauri::command]
pub async fn open_instances_folder(
    app: tauri::AppHandle,
    state: State<'_, ProfilesState>,
) -> Result<(), CodedError> {
    let root = state.root.clone();
    tokio::task::spawn_blocking(move || {
        let dir = resolve_instances_dir(&root)?;
        app.opener()
            .open_path(dir.to_string_lossy(), None::<&str>)
            .map_err(|e| CodedError::new("open_failed", e.to_string()))
    })
    .await
    .map_err(|e| CodedError::new("open_failed", e.to_string()))?
}

#[tauri::command]
pub async fn restore_profile(
    state: State<'_, ProfilesState>,
    settings: State<'_, SettingsState>,
    folder: String,
) -> Result<ProfilesDto, CodedError> {
    restore_profile_inner(&state, &settings, &folder).await
}

#[tauri::command]
pub async fn open_removed_profiles_folder(
    app: tauri::AppHandle,
    state: State<'_, ProfilesState>,
) -> Result<(), CodedError> {
    let root = state.root.clone();
    tokio::task::spawn_blocking(move || {
        let dir = resolve_removed_profiles_dir(&root)?;
        app.opener()
            .open_path(dir.to_string_lossy(), None::<&str>)
            .map_err(|e| CodedError::new("open_failed", e.to_string()))
    })
    .await
    .map_err(|e| CodedError::new("open_failed", e.to_string()))?
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
            resolution: None,
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
            memory_mb: Some(4096),
            resolution: Some(Resolution {
                width: 1280,
                height: 720,
            }),
            jvm_args: vec!["-XX:+UseStringDeduplication".into()],
        })
        .unwrap();
        assert_eq!(json["playtimeSeconds"], 5);
        assert_eq!(json["memoryMb"], 4096);
        assert_eq!(json["resolution"]["width"], 1280);
        assert_eq!(json["jvmArgs"][0], "-XX:+UseStringDeduplication");
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

    #[tokio::test]
    async fn create_verified_refuses_dash_2_when_slug_collides_with_damaged_folder_in_problems() {
        let e = env();
        let damaged = e.root.instances_dir().join("survival");
        fs::create_dir_all(damaged.join("game/saves/MyWorld")).unwrap();
        fs::write(damaged.join("game/saves/MyWorld/level.dat"), b"world-bytes").unwrap();
        fs::write(damaged.join("instance.json"), b"{corrupt").unwrap();

        let listing = InstanceStore::new(&e.root).list().unwrap();
        assert_eq!(listing.problems.len(), 1);
        assert_eq!(listing.problems[0].0, "survival");

        let err = create(&e, vanilla("Survival", "1.21.4")).await.unwrap_err();
        assert_eq!(err.coded().code, "storage");
        assert!(
            e.global.calls().is_empty(),
            "colliding with a damaged profile folder fails before any network call"
        );
        assert!(!e.root.instances_dir().join("survival-2").exists());
        assert_eq!(
            fs::read(damaged.join("game/saves/MyWorld/level.dat")).unwrap(),
            b"world-bytes"
        );

        let st = ProfilesState::new(e.root.clone(), Err("offline".into()), Err("offline".into()));
        let _guard = st.create_lock.lock().await;
    }

    #[tokio::test]
    async fn snapshot_reports_memory_mb_and_sorted_unreadable_folders_ignoring_removed() {
        let e = env();
        let a = create(&e, vanilla("First", "1.21.4")).await.unwrap();
        let mut req_b = vanilla("Second", "1.20.1");
        req_b.memory_mb = Some(4096);
        let b = create(&e, req_b).await.unwrap();
        let (id_a, id_b) = (a.profiles[0].id.clone(), b.profiles[1].id.clone());

        let inst_dir = e.root.instances_dir();
        fs::create_dir_all(inst_dir.join("zeta-corrupt")).unwrap();
        fs::write(inst_dir.join("zeta-corrupt/instance.json"), b"{broken").unwrap();
        fs::create_dir_all(inst_dir.join("alpha-empty")).unwrap();
        fs::create_dir_all(inst_dir.join("BadUpper")).unwrap();
        fs::create_dir_all(inst_dir.join(".removed/old-world")).unwrap();
        fs::write(inst_dir.join("stray-note.txt"), b"ignored").unwrap();

        let snap = snapshot(&e.root, None).unwrap();
        assert_eq!(
            snap.profiles
                .iter()
                .find(|p| p.id == id_a)
                .unwrap()
                .memory_mb,
            None
        );
        assert_eq!(
            snap.profiles
                .iter()
                .find(|p| p.id == id_b)
                .unwrap()
                .memory_mb,
            Some(4096)
        );
        assert_eq!(
            snap.unreadable,
            vec!["BadUpper", "alpha-empty", "zeta-corrupt"]
        );

        let wire = serde_json::to_value(&snap).unwrap();
        assert_eq!(
            wire["unreadable"],
            serde_json::json!(["BadUpper", "alpha-empty", "zeta-corrupt"])
        );
    }

    fn offline_state(root: &PortableRoot) -> ProfilesState {
        ProfilesState::new(root.clone(), Err("offline".into()), Err("offline".into()))
    }

    #[tokio::test]
    async fn rename_allows_case_only_self_rename_and_ignores_damaged_folder_slug_collision() {
        let e = env();
        let created = create(&e, vanilla("survival", "1.21.4")).await.unwrap();
        let id = created.profiles[0].id.clone();
        assert_eq!(id, "survival");

        let save_file = e
            .root
            .instances_dir()
            .join(&id)
            .join("game/saves/World/level.dat");
        fs::create_dir_all(save_file.parent().unwrap()).unwrap();
        fs::write(&save_file, b"precious-save").unwrap();

        let state = offline_state(&e.root);
        let renamed = rename_profile_inner(&state, &e.settings, &id, "Survival")
            .await
            .unwrap();
        assert_eq!(renamed.profiles.len(), 1);
        assert_eq!(renamed.profiles[0].id, "survival");
        assert_eq!(renamed.profiles[0].name, "Survival");
        assert_eq!(renamed.selected.as_deref(), Some("survival"));
        assert_eq!(fs::read(&save_file).unwrap(), b"precious-save");
        assert!(!e.root.instances_dir().join("survival-2").exists());

        let renamed_upper = rename_profile_inner(&state, &e.settings, &id, "  SURVIVAL  ")
            .await
            .unwrap();
        assert_eq!(renamed_upper.profiles[0].id, "survival");
        assert_eq!(renamed_upper.profiles[0].name, "SURVIVAL");

        // Renaming another profile to a name whose slug matches a damaged folder in `problems`
        // must succeed because rename never moves or allocates directories.
        let second = create(&e, vanilla("Builder", "1.20.1")).await.unwrap();
        let builder_id = second
            .profiles
            .iter()
            .find(|p| p.name == "Builder")
            .unwrap()
            .id
            .clone();
        let damaged = e.root.instances_dir().join("redstone-lab");
        fs::create_dir_all(&damaged).unwrap();
        fs::write(damaged.join("instance.json"), b"{corrupt").unwrap();

        let after = rename_profile_inner(&state, &e.settings, &builder_id, "Redstone Lab")
            .await
            .unwrap();
        let builder = after.profiles.iter().find(|p| p.id == builder_id).unwrap();
        assert_eq!(builder.name, "Redstone Lab");
        assert!(e.root.instances_dir().join(&builder_id).is_dir());
        assert_eq!(after.unreadable, vec!["redstone-lab"]);
    }

    #[tokio::test]
    async fn rename_rejects_duplicates_invalid_names_and_unknown_or_bad_ids() {
        let e = env();
        create(&e, vanilla("First", "1.21.4")).await.unwrap();
        let second = create(&e, vanilla("Second", "1.20.1")).await.unwrap();
        let second_id = second
            .profiles
            .iter()
            .find(|p| p.name == "Second")
            .unwrap()
            .id
            .clone();

        let state = offline_state(&e.root);

        for (raw, expected_trimmed) in [
            ("first", "first"),
            ("FIRST", "FIRST"),
            ("  First  ", "First"),
        ] {
            let err = rename_profile_inner(&state, &e.settings, &second_id, raw)
                .await
                .unwrap_err();
            assert_eq!(err.code, "duplicate_name");
            assert_eq!(err.params.as_ref().unwrap()["name"], expected_trimmed);
        }

        for bad_name in ["", "   ", &"x".repeat(65), "bad\nname"] {
            let err = rename_profile_inner(&state, &e.settings, &second_id, bad_name)
                .await
                .unwrap_err();
            assert_eq!(err.code, "invalid_name", "name={bad_name:?}");
        }

        assert_eq!(
            InstanceStore::new(&e.root)
                .get(&InstanceId::new(&second_id).unwrap())
                .unwrap()
                .name,
            "Second"
        );

        for bad_id in ["missing-profile", "../escape", "UPPER", ""] {
            let err = rename_profile_inner(&state, &e.settings, bad_id, "Valid Name")
                .await
                .unwrap_err();
            assert_eq!(err.code, "not_found", "id={bad_id:?}");
        }
    }

    #[tokio::test]
    async fn profile_locks_guard_rename_and_set_memory_and_release_on_drop() {
        let e = env();
        let snap = create(&e, vanilla("Locked", "1.21.4")).await.unwrap();
        let id = snap.profiles[0].id.clone();
        let iid = InstanceId::new(&id).unwrap();

        let locks = Arc::new(ProfileLocks::default());
        let state = offline_state(&e.root).with_locks(locks.clone());

        {
            let _held = claim_profile_lock(&locks, &iid).unwrap();
            assert_eq!(locks.current(&id), Some(Activity::Preparing));
        }
        assert_eq!(locks.current(&id), None);

        locks.claim(&id, Activity::Launching).unwrap();
        assert_eq!(
            rename_profile_inner(&state, &e.settings, &id, "Other")
                .await
                .unwrap_err()
                .code,
            "already_running"
        );
        assert_eq!(
            set_profile_memory_inner(&state, &e.settings, &id, None)
                .await
                .unwrap_err()
                .code,
            "already_running"
        );
        assert_eq!(locks.current(&id), Some(Activity::Launching));
        locks.release(&id);

        locks.claim(&id, Activity::Preparing).unwrap();
        assert_eq!(
            rename_profile_inner(&state, &e.settings, &id, "Other")
                .await
                .unwrap_err()
                .code,
            "busy"
        );
        assert_eq!(
            set_profile_memory_inner(&state, &e.settings, &id, None)
                .await
                .unwrap_err()
                .code,
            "busy"
        );
        assert_eq!(locks.current(&id), Some(Activity::Preparing));
        locks.release(&id);

        let ok_rename = rename_profile_inner(&state, &e.settings, &id, "Renamed")
            .await
            .unwrap();
        assert_eq!(ok_rename.profiles[0].name, "Renamed");
        assert_eq!(locks.current(&id), None);

        let err_rename = rename_profile_inner(&state, &e.settings, &id, "   ")
            .await
            .unwrap_err();
        assert_eq!(err_rename.code, "invalid_name");
        assert_eq!(locks.current(&id), None);

        let (min_mb, _) = crate::system::memory_limits_mb(crate::system::total_memory_mb());
        let ok_mem = set_profile_memory_inner(&state, &e.settings, &id, Some(min_mb))
            .await
            .unwrap();
        assert_eq!(ok_mem.profiles[0].memory_mb, Some(min_mb));
        assert_eq!(locks.current(&id), None);

        let err_mem = set_profile_memory_inner(&state, &e.settings, &id, Some(512))
            .await
            .unwrap_err();
        assert_eq!(err_mem.code, "invalid_memory");
        assert_eq!(locks.current(&id), None);

        for bad_id in ["missing-profile", "../escape", "UPPER"] {
            assert_eq!(
                set_profile_memory_inner(&state, &e.settings, bad_id, None)
                    .await
                    .unwrap_err()
                    .code,
                "not_found"
            );
            assert_eq!(locks.current(bad_id), None);
        }
    }

    #[tokio::test]
    async fn set_memory_in_store_validates_bounds_and_step_and_round_trips() {
        let e = env();
        let snap = create(&e, vanilla("MemTest", "1.21.4")).await.unwrap();
        let iid = InstanceId::new(&snap.profiles[0].id).unwrap();

        let total = Some(16_384);
        let (min, max) = crate::system::memory_limits_mb(total);
        assert_eq!((min, max), (1024, 12_288));

        for bad in [0, 512, min - 256, max + 256, 32_768, 4000, 1025] {
            let res = set_memory_in_store(&e.root, &iid, Some(bad), total, Some(iid.clone()));
            let err = res.unwrap_err();
            assert_eq!(err.code, "invalid_memory", "mb={bad}");
            let p = err.params.as_ref().unwrap();
            assert_eq!(p["min"], min);
            assert_eq!(p["max"], max);
        }

        let sel = Some(iid.clone());
        let set_snap = set_memory_in_store(&e.root, &iid, Some(4096), total, sel.clone()).unwrap();
        assert_eq!(set_snap.selected.as_deref(), Some(iid.as_str()));
        assert_eq!(set_snap.profiles[0].memory_mb, Some(4096));
        assert_eq!(
            InstanceStore::new(&e.root).get(&iid).unwrap().memory_mb,
            Some(4096)
        );

        let cleared_snap = set_memory_in_store(&e.root, &iid, None, total, sel).unwrap();
        assert_eq!(cleared_snap.profiles[0].memory_mb, None);
        assert_eq!(
            InstanceStore::new(&e.root).get(&iid).unwrap().memory_mb,
            None
        );

        let missing = InstanceId::new("ghost").unwrap();
        let err = set_memory_in_store(&e.root, &missing, Some(4096), total, None).unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    #[tokio::test]
    async fn resolve_folder_helpers_create_missing_dirs_and_reject_unknown_or_linked_paths() {
        let e = env();
        let snap = create(&e, vanilla("FolderTest", "1.21.4")).await.unwrap();
        let iid = InstanceId::new(&snap.profiles[0].id).unwrap();

        let expected_game = InstanceStore::new(&e.root).dirs(&iid).game_dir();
        fs::remove_dir_all(&expected_game).unwrap();
        assert!(!expected_game.exists());

        let resolved_game = resolve_profile_game_dir(&e.root, &iid).unwrap();
        assert_eq!(resolved_game, expected_game);
        assert!(resolved_game.is_dir());

        let missing = InstanceId::new("missing").unwrap();
        let err = resolve_profile_game_dir(&e.root, &missing).unwrap_err();
        assert_eq!(err.code, "not_found");
        assert!(!e.root.instances_dir().join("missing").exists());

        let empty = env();
        fs::remove_dir_all(empty.root.instances_dir()).unwrap();
        assert!(!empty.root.instances_dir().exists());
        let resolved_instances = resolve_instances_dir(&empty.root).unwrap();
        assert_eq!(resolved_instances, empty.root.instances_dir());
        assert!(resolved_instances.is_dir());

        {
            let outside = e._d.path().join("outside-target");
            fs::create_dir_all(&outside).unwrap();

            fs::remove_dir_all(&expected_game).unwrap();
            let out = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&expected_game)
                .arg(&outside)
                .output()
                .unwrap();
            assert!(out.status.success(), "mklink /J failed: {out:?}");
            let err = resolve_profile_game_dir(&e.root, &iid).unwrap_err();
            assert_eq!(err.code, "storage");
            fs::remove_dir(&expected_game).unwrap();

            let inst_dir = empty.root.instances_dir();
            fs::remove_dir_all(&inst_dir).unwrap();
            let out = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&inst_dir)
                .arg(&outside)
                .output()
                .unwrap();
            assert!(out.status.success(), "mklink /J failed: {out:?}");
            let err = resolve_instances_dir(&empty.root).unwrap_err();
            assert_eq!(err.code, "storage");
            fs::remove_dir(&inst_dir).unwrap();
        }
    }

    #[tokio::test]
    async fn set_version_verified_checks_catalogs_respects_locks_and_resets_ready() {
        let e = env();
        let locks = Arc::new(ProfileLocks::default());
        let created = create_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &e.settings,
            NewInstance::vanilla("Survival", "1.20.1").unwrap(),
            100,
        )
        .await
        .unwrap();
        let id = created.profiles[0].id.clone();
        let iid = InstanceId::new(&id).unwrap();
        InstanceStore::new(&e.root)
            .mark_ready(
                &iid,
                crate::versions::VersionId::new("1.20.1").unwrap(),
                None,
            )
            .unwrap();

        // Unknown Minecraft version is refused before touching the profile.
        let bad = set_version_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &locks,
            Some(iid.clone()),
            VersionUpdate {
                id: &id,
                minecraft: "9.99.9",
                loader_version: None,
            },
            101,
        )
        .await
        .unwrap_err();
        assert_eq!(bad.code, "unknown_minecraft");
        assert!(InstanceStore::new(&e.root).get(&iid).unwrap().is_ready());

        // Running lock refuses the change with `already_running`.
        locks.claim(&id, Activity::Launching).unwrap();
        let locked = set_version_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &locks,
            Some(iid.clone()),
            VersionUpdate {
                id: &id,
                minecraft: "1.21.4",
                loader_version: None,
            },
            102,
        )
        .await
        .unwrap_err();
        assert_eq!(locked.code, "already_running");
        locks.release(&id);

        // Valid change updates Minecraft version and flips `ready` to false.
        let updated = set_version_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &locks,
            Some(iid.clone()),
            VersionUpdate {
                id: &id,
                minecraft: "1.21.4",
                loader_version: None,
            },
            103,
        )
        .await
        .unwrap();
        assert_eq!(updated.profiles[0].minecraft, "1.21.4");
        assert!(!updated.profiles[0].ready);

        // Modded profile verifies loader compatibility against official loader catalog.
        let modded = create_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &e.settings,
            NewInstance::modded(
                "Fabric World",
                "1.21.4",
                LoaderKind::Fabric,
                LoaderVersionSpec::Auto,
            )
            .unwrap(),
            110,
        )
        .await
        .unwrap();
        let mid = modded
            .profiles
            .iter()
            .find(|p| p.name == "Fabric World")
            .unwrap()
            .id
            .clone();
        // 1.20.1 has empty Fabric list in `env()` -> `no_compatible_loader`.
        let incompatible = set_version_verified(
            &e.global,
            &e.loaders,
            &e.root,
            &locks,
            None,
            VersionUpdate {
                id: &mid,
                minecraft: "1.20.1",
                loader_version: Some(LoaderVersionSpec::Auto),
            },
            111,
        )
        .await
        .unwrap_err();
        assert_eq!(incompatible.code, "no_compatible_loader");
    }

    #[tokio::test]
    async fn duplicate_profile_increments_copy_names_clones_files_and_selects_new_copy() {
        let e = env();
        let state = ProfilesState::new(e.root.clone(), Err("unused".into()), Err("unused".into()));
        let store = InstanceStore::new(&e.root);
        let src = store
            .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 200)
            .unwrap();
        fs::write(
            store.dirs(&src.id).game_dir().join("options.txt"),
            b"gamma:1.0",
        )
        .unwrap();

        let snap1 = duplicate_profile_inner(&state, &e.settings, src.id.as_str(), 201)
            .await
            .unwrap();
        assert_eq!(snap1.profiles.len(), 2);
        let copy1 = snap1
            .profiles
            .iter()
            .find(|p| p.name == "Survival (2)")
            .unwrap();
        assert_eq!(snap1.selected.as_deref(), Some(copy1.id.as_str()));
        assert_eq!(
            fs::read(
                store
                    .dirs(&InstanceId::new(&copy1.id).unwrap())
                    .game_dir()
                    .join("options.txt")
            )
            .unwrap(),
            b"gamma:1.0"
        );

        // Duplicating "Survival (2)" produces "Survival (3)", not "Survival (2) (2)".
        let snap2 = duplicate_profile_inner(&state, &e.settings, &copy1.id, 202)
            .await
            .unwrap();
        assert!(snap2.profiles.iter().any(|p| p.name == "Survival (3)"));

        // Long 64-char name truncates cleanly to fit " (2)".
        let long_name = "A".repeat(64);
        let long_inst = store
            .create(NewInstance::vanilla(&long_name, "1.21.4").unwrap(), 203)
            .unwrap();
        let snap3 = duplicate_profile_inner(&state, &e.settings, long_inst.id.as_str(), 204)
            .await
            .unwrap();
        let copy_long = snap3
            .profiles
            .iter()
            .find(|p| p.id != long_inst.id.as_str() && p.name.starts_with("AAAA"))
            .unwrap();
        assert!(copy_long.name.chars().count() <= 64);
        assert!(copy_long.name.ends_with(" (2)"));
    }

    #[tokio::test]
    async fn restore_profile_returns_profile_to_list_and_resolves_name_collisions() {
        let e = env();
        let state = ProfilesState::new(e.root.clone(), Err("unused".into()), Err("unused".into()));
        let store = InstanceStore::new(&e.root);
        let orig = store
            .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 100)
            .unwrap();
        let saved = store.dirs(&orig.id).game_dir().join("options.txt");
        fs::write(&saved, b"gamma:0.8").unwrap();
        let retired = store.retire(&orig.id).unwrap();
        let folder = retired.file_name().unwrap().to_str().unwrap().to_owned();

        let snap_before = snapshot(&e.root, None).unwrap();
        assert!(snap_before.profiles.is_empty());
        assert_eq!(snap_before.removed.len(), 1);
        assert_eq!(snap_before.removed[0].folder, folder);
        assert_eq!(snap_before.removed[0].name, "Survival");

        let restored_snap = restore_profile_inner(&state, &e.settings, &folder)
            .await
            .unwrap();
        assert_eq!(restored_snap.profiles.len(), 1);
        assert_eq!(restored_snap.selected.as_deref(), Some(orig.id.as_str()));
        assert!(restored_snap.removed.is_empty());
        assert_eq!(fs::read(&saved).unwrap(), b"gamma:0.8");

        // Remove again, create an active profile with the same name, then restore -> "Survival (2)".
        let retired2 = store.retire(&orig.id).unwrap();
        let folder2 = retired2.file_name().unwrap().to_str().unwrap().to_owned();
        store
            .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 200)
            .unwrap();

        let collision_snap = restore_profile_inner(&state, &e.settings, &folder2)
            .await
            .unwrap();
        assert_eq!(collision_snap.profiles.len(), 2);
        assert!(collision_snap.removed.is_empty());
        let restored2 = collision_snap
            .profiles
            .iter()
            .find(|p| p.name == "Survival (2)")
            .unwrap();
        assert_eq!(
            collision_snap.selected.as_deref(),
            Some(restored2.id.as_str())
        );

        // Missing or traversal folders fail with `not_found`.
        let err_missing = restore_profile_inner(&state, &e.settings, "does-not-exist")
            .await
            .unwrap_err();
        assert_eq!(err_missing.code, "not_found");
        let err_bad = restore_profile_inner(&state, &e.settings, "../survival")
            .await
            .unwrap_err();
        assert_eq!(err_bad.code, "not_found");
    }
}
