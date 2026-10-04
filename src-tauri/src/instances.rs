//! Instances: isolated Minecraft profiles. Every profile is explicitly Vanilla or Modded.
//!
//! Product rules this module enforces (see docs/TECHNICAL.md, "Profiles and isolation"):
//! - **Explicit choice.** Creating a profile requires an explicit [`Profile`]: `Vanilla` or
//!   `Modded { loader, .. }`. There is deliberately no default and no way to "upgrade" a
//!   vanilla instance into a modded one in place. The user always chooses the Minecraft version.
//! - **Vanilla stays clean.** A Vanilla profile has no loader, the launcher installs only that
//!   Minecraft version and the Mojang runtime/files it needs, and the launcher creates nothing
//!   mod-related in it (its `game/` directory starts empty).
//! - **Modded = Minecraft + exactly one loader from its official source.** The launcher prepares
//!   the loader, then the instance is ready for the user to add their own content by hand. The
//!   modded layout has the instance-local `mods/`, `config/`, `resourcepacks/`, `shaderpacks/`
//!   and `saves/` directories. Nothing in the launcher searches, lists, downloads, enables or
//!   updates mods, modpacks, shaders or resource packs.
//! - **Total isolation.** Everything a player or a mod can edit lives under
//!   `instances/<id>/game/`. All such paths come from [`InstanceDirs`], which is built from one
//!   instance's directory and nothing else, so there is no API that can express a shared or
//!   global mods/config folder. Instances are never symlinked or merged.
//! - **Not locked to a version.** Minecraft versions are opaque validated strings; none is
//!   hard-coded or special-cased.
//!
//! Shared, global data (`minecraft/versions`, `libraries/`, `assets/`, `runtimes/`) holds only
//! immutable, hash-verified files published by Mojang or by the loader project. Nothing a user or
//! mod can modify is ever stored there.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use crate::loaders::LoaderKind;
pub use crate::versions::{LoaderVersion, McVersion, VersionId};

use crate::portable::fsutil::{atomic_write, is_link};
use crate::portable::{PortableRoot, RelPath};
use crate::versions::InvalidToken;

const SCHEMA: u32 = 2;
const MANIFEST: &str = "instance.json";
const MAX_JVM_ARGS: usize = 64;
const MAX_JVM_ARG_LEN: usize = 512;

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error("invalid instance id {0:?}")]
    BadId(String),
    #[error("{0}")]
    Token(#[from] InvalidToken),
    #[error("invalid instance name: {0}")]
    BadName(&'static str),
    #[error("invalid JVM argument: {0}")]
    BadJvmArg(&'static str),
    #[error("instance '{0}' does not exist")]
    NotFound(String),
    #[error("instance manifest in '{dir}' claims id '{claimed}'")]
    IdMismatch { dir: String, claimed: String },
    #[error("inconsistent instance setup: {0}")]
    InconsistentSetup(&'static str),
    #[error("refusing to touch a link at {0}")]
    Link(String),
    #[error("manifest is from a newer launcher (schema {0})")]
    TooNew(u32),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("manifest is invalid: {0}")]
    Json(#[from] serde_json::Error),
}

/// Directory name of an instance: lowercase slug, a single path component.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn new(s: &str) -> Result<Self, InstanceError> {
        let ok = !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && !s.starts_with(['.', '-'])
            && s.chars().all(|c| !c.is_ascii_uppercase())
            && RelPath::new(s)
                .map(|r| r.components().count() == 1)
                .unwrap_or(false);
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(InstanceError::BadId(s.to_owned()))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl Serialize for InstanceId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for InstanceId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        InstanceId::new(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// Which loader version a Modded profile wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LoaderVersionSpec {
    /// Let the launcher pick the best compatible version from the loader's official catalog
    /// (recommended, then stable, then newest). The result is pinned in `SetupState::Ready`.
    Auto,
    /// The user chose one of the offered versions.
    Pinned { version: LoaderVersion },
}

/// The user's explicit "Vanilla or Modded?" answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Profile {
    /// Plain Minecraft from Mojang. No loader, nothing else.
    Vanilla,
    /// Minecraft plus exactly one loader (Fabric, Forge, NeoForge or Quilt) from its official source.
    Modded {
        loader: LoaderKind,
        loader_version: LoaderVersionSpec,
    },
}

/// Whether the game files for this profile are installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SetupState {
    /// Created, but Minecraft (and the loader) are not installed yet.
    Pending,
    /// Installed. Everything needed to launch is in the shared immutable stores, so the instance
    /// runs offline from here on.
    Ready {
        /// The launchable version profile under `minecraft/versions/` (the Minecraft version id for
        /// Vanilla, the loader's profile id for Modded).
        version_id: VersionId,
        /// The exact loader version installed (Modded only).
        loader_version: Option<LoaderVersion>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JavaChoice {
    /// Use the runtime Minecraft's own version metadata asks for (launcher-managed).
    Auto,
    /// A specific launcher-managed runtime from `runtimes/java/<id>`.
    Runtime { id: String },
}

fn default_java() -> JavaChoice {
    JavaChoice::Auto
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub schema: u32,
    pub id: InstanceId,
    pub name: String,
    /// Chosen by the user; any version Mojang publishes.
    pub minecraft: McVersion,
    pub profile: Profile,
    pub setup: SetupState,
    pub java: JavaChoice,
    pub memory_mb: Option<u32>,
    /// Untrusted user input; validated for shape here and shown to the user before launch.
    pub extra_jvm_args: Vec<String>,
    pub created_at: i64,
}

impl Instance {
    pub fn is_vanilla(&self) -> bool {
        matches!(self.profile, Profile::Vanilla)
    }
    pub fn loader(&self) -> Option<LoaderKind> {
        match self.profile {
            Profile::Vanilla => None,
            Profile::Modded { loader, .. } => Some(loader),
        }
    }
    pub fn is_ready(&self) -> bool {
        matches!(self.setup, SetupState::Ready { .. })
    }
}

/// Every instance-specific location, derived from exactly one instance directory.
#[derive(Debug, Clone)]
pub struct InstanceDirs {
    dir: PathBuf,
}

impl InstanceDirs {
    pub fn root(&self) -> &Path {
        &self.dir
    }
    pub fn manifest_file(&self) -> PathBuf {
        self.dir.join(MANIFEST)
    }
    /// Working directory and `--gameDir`. Minecraft and its mods write only inside here.
    pub fn game_dir(&self) -> PathBuf {
        self.dir.join("game")
    }
    /// Extracted native libraries; launcher-owned and disposable.
    pub fn natives_dir(&self) -> PathBuf {
        self.dir.join("natives")
    }
    /// Stands in for the user profile while the game runs (`user.home`, `APPDATA`...), so
    /// anything a game or mod writes there stays in this instance.
    pub fn home_dir(&self) -> PathBuf {
        self.dir.join("home")
    }
    pub fn playtime_file(&self) -> PathBuf {
        self.dir.join("playtime.json")
    }
    pub fn mods_dir(&self) -> PathBuf {
        self.game_dir().join("mods")
    }
    pub fn config_dir(&self) -> PathBuf {
        self.game_dir().join("config")
    }
    pub fn saves_dir(&self) -> PathBuf {
        self.game_dir().join("saves")
    }
    pub fn resourcepacks_dir(&self) -> PathBuf {
        self.game_dir().join("resourcepacks")
    }
    pub fn shaderpacks_dir(&self) -> PathBuf {
        self.game_dir().join("shaderpacks")
    }
}

/// A request to create a profile. `minecraft` and `profile` are required fields with no
/// defaults: whatever drives creation (the UI, later) must ask the user for both.
#[derive(Debug, Clone, Deserialize)]
pub struct NewInstance {
    pub name: String,
    pub minecraft: McVersion,
    pub profile: Profile,
    #[serde(default = "default_java")]
    pub java: JavaChoice,
    #[serde(default)]
    pub memory_mb: Option<u32>,
    #[serde(default)]
    pub extra_jvm_args: Vec<String>,
}

impl NewInstance {
    /// A clean vanilla profile of the given Minecraft version.
    pub fn vanilla(name: &str, minecraft: &str) -> Result<Self, InstanceError> {
        Ok(Self {
            name: name.to_owned(),
            minecraft: McVersion::new(minecraft)?,
            profile: Profile::Vanilla,
            java: JavaChoice::Auto,
            memory_mb: None,
            extra_jvm_args: Vec::new(),
        })
    }

    /// A modded profile: the given Minecraft version plus one loader.
    pub fn modded(
        name: &str,
        minecraft: &str,
        loader: LoaderKind,
        loader_version: LoaderVersionSpec,
    ) -> Result<Self, InstanceError> {
        Ok(Self {
            name: name.to_owned(),
            minecraft: McVersion::new(minecraft)?,
            profile: Profile::Modded {
                loader,
                loader_version,
            },
            java: JavaChoice::Auto,
            memory_mb: None,
            extra_jvm_args: Vec::new(),
        })
    }
}

#[derive(Debug, Default)]
pub struct Listing {
    pub instances: Vec<Instance>,
    /// Directories that could not be loaded, with the reason; never silently dropped.
    pub problems: Vec<(String, String)>,
}

pub struct InstanceStore<'a> {
    root: &'a PortableRoot,
}

fn validate_name(name: &str) -> Result<String, InstanceError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 64 {
        return Err(InstanceError::BadName("must be 1-64 characters"));
    }
    if n.chars().any(char::is_control) {
        return Err(InstanceError::BadName(
            "must not contain control characters",
        ));
    }
    Ok(n.to_owned())
}

fn validate_jvm_args(args: &[String]) -> Result<(), InstanceError> {
    if args.len() > MAX_JVM_ARGS {
        return Err(InstanceError::BadJvmArg("too many arguments"));
    }
    for a in args {
        if a.is_empty()
            || a.len() > MAX_JVM_ARG_LEN
            || a.chars().any(|c| c == '\0' || c == '\n' || c == '\r')
        {
            return Err(InstanceError::BadJvmArg(
                "empty, too long, or contains a line break",
            ));
        }
    }
    Ok(())
}

/// A profile and its setup state must agree: Vanilla never has a loader, Modded always does
/// once ready, and a pinned loader version is the one that was installed.
fn validate_consistency(inst: &Instance) -> Result<(), InstanceError> {
    match (&inst.profile, &inst.setup) {
        (_, SetupState::Pending) => Ok(()),
        (
            Profile::Vanilla,
            SetupState::Ready {
                version_id,
                loader_version,
            },
        ) => {
            if loader_version.is_some() {
                return Err(InstanceError::InconsistentSetup(
                    "a Vanilla instance cannot have a loader",
                ));
            }
            if version_id.as_str() != inst.minecraft.as_str() {
                return Err(InstanceError::InconsistentSetup(
                    "a Vanilla instance launches its own Minecraft version",
                ));
            }
            Ok(())
        }
        (
            Profile::Modded {
                loader_version: spec,
                ..
            },
            SetupState::Ready { loader_version, .. },
        ) => {
            let Some(installed) = loader_version else {
                return Err(InstanceError::InconsistentSetup(
                    "a ready Modded instance must record its loader version",
                ));
            };
            if let LoaderVersionSpec::Pinned { version } = spec {
                if version != installed {
                    return Err(InstanceError::InconsistentSetup(
                        "installed loader version differs from the pinned one",
                    ));
                }
            }
            Ok(())
        }
    }
}

/// `Fabric 1.21.4` -> `fabric-1-21-4`. Always yields a valid id.
pub fn slugify(name: &str) -> InstanceId {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_matches('-');
    let s: String = s.chars().take(48).collect();
    let s = s.trim_matches('-').to_owned();
    let base = if s.is_empty() {
        "instance".to_owned()
    } else {
        s
    };
    // Reserved Windows device names ("con", "nul", ...) fall back to a safe prefix.
    InstanceId::new(&base).unwrap_or_else(|_| InstanceId(format!("i-{base}")))
}

impl<'a> InstanceStore<'a> {
    pub fn new(root: &'a PortableRoot) -> Self {
        Self { root }
    }

    pub fn dirs(&self, id: &InstanceId) -> InstanceDirs {
        InstanceDirs {
            dir: self.root.instances_dir().join(id.as_str()),
        }
    }

    /// Creates the instance directory and manifest (`SetupState::Pending`). Installing Minecraft
    /// and the loader is the later pipeline's job; it ends with [`InstanceStore::mark_ready`].
    pub fn create(&self, new: NewInstance, now: i64) -> Result<Instance, InstanceError> {
        let name = validate_name(&new.name)?;
        validate_jvm_args(&new.extra_jvm_args)?;
        fs::create_dir_all(self.root.instances_dir())?;

        // `create_dir` fails if the directory exists, which makes id allocation race-free.
        let base = slugify(&name);
        let mut n = 1u32;
        let (id, dirs) = loop {
            let candidate = if n == 1 {
                base.clone()
            } else {
                InstanceId::new(&format!("{}-{n}", base.as_str()))?
            };
            let dirs = self.dirs(&candidate);
            match fs::create_dir(dirs.root()) {
                Ok(()) => break (candidate, dirs),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e.into()),
            }
        };

        let result = (|| {
            fs::create_dir(dirs.game_dir())?;
            match &new.profile {
                // Vanilla stays completely clean: `game/` starts empty; Minecraft itself creates
                // whatever it needs (saves, options, logs) when it runs.
                Profile::Vanilla => {}
                // Modded: the instance-local folders the user will put their own content in.
                Profile::Modded { .. } => {
                    for d in [
                        dirs.mods_dir(),
                        dirs.config_dir(),
                        dirs.resourcepacks_dir(),
                        dirs.shaderpacks_dir(),
                        dirs.saves_dir(),
                    ] {
                        fs::create_dir(d)?;
                    }
                }
            }
            let instance = Instance {
                schema: SCHEMA,
                id: id.clone(),
                name,
                minecraft: new.minecraft,
                profile: new.profile,
                setup: SetupState::Pending,
                java: new.java,
                memory_mb: new.memory_mb,
                extra_jvm_args: new.extra_jvm_args,
                created_at: now,
            };
            atomic_write(
                &dirs.manifest_file(),
                &serde_json::to_vec_pretty(&instance)?,
            )?;
            Ok::<_, InstanceError>(instance)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(dirs.root()); // never leave a half-made instance behind
        }
        result
    }

    /// Records that installation finished. For Vanilla `version_id` must be the Minecraft
    /// version and there is no loader version; for Modded the installed loader version is required.
    pub fn mark_ready(
        &self,
        id: &InstanceId,
        version_id: VersionId,
        loader_version: Option<LoaderVersion>,
    ) -> Result<Instance, InstanceError> {
        let mut inst = self.get(id)?;
        inst.setup = SetupState::Ready {
            version_id,
            loader_version,
        };
        validate_consistency(&inst)?;
        atomic_write(
            &self.dirs(id).manifest_file(),
            &serde_json::to_vec_pretty(&inst)?,
        )?;
        Ok(inst)
    }

    pub fn get(&self, id: &InstanceId) -> Result<Instance, InstanceError> {
        let dirs = self.dirs(id);
        let bytes = match fs::read(dirs.manifest_file()) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(InstanceError::NotFound(id.to_string()))
            }
            Err(e) => return Err(e.into()),
        };
        let inst: Instance = serde_json::from_slice(&bytes)?;
        if inst.schema > SCHEMA {
            return Err(InstanceError::TooNew(inst.schema));
        }
        // A manifest copied from another instance must not impersonate it.
        if inst.id != *id {
            return Err(InstanceError::IdMismatch {
                dir: id.to_string(),
                claimed: inst.id.to_string(),
            });
        }
        validate_jvm_args(&inst.extra_jvm_args)?;
        validate_consistency(&inst)?;
        Ok(inst)
    }

    pub fn list(&self) -> io::Result<Listing> {
        let mut out = Listing::default();
        let dir = self.root.instances_dir();
        if !dir.is_dir() {
            return Ok(out);
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            match InstanceId::new(&name).and_then(|id| self.get(&id)) {
                Ok(i) => out.instances.push(i),
                Err(e) => out.problems.push((name, e.to_string())),
            }
        }
        out.instances.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        Ok(out)
    }

    /// Changes only the display name. The id and directory never change, so no path stored
    /// anywhere (settings, playtime, shortcuts) can be invalidated by a rename.
    pub fn rename(&self, id: &InstanceId, new_name: &str) -> Result<Instance, InstanceError> {
        let mut inst = self.get(id)?;
        inst.name = validate_name(new_name)?;
        atomic_write(
            &self.dirs(id).manifest_file(),
            &serde_json::to_vec_pretty(&inst)?,
        )?;
        Ok(inst)
    }

    /// Deletes one instance and everything in it (saves included; the UI must confirm first).
    /// Other instances and all shared data are untouched. Callers must ensure it is not running.
    pub fn delete(&self, id: &InstanceId) -> Result<(), InstanceError> {
        self.get(id)?; // proves it is a real instance of ours, not an arbitrary folder
        let dirs = self.dirs(id);
        if is_link(dirs.root())? {
            return Err(InstanceError::Link(dirs.root().display().to_string()));
        }
        Ok(fs::remove_dir_all(dirs.root())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::root::DIRS;

    fn setup() -> (tempfile::TempDir, PortableRoot) {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path());
        root.ensure_layout().unwrap();
        (d, root)
    }

    fn fabric(name: &str) -> NewInstance {
        NewInstance::modded(name, "1.21.4", LoaderKind::Fabric, LoaderVersionSpec::Auto).unwrap()
    }

    fn lv(s: &str) -> LoaderVersion {
        LoaderVersion::new(s).unwrap()
    }
    fn vid(s: &str) -> VersionId {
        VersionId::new(s).unwrap()
    }

    #[test]
    fn the_vanilla_or_modded_choice_and_the_version_are_required() {
        // No default profile type: a request that does not say Vanilla or Modded is rejected.
        assert!(
            serde_json::from_str::<NewInstance>(r#"{"name":"x","minecraft":"1.21.4"}"#).is_err()
        );
        // No default Minecraft version either: the user always chooses one.
        assert!(serde_json::from_str::<NewInstance>(
            r#"{"name":"x","profile":{"type":"vanilla"}}"#
        )
        .is_err());
        let v: NewInstance = serde_json::from_str(
            r#"{"name":"x","minecraft":"1.21.4","profile":{"type":"vanilla"}}"#,
        )
        .unwrap();
        assert_eq!(v.profile, Profile::Vanilla);
        let m: NewInstance = serde_json::from_str(
            r#"{"name":"x","minecraft":"1.20.1","profile":{"type":"modded","loader":"neoforge","loader_version":{"mode":"auto"}}}"#,
        )
        .unwrap();
        assert!(matches!(
            m.profile,
            Profile::Modded {
                loader: LoaderKind::NeoForge,
                ..
            }
        ));
        // Hostile ids are rejected at the boundary.
        assert!(serde_json::from_str::<NewInstance>(
            r#"{"name":"x","minecraft":"../x","profile":{"type":"vanilla"}}"#
        )
        .is_err());
    }

    #[test]
    fn vanilla_instance_is_clean_and_has_no_loader_or_mod_folders() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let i = store
            .create(NewInstance::vanilla("Vanilla 1.21.4", "1.21.4").unwrap(), 1)
            .unwrap();
        assert!(i.is_vanilla() && i.loader().is_none() && !i.is_ready());
        let dirs = store.dirs(&i.id);
        assert!(dirs.game_dir().is_dir());
        for absent in [
            dirs.mods_dir(),
            dirs.config_dir(),
            dirs.resourcepacks_dir(),
            dirs.shaderpacks_dir(),
            dirs.saves_dir(),
        ] {
            assert!(
                !absent.exists(),
                "{} must not be created for vanilla",
                absent.display()
            );
        }
        assert_eq!(
            fs::read_dir(dirs.game_dir()).unwrap().count(),
            0,
            "the launcher must not pre-populate a vanilla game dir"
        );
    }

    #[test]
    fn modded_instance_gets_exactly_the_five_instance_local_folders() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let i = store.create(fabric("Fabric 1.21.4"), 1).unwrap();
        assert_eq!(i.loader(), Some(LoaderKind::Fabric));
        let dirs = store.dirs(&i.id);
        let mut found: Vec<String> = fs::read_dir(dirs.game_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(
            found,
            ["config", "mods", "resourcepacks", "saves", "shaderpacks"]
        );
        // They are empty: the launcher puts no content in them.
        for d in [
            dirs.mods_dir(),
            dirs.config_dir(),
            dirs.resourcepacks_dir(),
            dirs.shaderpacks_dir(),
            dirs.saves_dir(),
        ] {
            assert_eq!(fs::read_dir(d).unwrap().count(), 0);
        }
    }

    #[test]
    fn every_loader_can_be_chosen_with_auto_or_a_pinned_version() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        for kind in LoaderKind::ALL {
            let auto = NewInstance::modded("a", "1.21.4", kind, LoaderVersionSpec::Auto).unwrap();
            let pinned = NewInstance::modded(
                "b",
                "1.21.4",
                kind,
                LoaderVersionSpec::Pinned {
                    version: lv("1.2.3"),
                },
            )
            .unwrap();
            assert_eq!(store.create(auto, 1).unwrap().loader(), Some(kind));
            assert_eq!(store.create(pinned, 1).unwrap().loader(), Some(kind));
        }
        assert_eq!(store.list().unwrap().instances.len(), 8);
    }

    #[test]
    fn setup_is_pending_until_the_pipeline_marks_it_ready_and_ready_survives_reload() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let v = store
            .create(NewInstance::vanilla("V", "1.21.4").unwrap(), 1)
            .unwrap();
        let m = store.create(fabric("M"), 2).unwrap();
        assert!(!v.is_ready() && !m.is_ready());

        store.mark_ready(&v.id, vid("1.21.4"), None).unwrap();
        store
            .mark_ready(
                &m.id,
                vid("fabric-loader-0.19.5-1.21.4"),
                Some(lv("0.19.5")),
            )
            .unwrap();
        let (v2, m2) = (store.get(&v.id).unwrap(), store.get(&m.id).unwrap());
        assert!(v2.is_ready() && m2.is_ready());
        assert_eq!(
            m2.setup,
            SetupState::Ready {
                version_id: vid("fabric-loader-0.19.5-1.21.4"),
                loader_version: Some(lv("0.19.5"))
            }
        );
    }

    #[test]
    fn vanilla_can_never_be_given_a_loader_and_modded_must_record_one() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let v = store
            .create(NewInstance::vanilla("V", "1.21.4").unwrap(), 1)
            .unwrap();
        // Vanilla with a loader recorded: refused (keeps vanilla clean).
        assert!(matches!(
            store.mark_ready(&v.id, vid("1.21.4"), Some(lv("0.19.5"))),
            Err(InstanceError::InconsistentSetup(_))
        ));
        // Vanilla must launch its own Minecraft version, not some loader profile.
        assert!(matches!(
            store.mark_ready(&v.id, vid("fabric-loader-0.19.5-1.21.4"), None),
            Err(InstanceError::InconsistentSetup(_))
        ));
        assert!(
            !store.get(&v.id).unwrap().is_ready(),
            "a refused update changes nothing on disk"
        );

        let m = store.create(fabric("M"), 2).unwrap();
        assert!(matches!(
            store.mark_ready(&m.id, vid("fabric-loader-0.19.5-1.21.4"), None),
            Err(InstanceError::InconsistentSetup(_))
        ));

        // A pinned version must be the one installed.
        let pinned = store
            .create(
                NewInstance::modded(
                    "P",
                    "1.21.4",
                    LoaderKind::Forge,
                    LoaderVersionSpec::Pinned {
                        version: lv("54.1.14"),
                    },
                )
                .unwrap(),
                3,
            )
            .unwrap();
        assert!(matches!(
            store.mark_ready(&pinned.id, vid("1.21.4-forge-54.1.18"), Some(lv("54.1.18"))),
            Err(InstanceError::InconsistentSetup(_))
        ));
        assert!(store
            .mark_ready(&pinned.id, vid("1.21.4-forge-54.1.14"), Some(lv("54.1.14")))
            .is_ok());
    }

    #[test]
    fn a_hand_edited_manifest_that_breaks_the_rules_is_reported_not_trusted() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let v = store
            .create(NewInstance::vanilla("V", "1.21.4").unwrap(), 1)
            .unwrap();
        let path = store.dirs(&v.id).manifest_file();
        let mut j: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        j["setup"] =
            serde_json::json!({"state":"ready","version_id":"1.21.4","loader_version":"0.19.5"});
        fs::write(&path, serde_json::to_vec(&j).unwrap()).unwrap();
        assert!(matches!(
            store.get(&v.id),
            Err(InstanceError::InconsistentSetup(_))
        ));
        assert_eq!(store.list().unwrap().problems.len(), 1);
    }

    #[test]
    fn instances_are_isolated_from_each_other() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let vanilla = store
            .create(NewInstance::vanilla("Vanilla", "1.21.4").unwrap(), 1)
            .unwrap();
        let modded = store.create(fabric("Fabric"), 2).unwrap();
        let (vd, md) = (store.dirs(&vanilla.id), store.dirs(&modded.id));

        assert!(!vd.root().starts_with(md.root()) && !md.root().starts_with(vd.root()));
        assert_ne!(vd.game_dir(), md.game_dir());

        // Content a user puts in the modded instance never appears in the vanilla one.
        fs::write(md.mods_dir().join("some-mod.jar"), b"jar").unwrap();
        fs::write(md.config_dir().join("mod.toml"), b"x=1").unwrap();
        fs::write(md.shaderpacks_dir().join("pack.zip"), b"z").unwrap();
        fs::write(md.resourcepacks_dir().join("pack.zip"), b"z").unwrap();
        assert!(
            !vd.mods_dir().exists()
                && !vd.config_dir().exists()
                && !vd.shaderpacks_dir().exists()
                && !vd.resourcepacks_dir().exists()
        );
        assert_eq!(fs::read_dir(vd.game_dir()).unwrap().count(), 0);

        // Deleting one instance leaves the other intact.
        fs::write(vd.game_dir().join("options.txt"), b"fov:70").unwrap();
        store.delete(&modded.id).unwrap();
        assert!(!md.root().exists());
        assert_eq!(
            fs::read(vd.game_dir().join("options.txt")).unwrap(),
            b"fov:70"
        );
    }

    #[test]
    fn two_modded_instances_with_the_same_loader_and_version_still_share_nothing() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let a = store.create(fabric("Same"), 1).unwrap();
        let b = store.create(fabric("Same"), 2).unwrap();
        let (ad, bd) = (store.dirs(&a.id), store.dirs(&b.id));
        fs::write(ad.mods_dir().join("only-in-a.jar"), b"jar").unwrap();
        assert_eq!(fs::read_dir(bd.mods_dir()).unwrap().count(), 0);
        assert_ne!(ad.mods_dir(), bd.mods_dir());
    }

    #[test]
    fn the_shared_layout_contains_no_global_mod_or_config_folders() {
        for d in DIRS {
            let last = d.rsplit('/').next().unwrap();
            for forbidden in [
                "mods",
                "config",
                "saves",
                "resourcepacks",
                "shaderpacks",
                "screenshots",
            ] {
                assert_ne!(
                    last, forbidden,
                    "global '{d}' would break instance isolation"
                );
            }
        }
    }

    #[test]
    fn many_minecraft_versions_are_accepted_and_none_is_special() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        for v in [
            "1.21.4",
            "1.20.1",
            "1.12.2",
            "1.7.10",
            "24w10a",
            "1.21.5-pre1",
            "b1.7.3",
            "rd-132211",
            "1.14_combat-0",
            "25w02a",
        ] {
            store
                .create(NewInstance::vanilla(&format!("v {v}"), v).unwrap(), 1)
                .unwrap();
        }
        assert_eq!(store.list().unwrap().instances.len(), 10);
        for bad in [
            "",
            "../1.21",
            "1.21/4",
            "C:1.21",
            "con",
            "1  21",
            "-rf",
            &"9".repeat(80),
        ] {
            assert!(
                NewInstance::vanilla("x", bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn ids_are_unique_safe_and_stable_under_rename() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let a = store
            .create(NewInstance::vanilla("My World", "1.21.4").unwrap(), 1)
            .unwrap();
        let b = store
            .create(NewInstance::vanilla("My World", "1.21.4").unwrap(), 2)
            .unwrap();
        assert_eq!(a.id.as_str(), "my-world");
        assert_eq!(b.id.as_str(), "my-world-2");
        let renamed = store.rename(&a.id, "Renamed").unwrap();
        assert_eq!(renamed.id, a.id);
        assert!(store.dirs(&a.id).root().is_dir());
        for weird in ["CON", "../../x", "Ünïcode ✨", "a/b\\c"] {
            // Whatever the display name, the directory id must be a safe single component.
            let i = store
                .create(NewInstance::vanilla(weird, "1.21.4").unwrap(), 3)
                .unwrap();
            assert!(
                InstanceId::new(i.id.as_str()).is_ok(),
                "{weird:?} -> {}",
                i.id
            );
            assert_eq!(
                store.dirs(&i.id).root().parent().unwrap(),
                root.instances_dir()
            );
        }
        for bad in ["../x", "a/b", "A", ".hidden", "-rf", "con", "", "a b"] {
            assert!(InstanceId::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn manifest_must_match_its_directory() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let a = store
            .create(NewInstance::vanilla("A", "1.21.4").unwrap(), 1)
            .unwrap();
        let b = store
            .create(NewInstance::vanilla("B", "1.21.4").unwrap(), 2)
            .unwrap();
        fs::copy(
            store.dirs(&a.id).manifest_file(),
            store.dirs(&b.id).manifest_file(),
        )
        .unwrap();
        assert!(matches!(
            store.get(&b.id),
            Err(InstanceError::IdMismatch { .. })
        ));
        let listing = store.list().unwrap();
        assert_eq!(listing.instances.len(), 1);
        assert_eq!(
            listing.problems.len(),
            1,
            "a broken instance is reported, not hidden"
        );
    }

    #[test]
    fn untrusted_fields_are_validated_and_failures_leave_nothing_behind() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let mut bad = fabric("x");
        bad.extra_jvm_args = vec!["-Dok=1\n-Devil=1".into()];
        assert!(matches!(
            store.create(bad, 1),
            Err(InstanceError::BadJvmArg(_))
        ));
        assert!(matches!(
            store.create(NewInstance::vanilla("   ", "1.21.4").unwrap(), 1),
            Err(InstanceError::BadName(_))
        ));
        assert!(
            NewInstance::modded("x", "1.21.4", LoaderKind::Forge, LoaderVersionSpec::Auto).is_ok()
        );
        assert!(LoaderVersion::new("../x").is_err());
        assert_eq!(store.list().unwrap().instances.len(), 0);
        assert_eq!(fs::read_dir(root.instances_dir()).unwrap().count(), 0);
    }

    #[test]
    fn delete_refuses_non_instances() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        fs::create_dir(root.instances_dir().join("not-ours")).unwrap();
        fs::write(
            root.instances_dir().join("not-ours").join("precious.txt"),
            b"x",
        )
        .unwrap();
        let id = InstanceId::new("not-ours").unwrap();
        assert!(store.delete(&id).is_err());
        assert!(root
            .instances_dir()
            .join("not-ours")
            .join("precious.txt")
            .exists());
    }
}
