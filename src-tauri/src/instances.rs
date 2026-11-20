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

use crate::portable::fsutil::{
    atomic_write, is_link, move_new_no_replace, read_bounded, reject_links_between,
    remove_dir_all_with_retry,
};
use crate::portable::{PortableRoot, RelPath};
use crate::versions::InvalidToken;

const SCHEMA: u32 = 2;
const MANIFEST: &str = "instance.json";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_JVM_ARGS: usize = 64;
const MAX_JVM_ARG_LEN: usize = 512;
pub const MIN_RESOLUTION_WIDTH: u32 = 320;
pub const MAX_RESOLUTION_WIDTH: u32 = 7680;
pub const MIN_RESOLUTION_HEIGHT: u32 = 240;
pub const MAX_RESOLUTION_HEIGHT: u32 = 4320;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

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
    #[error("invalid resolution: {0}")]
    BadResolution(&'static str),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
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
    pub resolution: Option<Resolution>,
    #[serde(skip)]
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
            resolution: None,
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
            resolution: None,
            extra_jvm_args: Vec::new(),
        })
    }
}

/// Folder inside `instances/` that keeps profiles removed from Clade, whole and untouched. It is
/// not a valid instance id (ids cannot start with a dot), so it can never be an instance, and
/// [`InstanceStore::list`] skips it.
pub const RETAINED_DIR: &str = ".removed";

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

const FORBIDDEN_JVM_PREFIXES: &[&str] = &[
    "-javaagent",
    "-agentlib",
    "-agentpath",
    "-xrun",
    "-jar",
    "-cp",
    "-classpath",
    "--class-path",
    "-p",
    "--module-path",
    "--upgrade-module-path",
    "--patch-module",
    "-xbootclasspath",
    "-xx:onerror",
    "-xx:onoutofmemoryerror",
    "-xx:errorfile",
    "-xx:heapdumppath",
    "-xx:logfile",
    "-xx:sharedarchivefile",
    "-xx:archiveclassesatexit",
    "-duser.home",
    "-duser.dir",
    "-djava.io.tmpdir",
    "-djava.library.path",
    "-djava.class.path",
    "-djdk.module.path",
    "-dlog4j.configurationfile",
    "-dlog4j2.formatmsgnolookups",
    "-xmx",
    "-xms",
    "-xx:maxram",
    "-xx:initialram",
];

const FORBIDDEN_XX_OPTS: &[&str] = &[
    "onerror",
    "onoutofmemoryerror",
    "errorfile",
    "heapdumppath",
    "logfile",
    "sharedarchivefile",
    "archiveclassesatexit",
];

fn is_allowed_jvm_arg(a: &str) -> bool {
    if let Some(rest) = a
        .strip_prefix("-XX:+")
        .or_else(|| a.strip_prefix("-XX:-"))
        .or_else(|| a.strip_prefix("-XX:"))
    {
        let (opt, val) = match rest.split_once('=') {
            Some((k, v)) => (k, Some(v)),
            None => (rest, None),
        };
        if opt.is_empty()
            || !opt.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || val.is_some_and(str::is_empty)
        {
            return false;
        }
        let opt_lower = opt.to_ascii_lowercase();
        return !FORBIDDEN_XX_OPTS.contains(&opt_lower.as_str())
            && !opt_lower.starts_with("maxram")
            && !opt_lower.starts_with("initialram");
    }
    if let Some(rest) = a.strip_prefix("-Xss").or_else(|| a.strip_prefix("-Xmn")) {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric());
    }
    if let Some(rest) = a.strip_prefix("-D") {
        let key = rest.split_once('=').map_or(rest, |(k, _)| k);
        return !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    }
    if matches!(a, "-ea" | "-da" | "-esa" | "-dsa") {
        return true;
    }
    if let Some(rest) = a
        .strip_prefix("--add-opens=")
        .or_else(|| a.strip_prefix("--add-exports="))
        .or_else(|| a.strip_prefix("--enable-native-access="))
    {
        return !rest.is_empty();
    }
    false
}

fn validate_jvm_args(args: &[String]) -> Result<(), InstanceError> {
    if args.len() > MAX_JVM_ARGS {
        return Err(InstanceError::BadJvmArg("too many arguments"));
    }
    for a in args {
        if a.is_empty()
            || a.len() > MAX_JVM_ARG_LEN
            || a.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(InstanceError::BadJvmArg(
                "empty, too long, or contains whitespace or control characters",
            ));
        }
        if !a.starts_with('-') {
            return Err(InstanceError::BadJvmArg("must start with '-'"));
        }
        let lower = a.to_ascii_lowercase();
        if FORBIDDEN_JVM_PREFIXES.iter().any(|p| lower.starts_with(p)) || !is_allowed_jvm_arg(a) {
            return Err(InstanceError::BadJvmArg(
                "flag is not in the safe JVM allowlist",
            ));
        }
    }
    Ok(())
}

fn validate_resolution(resolution: Option<Resolution>) -> Result<(), InstanceError> {
    if let Some(r) = resolution {
        if !(MIN_RESOLUTION_WIDTH..=MAX_RESOLUTION_WIDTH).contains(&r.width)
            || !(MIN_RESOLUTION_HEIGHT..=MAX_RESOLUTION_HEIGHT).contains(&r.height)
        {
            return Err(InstanceError::BadResolution(
                "dimensions outside the allowed window range",
            ));
        }
    }
    Ok(())
}

fn map_link_err(e: io::Error) -> InstanceError {
    if e.to_string().contains("link") {
        InstanceError::Link(e.to_string())
    } else {
        InstanceError::Io(e)
    }
}

/// Recursively copies regular files and directories from `src` into `dst`, refusing to follow any
/// symbolic link or NTFS junction so a profile duplication never escapes its sandbox.
fn copy_dir_no_links(src: &Path, dst: &Path) -> Result<(), InstanceError> {
    if is_link(src)? {
        return Err(InstanceError::Link(src.display().to_string()));
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if is_link(&path)? {
            return Err(InstanceError::Link(path.display().to_string()));
        }
        let target = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir_no_links(&path, &target)?;
        } else if ft.is_file() {
            fs::copy(&path, &target)?;
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
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if self.get(&candidate).is_err() {
                        return Err(InstanceError::Io(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "profile folder '{candidate}' already exists and could not be read"
                            ),
                        )));
                    }
                    n += 1;
                }
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
                resolution: new.resolution,
                extra_jvm_args: new.extra_jvm_args,
                created_at: now,
            };
            validate_resolution(instance.resolution)?;
            atomic_write(
                &dirs.manifest_file(),
                &serde_json::to_vec_pretty(&instance)?,
            )?;
            Ok::<_, InstanceError>(instance)
        })();
        if result.is_err() {
            let _ = remove_dir_all_with_retry(dirs.root()); // never leave a half-made instance behind
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
        reject_links_between(self.root.path(), &dirs.manifest_file()).map_err(map_link_err)?;
        let bytes = match read_bounded(&dirs.manifest_file(), MAX_MANIFEST_BYTES) {
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
        validate_resolution(inst.resolution)?;
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
            if name == RETAINED_DIR {
                continue; // removed profiles kept for the user: never an active profile, never a "problem"
            }
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

    /// Sets or clears (`None` = follow global/automatic policy) the profile's own memory override
    /// in MiB, persisted atomically to `instance.json`.
    pub fn set_memory(
        &self,
        id: &InstanceId,
        memory_mb: Option<u32>,
    ) -> Result<Instance, InstanceError> {
        let mut inst = self.get(id)?;
        inst.memory_mb = memory_mb;
        atomic_write(
            &self.dirs(id).manifest_file(),
            &serde_json::to_vec_pretty(&inst)?,
        )?;
        Ok(inst)
    }

    /// Sets or clears (`None` = game default) the profile's initial game window resolution.
    pub fn set_resolution(
        &self,
        id: &InstanceId,
        resolution: Option<Resolution>,
    ) -> Result<Instance, InstanceError> {
        validate_resolution(resolution)?;
        let mut inst = self.get(id)?;
        inst.resolution = resolution;
        atomic_write(
            &self.dirs(id).manifest_file(),
            &serde_json::to_vec_pretty(&inst)?,
        )?;
        Ok(inst)
    }

    /// Updates the profile's validated extra JVM flags (`vec![]` clears them).
    pub fn set_jvm_args(
        &self,
        id: &InstanceId,
        args: Vec<String>,
    ) -> Result<Instance, InstanceError> {
        validate_jvm_args(&args)?;
        let mut inst = self.get(id)?;
        inst.extra_jvm_args = args;
        atomic_write(
            &self.dirs(id).manifest_file(),
            &serde_json::to_vec_pretty(&inst)?,
        )?;
        Ok(inst)
    }

    /// Updates a profile's Minecraft version (and, for Modded profiles, its loader version spec)
    /// while keeping the profile's type (`Vanilla` vs `Modded` and its `LoaderKind`) and all user
    /// files inside `game/` untouched. When the effective version changes, `setup` resets to
    /// [`SetupState::Pending`] and disposable `natives/` are removed so the next setup prepares
    /// the new version cleanly.
    pub fn set_version(
        &self,
        id: &InstanceId,
        minecraft: McVersion,
        loader_version: Option<LoaderVersionSpec>,
    ) -> Result<Instance, InstanceError> {
        let mut inst = self.get(id)?;
        let dirs = self.dirs(id);
        match inst.profile.clone() {
            Profile::Vanilla => {
                if loader_version.is_some() {
                    return Err(InstanceError::InconsistentSetup(
                        "a Vanilla instance cannot have a loader",
                    ));
                }
                if inst.minecraft == minecraft {
                    return Ok(inst);
                }
                inst.minecraft = minecraft;
                inst.setup = SetupState::Pending;
            }
            Profile::Modded {
                loader,
                loader_version: current_spec,
            } => {
                let next_spec = loader_version.unwrap_or(LoaderVersionSpec::Auto);
                let unchanged = inst.minecraft == minecraft
                    && match (&current_spec, &next_spec, &inst.setup) {
                        (
                            _,
                            LoaderVersionSpec::Pinned { version },
                            SetupState::Ready {
                                loader_version: Some(installed),
                                ..
                            },
                        ) => version == installed,
                        (a, b, _) => a == b,
                    };
                if unchanged {
                    return Ok(inst);
                }
                inst.minecraft = minecraft;
                inst.profile = Profile::Modded {
                    loader,
                    loader_version: next_spec,
                };
                inst.setup = SetupState::Pending;
            }
        }
        validate_consistency(&inst)?;
        let _ = remove_dir_all_with_retry(&dirs.natives_dir());
        atomic_write(&dirs.manifest_file(), &serde_json::to_vec_pretty(&inst)?)?;
        Ok(inst)
    }

    /// Creates a new isolated profile cloned from `source_id` under `new_name`: copies `game/`
    /// (worlds, options, mods, configs, resource packs, shaders) while rejecting any link, keeps
    /// `setup` (`Ready` stays `Ready` because game binaries, libraries, assets and Java live in
    /// shared immutable stores), starts playtime fresh at zero, and cleans up completely on error.
    pub fn duplicate(
        &self,
        source_id: &InstanceId,
        new_name: &str,
        now: i64,
    ) -> Result<Instance, InstanceError> {
        let name = validate_name(new_name)?;
        let source_dirs = self.dirs(source_id);
        reject_links_between(self.root.path(), source_dirs.root()).map_err(map_link_err)?;
        if is_link(source_dirs.root())? {
            return Err(InstanceError::Link(
                source_dirs.root().display().to_string(),
            ));
        }
        let source = self.get(source_id)?;
        fs::create_dir_all(self.root.instances_dir())?;

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
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if self.get(&candidate).is_err() {
                        return Err(InstanceError::Io(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "profile folder '{candidate}' already exists and could not be read"
                            ),
                        )));
                    }
                    n += 1;
                }
                Err(e) => return Err(e.into()),
            }
        };

        let result = (|| {
            let src_game = source_dirs.game_dir();
            if src_game.exists() {
                copy_dir_no_links(&src_game, &dirs.game_dir())?;
            } else {
                fs::create_dir_all(dirs.game_dir())?;
            }
            let instance = Instance {
                schema: SCHEMA,
                id,
                name,
                minecraft: source.minecraft,
                profile: source.profile,
                setup: source.setup,
                java: source.java,
                memory_mb: source.memory_mb,
                resolution: source.resolution,
                extra_jvm_args: source.extra_jvm_args,
                created_at: now,
            };
            validate_consistency(&instance)?;
            atomic_write(
                &dirs.manifest_file(),
                &serde_json::to_vec_pretty(&instance)?,
            )?;
            Ok::<_, InstanceError>(instance)
        })();
        if result.is_err() {
            let _ = remove_dir_all_with_retry(dirs.root());
        }
        result
    }

    /// Takes one profile out of Clade without deleting any user content: its whole folder is
    /// moved atomically on the same volume to `instances/.removed/<id>-<time>-<random>/`, where
    /// nothing lists or launches it and every user file stays as it was (worlds, mods, config,
    /// screenshots, logs, `home/`, `playtime.json`, `instance.json`, unknown files); only
    /// disposable launcher-extracted `natives/` DLLs are cleaned up afterwards. Returns the new
    /// location.
    ///
    /// If the folder is not a real, readable Clade instance, is a link, or cannot be renamed (for
    /// example a file inside is open), the profile stays exactly where it was and an error is
    /// returned; there is deliberately no copy-and-delete fallback. Callers must ensure the profile
    /// is not running or being prepared.
    pub fn retire(&self, id: &InstanceId) -> Result<PathBuf, InstanceError> {
        let dirs = self.dirs(id);
        reject_links_between(self.root.path(), dirs.root()).map_err(map_link_err)?;
        self.get(id)?; // proves it is a real instance of ours, not an arbitrary folder
        if is_link(dirs.root())? {
            return Err(InstanceError::Link(dirs.root().display().to_string()));
        }
        let retained = self.root.instances_dir().join(RETAINED_DIR);
        fs::create_dir_all(&retained)?;
        reject_links_between(self.root.path(), &retained).map_err(map_link_err)?;
        if is_link(&retained)? || !retained.is_dir() {
            return Err(InstanceError::Link(retained.display().to_string()));
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for _ in 0..8 {
            let mut nonce = [0u8; 4];
            getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
            let dest = retained.join(format!("{}-{stamp}-{}", id.as_str(), hex::encode(nonce)));
            match move_new_no_replace(dirs.root(), &dest) {
                Ok(()) => {
                    let _ = remove_dir_all_with_retry(&dest.join("natives"));
                    return Ok(dest);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(io::Error::other("no free name in the retained folder").into())
    }

    /// Deletes one instance and everything in it (saves included). Not part of the product: no
    /// command or screen may erase a profile's files, so this exists only for tests; removing a
    /// profile from the launcher is [`Self::retire`], which keeps every file.
    #[cfg(test)]
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
    fn new_instance_deserialization_skips_extra_jvm_args() {
        let parsed: NewInstance = serde_json::from_str(
            r#"{
                "name": "Clean",
                "minecraft": "1.21.4",
                "profile": { "type": "vanilla" },
                "extra_jvm_args": ["-javaagent:evil.jar", "-Xmx99G", "-Dmy.flag=1"]
            }"#,
        )
        .unwrap();
        assert!(
            parsed.extra_jvm_args.is_empty(),
            "frontend IPC JSON must never inject extra_jvm_args"
        );
    }

    #[test]
    fn jvm_arg_allowlist_accepts_safe_flags_and_rejects_every_dangerous_flag() {
        let allowed = vec![
            "-XX:+UseG1GC".to_owned(),
            "-XX:-DisableExplicitGC".to_owned(),
            "-XX:MaxGCPauseMillis=50".to_owned(),
            "-XX:G1NewSizePercent=20".to_owned(),
            "-Xss1M".to_owned(),
            "-Xmn512M".to_owned(),
            "-Dfml.readTimeout=90".to_owned(),
            "-Dmy_prop-1.ok=true".to_owned(),
            "-ea".to_owned(),
            "-da".to_owned(),
            "-esa".to_owned(),
            "-dsa".to_owned(),
            "--add-opens=java.base/java.lang=ALL-UNNAMED".to_owned(),
            "--add-exports=java.base/sun.security.util=ALL-UNNAMED".to_owned(),
            "--enable-native-access=ALL-UNNAMED".to_owned(),
        ];
        assert!(validate_jvm_args(&allowed).is_ok());

        for forbidden in [
            "",
            "MainClass",
            "@argfile.txt",
            "-Dfoo=bar baz",
            "-Dfoo=1\t2",
            "-Dfoo=1\0",
            "-Dfoo=1\n-Dbar=2",
            "-JavaAgent:agent.jar",
            "-agentlib:jdwp=transport=dt_socket",
            "-agentpath:evil.dll",
            "-Xrunjdwp:transport=dt_socket",
            "-jar",
            "-cp",
            "-classpath",
            "--class-path=evil.jar",
            "-p",
            "--module-path=mods",
            "--upgrade-module-path=mods",
            "--patch-module=java.base=evil.jar",
            "-Xbootclasspath/a:evil.jar",
            "-XX:OnError=calc",
            "-XX:+OnError",
            "-XX:OnOutOfMemoryError=calc",
            "-XX:ErrorFile=hs_err.log",
            "-XX:HeapDumpPath=dump.hprof",
            "-XX:LogFile=jvm.log",
            "-XX:SharedArchiveFile=app.jsa",
            "-XX:ArchiveClassesAtExit=app.jsa",
            "-Duser.home=elsewhere",
            "-DUSER.DIR=elsewhere",
            "-Djava.io.tmpdir=elsewhere",
            "-Djava.library.path=elsewhere",
            "-Djava.class.path=elsewhere",
            "-Djdk.module.path=elsewhere",
            "-Dlog4j.configurationFile=http://evil/x.xml",
            "-Dlog4j2.formatMsgNoLookups=false",
            "-Xmx8G",
            "-Xms2G",
            "-XX:MaxRAM=16G",
            "-XX:MaxRAMPercentage=90",
            "-XX:InitialRAMPercentage=50",
            "-XX:Bad-Flag=1",
            "-XX:EmptyVal=",
            "-Xss",
            "-D=1",
            "-Dbad/key=1",
            "--add-opens=",
            "--unknown-flag=1",
        ] {
            assert!(
                matches!(
                    validate_jvm_args(&[forbidden.to_owned()]),
                    Err(InstanceError::BadJvmArg(_))
                ),
                "{forbidden:?} should be rejected"
            );
        }

        let too_long = format!("-Dlong={}", "a".repeat(MAX_JVM_ARG_LEN));
        assert!(matches!(
            validate_jvm_args(&[too_long]),
            Err(InstanceError::BadJvmArg(_))
        ));
        let too_many: Vec<String> = (0..=MAX_JVM_ARGS).map(|i| format!("-Darg{i}=1")).collect();
        assert!(matches!(
            validate_jvm_args(&too_many),
            Err(InstanceError::BadJvmArg(_))
        ));
    }

    #[test]
    fn get_rejects_oversized_manifests_and_links() {
        let (d, root) = setup();
        let store = InstanceStore::new(&root);
        let inst = store
            .create(NewInstance::vanilla("Big", "1.21.4").unwrap(), 1)
            .unwrap();
        let manifest = store.dirs(&inst.id).manifest_file();
        fs::write(&manifest, vec![b' '; (MAX_MANIFEST_BYTES + 1) as usize]).unwrap();
        assert!(matches!(store.get(&inst.id), Err(InstanceError::Io(_))));
        assert_eq!(store.list().unwrap().problems.len(), 1);

        {
            let target = d.path().join("outside-target");
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("instance.json"), b"{}").unwrap();
            let linked = root.instances_dir().join("linked");
            let out = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&linked)
                .arg(&target)
                .output()
                .unwrap();
            assert!(out.status.success(), "mklink /J failed: {out:?}");
            let linked_id = InstanceId::new("linked").unwrap();
            assert!(matches!(store.get(&linked_id), Err(InstanceError::Link(_))));
            fs::remove_dir(&linked).unwrap();
        }
        let _ = &d;
    }

    #[test]
    fn create_refuses_to_allocate_dash_2_when_slug_collides_with_damaged_folder() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let damaged = root.instances_dir().join("survival");
        fs::create_dir_all(damaged.join("game/saves/World")).unwrap();
        fs::write(damaged.join("game/saves/World/level.dat"), b"precious").unwrap();
        fs::write(damaged.join("instance.json"), b"{corrupt json").unwrap();

        let err = store
            .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 1)
            .unwrap_err();
        assert!(
            matches!(err, InstanceError::Io(ref e) if e.kind() == io::ErrorKind::AlreadyExists)
        );
        assert!(!root.instances_dir().join("survival-2").exists());
        assert_eq!(
            fs::read(damaged.join("game/saves/World/level.dat")).unwrap(),
            b"precious"
        );
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

    #[test]
    fn set_memory_sets_and_clears_override_atomically() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let created = store.create(fabric("Modded Profile"), 42).unwrap();
        let initial = store
            .mark_ready(
                &created.id,
                vid("fabric-loader-0.19.5-1.21.4"),
                Some(lv("0.19.5")),
            )
            .unwrap();
        assert_eq!(initial.memory_mb, None);

        let updated = store.set_memory(&initial.id, Some(4096)).unwrap();
        assert_eq!(updated.memory_mb, Some(4096));

        let reloaded = store.get(&initial.id).unwrap();
        assert_eq!(reloaded, updated);
        assert_eq!(reloaded.memory_mb, Some(4096));
        assert_eq!(reloaded.name, initial.name);
        assert_eq!(reloaded.minecraft, initial.minecraft);
        assert_eq!(reloaded.profile, initial.profile);
        assert_eq!(reloaded.setup, initial.setup);

        let mut entries: Vec<String> = fs::read_dir(store.dirs(&initial.id).root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert!(!entries.iter().any(|name| name.contains(".tmp")));
        assert_eq!(entries, ["game", "instance.json"]);

        let cleared = store.set_memory(&initial.id, None).unwrap();
        assert_eq!(cleared.memory_mb, None);
        let reloaded_cleared = store.get(&initial.id).unwrap();
        assert_eq!(reloaded_cleared.memory_mb, None);
        assert_eq!(reloaded_cleared, initial);
    }

    #[test]
    fn set_memory_returns_not_found_for_missing_instance() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let missing = InstanceId::new("missing").unwrap();

        let err = store.set_memory(&missing, Some(4096)).unwrap_err();
        assert!(matches!(err, InstanceError::NotFound(ref id) if id == "missing"));
        assert!(!root.instances_dir().join("missing").exists());
    }

    #[test]
    fn set_version_updates_in_place_preserves_game_files_resets_ready_and_cleans_natives() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);

        let vanilla = store
            .create(NewInstance::vanilla("Survival", "1.20.1").unwrap(), 100)
            .unwrap();
        let v_dirs = store.dirs(&vanilla.id);
        fs::create_dir_all(v_dirs.saves_dir().join("MyWorld")).unwrap();
        fs::write(
            v_dirs.saves_dir().join("MyWorld").join("level.dat"),
            b"world-bytes",
        )
        .unwrap();
        fs::create_dir_all(v_dirs.natives_dir()).unwrap();
        fs::write(v_dirs.natives_dir().join("lwjgl.dll"), b"dll").unwrap();
        store
            .mark_ready(&vanilla.id, VersionId::new("1.20.1").unwrap(), None)
            .unwrap();

        // A Vanilla profile cannot be given a loader version.
        assert!(matches!(
            store.set_version(
                &vanilla.id,
                McVersion::new("1.21.4").unwrap(),
                Some(LoaderVersionSpec::Auto),
            ),
            Err(InstanceError::InconsistentSetup(_))
        ));

        // Same version is a no-op and keeps Ready + natives intact.
        let same = store
            .set_version(&vanilla.id, McVersion::new("1.20.1").unwrap(), None)
            .unwrap();
        assert!(same.is_ready());
        assert!(v_dirs.natives_dir().exists());

        // New version updates minecraft, resets setup to Pending, removes natives, keeps saves.
        let moved = store
            .set_version(&vanilla.id, McVersion::new("1.21.4").unwrap(), None)
            .unwrap();
        assert_eq!(moved.minecraft.as_str(), "1.21.4");
        assert_eq!(moved.setup, SetupState::Pending);
        assert!(!v_dirs.natives_dir().exists());
        assert_eq!(
            fs::read(v_dirs.saves_dir().join("MyWorld").join("level.dat")).unwrap(),
            b"world-bytes"
        );

        // Modded profile keeps its loader kind, updates MC + loader spec, and resets Ready.
        let modded = store
            .create(
                NewInstance::modded(
                    "Modded",
                    "1.20.1",
                    LoaderKind::Fabric,
                    LoaderVersionSpec::Pinned {
                        version: LoaderVersion::new("0.15.0").unwrap(),
                    },
                )
                .unwrap(),
                110,
            )
            .unwrap();
        store
            .mark_ready(
                &modded.id,
                VersionId::new("fabric-loader-0.15.0-1.20.1").unwrap(),
                Some(LoaderVersion::new("0.15.0").unwrap()),
            )
            .unwrap();
        let modded_updated = store
            .set_version(
                &modded.id,
                McVersion::new("1.21.4").unwrap(),
                Some(LoaderVersionSpec::Pinned {
                    version: LoaderVersion::new("0.16.9").unwrap(),
                }),
            )
            .unwrap();
        assert_eq!(modded_updated.minecraft.as_str(), "1.21.4");
        assert_eq!(modded_updated.loader(), Some(LoaderKind::Fabric));
        assert_eq!(modded_updated.setup, SetupState::Pending);
    }

    #[test]
    fn duplicate_clones_game_tree_preserves_ready_and_skips_natives_and_playtime() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);

        let src = store
            .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 200)
            .unwrap();
        store.set_memory(&src.id, Some(4096)).unwrap();
        store
            .mark_ready(&src.id, VersionId::new("1.21.4").unwrap(), None)
            .unwrap();

        let src_dirs = store.dirs(&src.id);
        fs::create_dir_all(src_dirs.saves_dir().join("Home")).unwrap();
        fs::write(
            src_dirs.saves_dir().join("Home").join("level.dat"),
            b"seed-42",
        )
        .unwrap();
        fs::write(src_dirs.game_dir().join("options.txt"), b"fov:1.0").unwrap();
        fs::create_dir_all(src_dirs.natives_dir()).unwrap();
        fs::write(src_dirs.natives_dir().join("lwjgl.dll"), b"dll").unwrap();
        fs::write(src_dirs.playtime_file(), b"{\"total_seconds\":900}").unwrap();

        let copy = store.duplicate(&src.id, "Survival (2)", 210).unwrap();
        assert_eq!(copy.id.as_str(), "survival-2");
        assert_eq!(copy.name, "Survival (2)");
        assert_eq!(copy.minecraft.as_str(), "1.21.4");
        assert!(copy.is_ready());
        assert_eq!(copy.memory_mb, Some(4096));

        let copy_dirs = store.dirs(&copy.id);
        assert_eq!(
            fs::read(copy_dirs.saves_dir().join("Home").join("level.dat")).unwrap(),
            b"seed-42"
        );
        assert_eq!(
            fs::read(copy_dirs.game_dir().join("options.txt")).unwrap(),
            b"fov:1.0"
        );
        assert!(!copy_dirs.natives_dir().exists());
        assert!(!copy_dirs.playtime_file().exists());
    }

    #[test]
    fn set_resolution_and_jvm_args_validate_persist_and_reject_unsafe_inputs() {
        let (_d, root) = setup();
        let store = InstanceStore::new(&root);
        let inst = store
            .create(NewInstance::vanilla("Bench", "1.21.4").unwrap(), 300)
            .unwrap();
        assert_eq!(inst.resolution, None);
        assert!(inst.extra_jvm_args.is_empty());

        let updated = store
            .set_resolution(
                &inst.id,
                Some(Resolution {
                    width: 1280,
                    height: 720,
                }),
            )
            .unwrap();
        assert_eq!(
            updated.resolution,
            Some(Resolution {
                width: 1280,
                height: 720
            })
        );
        assert!(matches!(
            store.set_resolution(
                &inst.id,
                Some(Resolution {
                    width: 100,
                    height: 720
                })
            ),
            Err(InstanceError::BadResolution(_))
        ));
        assert_eq!(
            store.set_resolution(&inst.id, None).unwrap().resolution,
            None
        );

        let with_jvm = store
            .set_jvm_args(
                &inst.id,
                vec![
                    "-XX:+UseStringDeduplication".into(),
                    "-Dfml.readTimeout=120".into(),
                ],
            )
            .unwrap();
        assert_eq!(with_jvm.extra_jvm_args.len(), 2);
        for forbidden in ["-Xmx4G", "-javaagent:evil.jar", "-cp", "plain"] {
            assert!(
                matches!(
                    store.set_jvm_args(&inst.id, vec![forbidden.into()]),
                    Err(InstanceError::BadJvmArg(_))
                ),
                "{forbidden}"
            );
        }
        assert!(store
            .set_jvm_args(&inst.id, vec![])
            .unwrap()
            .extra_jvm_args
            .is_empty());
    }
}

#[cfg(test)]
mod retire_tests;
