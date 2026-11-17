//! Building the real Minecraft launch command from a prepared profile.
//!
//! Nothing here starts a process or touches the network. It reads the version metadata that
//! preparation stored under `minecraft/versions/`, resolves a loader profile's `inheritsFrom`
//! chain, and assembles `java <jvm args> <main class> <game args>` the way Mojang's metadata
//! describes, for every generation of Minecraft:
//! - modern versions: `arguments.jvm` and `arguments.game` with Mojang's rules (Windows x64, every
//!   launcher feature off);
//! - older versions: the single `minecraftArguments` string plus the default JVM arguments the
//!   official launcher always supplied (`-Djava.library.path`, the launcher brand, `-cp`);
//! - the oldest versions: positional arguments and `${auth_session}`.
//!
//! Loader profiles inherit from Vanilla: the loader's libraries come first, duplicates (same
//! `group:artifact[:classifier]`) keep the loader's copy, JVM and game arguments are the base's
//! followed by the loader's, and the main class is the loader's. `${version_name}` is the base
//! Minecraft version, and the client jar on the classpath is that version's jar (which is what
//! Forge/NeoForge's `-DignoreList=...${version_name}.jar` expects).
//!
//! The launch identity is the real account's UUID, profile name and Minecraft access token. The
//! token is held in a [`Secret`]; the command's `Debug` and its [`GameCommand::redacted`] text
//! never contain it, so it can be logged safely.
//!
//! Isolation: the working directory and `--gameDir` are the instance's `game/` folder; the
//! process gets a clean environment whose profile, AppData and TEMP folders live inside the
//! Clade folder, so nothing the game or a mod writes by default lands on the machine.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::instances::InstanceDirs;
use crate::loader_install::SystemEnv;
use crate::mc_meta::{
    parse_asset_index, parse_version, Arg, AssetIndexRef, JavaRequirement, Library, LogConfig,
    MetaError, Role, VersionPlan, WINDOWS_X64,
};
use crate::ms_auth::Secret;
use crate::portable::PortableRoot;
use crate::runtimes::ManagedJava;

pub const LAUNCHER_NAME: &str = "Clade";
/// Above this UTF-16 length, `-cp` entries under the Clade folder are rewritten relative to `cwd`.
const COMPACT_CP_CHARS: usize = 20_000;
/// Above this UTF-16 length on Java 9+, `-cp` spills to `instances/<id>/.clade-jvm.args`.
const MAX_COMMAND_CHARS: usize = 28_000;
/// Windows refuses a `CreateProcessW` command line longer than 32,767 `WCHAR`s; keep safety margin.
const HARD_MAX_COMMAND_CHARS: usize = 30_000;
const MAX_VERSION_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CHAIN: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("the version information for {0} is missing")]
    MissingVersion(String),
    #[error("the version information is not usable: {0}")]
    BadVersion(String),
    #[error("the version information is incomplete: {0}")]
    Incomplete(&'static str),
    #[error("a file the game needs is missing: {0}")]
    MissingFile(String),
    #[error("the game's command line would be too long for Windows")]
    TooLong,
    #[error(
        "the Clade folder path cannot contain ';' because Java uses ';' to separate libraries"
    )]
    UnsupportedFolderPath,
    #[error("a path is not valid Unicode")]
    NotUtf8,
}

impl From<MetaError> for LaunchError {
    fn from(e: MetaError) -> Self {
        Self::BadVersion(e.to_string())
    }
}

// ------------------------------------------------------------------------------ resolving

/// A version with its `inheritsFrom` chain merged: what is actually launched.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The profile that was asked for (`fabric-loader-...`, or the Minecraft version itself).
    pub launch_id: String,
    /// The Vanilla version at the root of the chain; its jar goes on the classpath.
    pub base_id: String,
    pub main_class: String,
    pub jvm: Vec<Arg>,
    pub game: Vec<Arg>,
    pub legacy_args: Option<String>,
    pub libraries: Vec<Library>,
    pub asset_index: Option<AssetIndexRef>,
    pub logging: Option<LogConfig>,
    pub java: JavaRequirement,
    pub version_type: String,
}

fn version_file(root: &PortableRoot, id: &str) -> PathBuf {
    root.minecraft_dir()
        .join("versions")
        .join(id)
        .join(format!("{id}.json"))
}

/// The chain `[launched, parent, ..., root]`, read from the saved metadata.
fn load_chain(root: &PortableRoot, id: &str) -> Result<Vec<VersionPlan>, LaunchError> {
    let mut chain: Vec<VersionPlan> = Vec::new();
    let mut next = Some(id.to_owned());
    while let Some(cur) = next.take() {
        if chain.len() >= MAX_CHAIN || chain.iter().any(|p| p.id == cur) {
            return Err(LaunchError::BadVersion(
                "the inheritance chain is too deep or loops".into(),
            ));
        }
        crate::versions::VersionId::new(&cur)
            .map_err(|e| LaunchError::BadVersion(e.to_string()))?;
        let vfile = version_file(root, &cur);
        crate::portable::fsutil::reject_links_between(root.path(), &vfile)
            .map_err(|e| LaunchError::BadVersion(e.to_string()))?;
        let bytes =
            crate::portable::fsutil::read_bounded(&vfile, MAX_VERSION_JSON_BYTES).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LaunchError::MissingVersion(cur.clone())
                } else {
                    LaunchError::BadVersion(e.to_string())
                }
            })?;
        let plan = parse_version(&bytes, &WINDOWS_X64)?;
        if plan.id != cur {
            return Err(LaunchError::BadVersion(format!(
                "version metadata for {cur} declares id {}",
                plan.id
            )));
        }
        next = plan.inherits_from.clone();
        chain.push(plan);
    }
    Ok(chain)
}

/// `group:artifact[:classifier]` without the version.
fn library_key(name: &str) -> String {
    let main = name.split('@').next().unwrap_or(name);
    let mut p = main.split(':');
    let (g, a) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
    let _version = p.next();
    match p.next() {
        Some(c) => format!("{g}:{a}:{c}"),
        None => format!("{g}:{a}"),
    }
}

pub fn resolve(root: &PortableRoot, launch_id: &str) -> Result<Resolved, LaunchError> {
    let chain = load_chain(root, launch_id)?;
    let base = chain.last().ok_or(LaunchError::Incomplete("no version"))?;
    base.client.as_ref().ok_or(LaunchError::Incomplete(
        "no client download in the base version",
    ))?;

    let main_class = chain
        .iter()
        .find_map(|p| p.main_class.clone())
        .ok_or(LaunchError::Incomplete("no main class"))?;

    // Libraries: the launched profile first, then its parents; the first copy of a key wins.
    let mut seen = HashSet::new();
    let libraries: Vec<Library> = chain
        .iter()
        .flat_map(|p| p.libraries.iter())
        .filter(|l| seen.insert(library_key(&l.name)))
        .cloned()
        .collect();

    // Arguments: the base's, then each child's on top (root to launched).
    // Legacy `minecraftArguments` replaces prior game arguments (Forge 1.7.10 / early 1.12.2),
    // while structured `arguments.game` appends on top (Forge 1.12.2 2851+, whose child JSON
    // carries `arguments.game: ["--tweakClass", "..."]` without `minecraftArguments`).
    let (mut jvm, mut game) = (Vec::new(), Vec::new());
    for plan in chain.iter().rev() {
        if let Some(s) = &plan.minecraft_arguments {
            game = s
                .split_whitespace()
                .map(|t| Arg::Plain(t.to_owned()))
                .collect();
        }
        if let Some(a) = &plan.arguments {
            jvm.extend(a.jvm.iter().cloned());
            game.extend(a.game.iter().cloned());
        }
    }
    let legacy_args = chain.iter().find_map(|p| p.minecraft_arguments.clone());
    if game.is_empty() {
        return Err(LaunchError::Incomplete("no game arguments"));
    }

    Ok(Resolved {
        launch_id: launch_id.to_owned(),
        base_id: base.id.clone(),
        main_class,
        jvm,
        game,
        legacy_args,
        libraries,
        asset_index: chain.iter().find_map(|p| p.asset_index.clone()),
        logging: chain.iter().find_map(|p| p.logging.clone()),
        java: base.java.clone(),
        version_type: chain
            .iter()
            .find_map(|p| p.version_type.clone())
            .unwrap_or_else(|| "release".into()),
    })
}

// ------------------------------------------------------------------------------ the command

/// Who is playing. Built from the account service; the token is a [`Secret`].
pub struct Identity {
    pub uuid: String,
    pub name: String,
    pub access_token: Secret,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Identity({}, <redacted>)", self.name)
    }
}

pub struct GameCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The complete environment. The process starts from an empty one.
    pub env: BTreeMap<String, String>,
    secrets: Vec<String>,
}

impl Drop for GameCommand {
    fn drop(&mut self) {
        for a in &mut self.args {
            zeroize::Zeroize::zeroize(a);
        }
        for s in &mut self.secrets {
            zeroize::Zeroize::zeroize(s);
        }
    }
}

impl fmt::Debug for GameCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GameCommand({} args, <secrets redacted>)",
            self.args.len()
        )
    }
}

impl GameCommand {
    /// The secret values that must never be written anywhere.
    pub fn secrets(&self) -> &[String] {
        &self.secrets
    }

    /// The command line as text that is safe to log: secrets replaced, and the Clade folder
    /// shown as `<clade>` so the log does not depend on where the folder lives.
    pub fn redacted(&self, root: &PortableRoot) -> String {
        let rootp = root.path().to_string_lossy().into_owned();
        let prog = self.program.to_string_lossy().into_owned();
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        let mut hide_next = false;
        for a in std::iter::once(&prog).chain(self.args.iter()) {
            let mut text = a.clone();
            if hide_next {
                text = "<redacted>".into();
                hide_next = false;
            } else if a == "--accessToken" || a == "--session" {
                hide_next = true;
            }
            for s in &self.secrets {
                if !s.is_empty() {
                    text = text.replace(s.as_str(), "<redacted>");
                }
            }
            text = text
                .replace(&rootp, "<clade>")
                .replace(r"..\..\..", "<clade>")
                .replace("../../..", "<clade>");
            parts.push(if text.contains(' ') || text.is_empty() {
                format!("\"{text}\"")
            } else {
                text
            });
        }
        parts.join(" ")
    }

    /// Builds the process: environment cleared first, then only our explicit variables, and no
    /// console window. The game is started directly, never through a shell.
    pub fn to_process(&self) -> std::process::Command {
        // portability-allow: the single place the game process is created; env is scrubbed below
        let mut c = std::process::Command::new(&self.program);
        c.env_clear()
            .envs(&self.env)
            .args(&self.args)
            .current_dir(&self.cwd);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        c
    }
}

fn quoted_utf16_arg_len<I>(units: I, force_quotes: bool) -> usize
where
    I: Iterator<Item = u16> + Clone,
{
    let mut is_empty = true;
    let mut needs_quotes = force_quotes;
    for c in units.clone() {
        is_empty = false;
        if c == u16::from(b' ') || c == u16::from(b'\t') || c == u16::from(b'"') {
            needs_quotes = true;
            break;
        }
    }
    let quote = needs_quotes || is_empty;
    let mut len = usize::from(quote);
    let mut backslashes = 0usize;
    for c in units {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                len += backslashes + 1;
            }
            backslashes = 0;
        }
        len += 1;
    }
    if quote {
        len += backslashes + 1;
    }
    len
}

/// Exact UTF-16 `WCHAR` length (including quotes, escaped backslashes/quotes, separating spaces,
/// and terminating `NUL`) of the Windows CRT command line `CreateProcessW` receives.
pub fn utf16_command_line_len(program: &Path, args: &[String]) -> usize {
    #[cfg(windows)]
    let mut total = {
        use std::os::windows::ffi::OsStrExt;
        quoted_utf16_arg_len(program.as_os_str().encode_wide(), true)
    };
    #[cfg(not(windows))]
    let mut total = {
        let s = program.to_string_lossy();
        quoted_utf16_arg_len(s.encode_utf16(), true)
    };
    for arg in args {
        total += 1 + quoted_utf16_arg_len(arg.encode_utf16(), false);
    }
    total + 1
}

fn utf8(p: &Path) -> Result<String, LaunchError> {
    p.to_str().map(str::to_owned).ok_or(LaunchError::NotUtf8)
}

/// What a version's assets look like on disk (older versions read plain, named files).
enum AssetLayout {
    Hashed,
    Virtual(String),
    InGameDir,
}

fn asset_layout(root: &PortableRoot, r: &Resolved) -> Result<AssetLayout, LaunchError> {
    let Some(ai) = &r.asset_index else {
        return Ok(AssetLayout::Hashed);
    };
    let bytes = fs::read(
        root.assets_dir()
            .join("indexes")
            .join(format!("{}.json", ai.id)),
    )
    .map_err(|_| LaunchError::MissingFile("the asset index".into()))?;
    let index = parse_asset_index(&bytes)?;
    Ok(if index.map_to_resources {
        AssetLayout::InGameDir
    } else if index.virtual_files {
        AssetLayout::Virtual(ai.assets_name.clone())
    } else {
        AssetLayout::Hashed
    })
}

pub struct BuildInput<'a> {
    pub root: &'a PortableRoot,
    pub dirs: &'a InstanceDirs,
    pub resolved: &'a Resolved,
    pub java: &'a ManagedJava,
    pub identity: &'a Identity,
    pub memory_mb: u32,
    /// The profile's own JVM arguments (validated when the profile was saved).
    pub extra_jvm_args: &'a [String],
    pub sys: &'a SystemEnv,
    /// Where the game's temporary files go (inside the Clade folder).
    pub temp_dir: &'a Path,
}

fn substitute(template: &str, vars: &BTreeMap<&str, String>, unknown: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(v) => out.push_str(v),
                    None => unknown.push(key.to_owned()), // unknown placeholder: becomes empty
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn default_jvm() -> Vec<Arg> {
    [
        "-Djava.library.path=${natives_directory}",
        "-Dminecraft.launcher.brand=${launcher_name}",
        "-Dminecraft.launcher.version=${launcher_version}",
        "-cp",
        "${classpath}",
    ]
    .into_iter()
    .map(|s| Arg::Plain(s.to_owned()))
    .collect()
}

fn compact_classpath_relative(
    jvm_args: &mut [String],
    root: &Path,
    game_dir: &Path,
) -> Result<(), LaunchError> {
    let Some(cp_idx) = (0..jvm_args.len().saturating_sub(1))
        .find(|&idx| jvm_args[idx] == "-cp" || jvm_args[idx] == "-classpath")
    else {
        return Ok(());
    };
    let Some(rel_game) = crate::portable::fsutil::strip_prefix_portable(game_dir, root) else {
        return Ok(());
    };
    let depth = rel_game
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .count();
    if depth == 0 {
        return Ok(());
    }
    let mut up = PathBuf::new();
    for _ in 0..depth {
        up.push("..");
    }
    let mut rewritten = Vec::new();
    for entry in jvm_args[cp_idx + 1].split(';') {
        if let Some(rel) = crate::portable::fsutil::strip_prefix_portable(Path::new(entry), root) {
            rewritten.push(utf8(&up.join(rel))?);
        } else {
            rewritten.push(entry.to_owned());
        }
    }
    jvm_args[cp_idx + 1] = rewritten.join(";");
    Ok(())
}

fn maybe_spill_classpath_argfile(
    args: &mut Vec<String>,
    main_idx: usize,
    root: &Path,
    instance_root: &Path,
    token: &str,
) -> Result<(), LaunchError> {
    let Some(cp_idx) = (0..main_idx.saturating_sub(1))
        .find(|&idx| args[idx] == "-cp" || args[idx] == "-classpath")
    else {
        return Ok(());
    };
    let cp_val = &args[cp_idx + 1];
    // Java argfiles are read in the OS ANSI codepage, never UTF-8, and must never hold credentials.
    if !cp_val.is_ascii() || (!token.is_empty() && cp_val.contains(token)) {
        return Ok(());
    }
    let mut escaped = String::with_capacity(cp_val.len() + 64);
    for ch in cp_val.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            c => escaped.push(c),
        }
    }
    let content = format!("-cp\n\"{escaped}\"\n");
    let argfile = instance_root.join(".clade-jvm.args");
    crate::portable::fsutil::reject_links_between(root, &argfile)
        .map_err(|e| LaunchError::BadVersion(e.to_string()))?;
    if crate::portable::fsutil::atomic_write(&argfile, content.as_bytes()).is_ok() {
        args.splice(cp_idx..=cp_idx + 1, ["@../.clade-jvm.args".to_owned()]);
    }
    Ok(())
}

pub fn build(i: &BuildInput<'_>) -> Result<GameCommand, LaunchError> {
    if utf8(i.root.path())?.contains(';')
        || utf8(i.dirs.root())?.contains(';')
        || utf8(i.temp_dir)?.contains(';')
    {
        return Err(LaunchError::UnsupportedFolderPath);
    }

    let r = i.resolved;
    let libdir = i.root.libraries_dir();

    // Classpath: libraries (loader first), then the base version's client jar.
    let mut missing = Vec::new();
    let mut cp: Vec<String> = Vec::new();
    for lib in r
        .libraries
        .iter()
        .filter(|l| matches!(l.role, Role::Classpath | Role::ClasspathAndNative { .. }))
    {
        let p = lib.artifact.path.to_path(&libdir);
        if !p.is_file() {
            missing.push(lib.name.clone());
        }
        cp.push(utf8(&p)?);
    }
    let client_jar = i
        .root
        .minecraft_dir()
        .join("versions")
        .join(&r.base_id)
        .join(format!("{}.jar", r.base_id));
    if !client_jar.is_file() {
        missing.push(format!("Minecraft {}", r.base_id));
    }
    cp.push(utf8(&client_jar)?);
    if let Some(first) = missing.first() {
        return Err(LaunchError::MissingFile(if missing.len() == 1 {
            first.clone()
        } else {
            format!("{first} and {} more", missing.len() - 1)
        }));
    }

    let assets_root = i.root.assets_dir();
    let game_dir = i.dirs.game_dir();
    let (game_assets, _) = match asset_layout(i.root, r)? {
        AssetLayout::Hashed => (assets_root.clone(), ()),
        AssetLayout::Virtual(name) => (assets_root.join("virtual").join(name), ()),
        AssetLayout::InGameDir => (game_dir.join("resources"), ()),
    };
    let log_path = r
        .logging
        .as_ref()
        .map(|l| i.root.assets_dir().join("log_configs").join(&l.file_name));
    if let Some(p) = &log_path {
        if !p.is_file() {
            return Err(LaunchError::MissingFile("the logging configuration".into()));
        }
    }

    let uuid = i.identity.uuid.replace('-', "").to_ascii_lowercase();
    let mut token = zeroize::Zeroizing::new(i.identity.access_token.expose().to_owned());
    let mut vars: BTreeMap<&str, String> = BTreeMap::new();
    for (k, v) in [
        ("auth_player_name", i.identity.name.clone()),
        ("auth_uuid", uuid.clone()),
        ("auth_access_token", (*token).clone()),
        ("auth_session", format!("token:{}:{}", *token, uuid)),
        ("user_type", "msa".to_owned()),
        ("user_properties", "{}".to_owned()),
        // Optional telemetry identifiers the game accepts empty.
        ("clientid", String::new()),
        ("auth_xuid", String::new()),
        ("version_name", r.base_id.clone()),
        ("version_type", r.version_type.clone()),
        ("game_directory", utf8(&game_dir)?),
        ("assets_root", utf8(&assets_root)?),
        ("game_assets", utf8(&game_assets)?),
        (
            "assets_index_name",
            r.asset_index
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_default(),
        ),
        ("natives_directory", utf8(&i.dirs.natives_dir())?),
        ("launcher_name", LAUNCHER_NAME.to_owned()),
        ("launcher_version", env!("CARGO_PKG_VERSION").to_owned()),
        ("classpath", cp.join(";")),
        ("classpath_separator", ";".to_owned()),
        ("library_directory", utf8(&libdir)?),
    ] {
        vars.insert(k, v);
    }

    // JVM arguments from the metadata (or the defaults older versions never listed).
    let mut jvm_template: Vec<String> = r.jvm.iter().flat_map(|a| a.values(&WINDOWS_X64)).collect();
    if !jvm_template.iter().any(|a| a.contains("${classpath}")) {
        jvm_template = default_jvm()
            .iter()
            .flat_map(|a| a.values(&WINDOWS_X64))
            .chain(jvm_template)
            .collect();
    }
    let game_template: Vec<String> = match (&r.legacy_args, r.game.is_empty()) {
        (_, false) => r.game.iter().flat_map(|a| a.values(&WINDOWS_X64)).collect(),
        (Some(s), true) => s.split_whitespace().map(str::to_owned).collect(),
        (None, true) => return Err(LaunchError::Incomplete("no game arguments")),
    };

    let mut unknown = Vec::new();
    let home = i.dirs.home_dir();
    let mut args: Vec<String> = vec![
        format!("-Xmx{}M", i.memory_mb),
        "-Dfile.encoding=UTF-8".into(),
        "-Dsun.stdout.encoding=UTF-8".into(),
        "-Dsun.stderr.encoding=UTF-8".into(),
    ];
    if i.java.java_major >= 19 {
        args.push("-Dstdout.encoding=UTF-8".into());
        args.push("-Dstderr.encoding=UTF-8".into());
    }
    args.push("-Dlog4j2.formatMsgNoLookups=true".into());
    if i.java.java_major <= 8 {
        args.push("-XX:-CreateMinidumpOnCrash".into());
    } else {
        args.push("-XX:-CreateCoredumpOnCrash".into());
    }
    let region_mb = if i.memory_mb <= 2048 { 16 } else { 32 };
    let direct_mb = (i.memory_mb / 2).clamp(512, 2048);
    args.extend([
        "-XX:+UnlockExperimentalVMOptions".into(),
        "-XX:+UseG1GC".into(),
        "-XX:G1NewSizePercent=20".into(),
        "-XX:G1ReservePercent=20".into(),
        "-XX:MaxGCPauseMillis=50".into(),
        format!("-XX:G1HeapRegionSize={region_mb}M"),
        format!("-XX:MaxDirectMemorySize={direct_mb}M"),
    ]);
    if i.java.java_major >= 12 {
        args.push("-XX:G1PeriodicGCInterval=60000".into());
    }
    if i.java.java_major <= 8
        && !jvm_template
            .iter()
            .any(|a| a.starts_with("-XX:HeapDumpPath="))
    {
        args.push(
            "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump"
                .into(),
        );
    }
    args.push(format!("-Djava.io.tmpdir={}", utf8(i.temp_dir)?));
    args.push(format!("-Duser.home={}", utf8(&home)?));
    args.extend(
        jvm_template
            .iter()
            .map(|a| substitute(a, &vars, &mut unknown)),
    );
    if let (Some(template), Some(p)) = (
        r.logging.as_ref().and_then(|l| l.argument.as_deref()),
        &log_path,
    ) {
        // `${path}` is the saved logging configuration.
        let mut v = vars.clone();
        v.insert("path", utf8(p)?);
        args.push(substitute(template, &v, &mut unknown));
        for val in v.values_mut() {
            zeroize::Zeroize::zeroize(val);
        }
    }
    args.extend(i.extra_jvm_args.iter().cloned());
    let main_idx = args.len();
    args.push(r.main_class.clone());
    args.extend(
        game_template
            .iter()
            .map(|a| substitute(a, &vars, &mut unknown)),
    );
    for val in vars.values_mut() {
        zeroize::Zeroize::zeroize(val);
    }

    if utf16_command_line_len(&i.java.exe, &args) > COMPACT_CP_CHARS {
        compact_classpath_relative(&mut args[..main_idx], i.root.path(), &game_dir)?;
    }
    if utf16_command_line_len(&i.java.exe, &args) > MAX_COMMAND_CHARS && i.java.java_major >= 9 {
        maybe_spill_classpath_argfile(&mut args, main_idx, i.root.path(), i.dirs.root(), &token)?;
    }
    if utf16_command_line_len(&i.java.exe, &args) > HARD_MAX_COMMAND_CHARS {
        for a in &mut args {
            zeroize::Zeroize::zeroize(a);
        }
        return Err(LaunchError::TooLong);
    }

    let (sys_root, tmp, userprofile) = (utf8(&i.sys.system_root)?, utf8(i.temp_dir)?, utf8(&home)?);
    let sys_drive = if sys_root
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && sys_root.as_bytes().get(1) == Some(&b':')
    {
        &sys_root[..2]
    } else {
        "C:"
    };
    let mut env = BTreeMap::new();
    for (k, v) in [
        ("SystemDrive", sys_drive.to_owned()),
        ("SystemRoot", sys_root.clone()),
        ("windir", sys_root.clone()),
        (
            "PATH",
            format!(r"{sys_root}\System32;{sys_root};{sys_root}\System32\Wbem"),
        ),
        ("COMSPEC", format!(r"{sys_root}\System32\cmd.exe")),
        ("PATHEXT", ".COM;.EXE;.BAT;.CMD".to_owned()),
        ("TEMP", tmp.clone()),
        ("TMP", tmp.clone()),
        ("TMPDIR", tmp),
        ("USERPROFILE", userprofile.clone()),
        ("HOME", userprofile),
        ("APPDATA", utf8(&home.join("AppData").join("Roaming"))?),
        ("LOCALAPPDATA", utf8(&home.join("AppData").join("Local"))?),
    ] {
        env.insert(k.to_owned(), v);
    }

    Ok(GameCommand {
        program: i.java.exe.clone(),
        args,
        cwd: game_dir,
        env,
        secrets: vec![std::mem::take(&mut *token)],
    })
}

#[cfg(test)]
mod tests;
