//! Portable, sandboxed loader installation.
//!
//! Rule (docs/TECHNICAL.md, "Loaders"): installing Fabric, Forge, NeoForge or Quilt must never use
//! the global `.minecraft`, a user-profile cache, the system TEMP directory or any other
//! machine-specific path. Everything a loader installer reads, writes or generates stays inside
//! the portable launcher root, and nothing here ever needs administrator rights.
//!
//! Why this is needed (read from the official Forge installer source): when no usable target is
//! given, it falls back to `%APPDATA%\.minecraft` on Windows (and `user.home`/`.minecraft`
//! elsewhere), where `%APPDATA%` comes from the process environment and `user.home` from the JVM.
//! We therefore never rely on the installer behaving: the process gets a *scrubbed* environment
//! in which every machine-specific location (`APPDATA`, `LOCALAPPDATA`, `USERPROFILE`, `HOME`,
//! `TEMP`, `TMP`, `-Duser.home`, `-Djava.io.tmpdir`) points into a per-install staging directory
//! under `cache/`. Even a mis-passed or ignored target argument can then only land inside the
//! launcher folder.
//!
//! Defence in depth, in the order it is applied:
//! 1. [`StageSession`]: a fresh staging tree `cache/loader-install/<id>/{mc,home,tmp}` with the
//!    stub `launcher_profiles.json` installers insist on. The installer's target is `mc/`.
//! 2. [`build_installer_command`]: scrubbed env + JVM flags + a target inside the stage; refuses
//!    Fabric/Quilt (they need no process at all) and never passes `--mirror`.
//! 3. [`verify_command`]: re-checks the finished command (program, working directory, every
//!    argument and environment value that looks like a path) and fails closed on anything
//!    outside the launcher root, except the read-only Windows system directory.
//! 4. [`Tripwire`]: snapshots the real global `.minecraft` locations before the run and fails
//!    loudly if any of them changed (it cannot undo a write, but it makes a leak impossible to miss).
//! 5. [`merge_stage_into_shared`]: only `libraries/` and `versions/` are taken from the stage,
//!    into the shared immutable stores, with strict path validation and a refusal to overwrite
//!    existing files with different content. Everything else the installer produced is discarded
//!    with the stage.
//!
//! Fabric and Quilt run no installer: [`library_relpath`] derives where each library from the
//! (untrusted) version profile goes under `libraries/`, safely.
//!
//! Residual limits (documented in docs/TECHNICAL.md): the JVM may still write Java preferences to the
//! Windows registry (it cannot be redirected), and the installer makes its own network
//! requests (the Forge installer enforces its own trusted-host list).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::loaders::LoaderKind;
use crate::portable::fsutil::{is_link, sha256_hex};
use crate::portable::{PortableRoot, RelPath};
use crate::runtimes::ManagedJava;
use crate::versions::{McVersion, VersionId};

const STAGE_PARENT: &str = "loader-install";
const MAX_WALK_ENTRIES: usize = 200_000;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("sandbox violation: {0}")]
    Violation(String),
    #[error("{0:?} needs no installer process")]
    NoInstallerNeeded(LoaderKind),
    #[error("refusing to overwrite {0} with different content")]
    Conflict(String),
    #[error("invalid library coordinate {0:?}")]
    BadCoordinate(String),
    #[error("the installer changed files outside the launcher folder: {0:?}")]
    EscapedSandbox(Vec<PathBuf>),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

fn violation(msg: impl Into<String>) -> InstallError {
    InstallError::Violation(msg.into())
}

// ---------------------------------------------------------------------------------------
// Lexical path containment
// ---------------------------------------------------------------------------------------

fn norm_components(p: &Path) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for c in p.components() {
        match c {
            Component::Prefix(x) => out.push(x.as_os_str().to_string_lossy().to_lowercase()),
            Component::RootDir => out.push("/".into()),
            Component::Normal(n) => out.push(if cfg!(windows) {
                n.to_string_lossy().to_lowercase()
            } else {
                n.to_string_lossy().into_owned()
            }),
            Component::CurDir => {}
            Component::ParentDir => return None, // `..` is never acceptable in a path we pass on
        }
    }
    Some(out)
}

/// True if `p` is lexically inside `base` (case-insensitive on Windows, no `..` allowed).
pub fn is_inside(base: &Path, p: &Path) -> bool {
    match (norm_components(base), norm_components(p)) {
        (Some(b), Some(c)) => c.len() >= b.len() && c[..b.len()] == b[..],
        _ => false,
    }
}

/// Does this string look like an absolute filesystem path (Windows drive/UNC, or POSIX)?
fn looks_absolute(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || s.starts_with("\\\\")
        || (s.starts_with('/') && s.len() > 1)
}

/// Pulls a path out of `-Dkey=value`, `--flag=value`, or a bare value.
fn embedded_path(arg: &str) -> &str {
    if let Some(rest) = arg.strip_prefix("-D") {
        if let Some((_, v)) = rest.split_once('=') {
            return v;
        }
    }
    if arg.starts_with("--") {
        if let Some((_, v)) = arg.split_once('=') {
            return v;
        }
    }
    arg
}

fn path_str(p: &Path) -> Result<String, InstallError> {
    p.to_str()
        .map(str::to_owned)
        .ok_or_else(|| violation("path is not valid Unicode"))
}

// ---------------------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------------------

/// A private working tree for exactly one loader installation, inside `cache/`.
#[derive(Debug)]
pub struct StageSession {
    base: PathBuf,
}

impl StageSession {
    pub fn create(root: &PortableRoot) -> Result<Self, InstallError> {
        let parent = root.cache_dir().join(STAGE_PARENT);
        fs::create_dir_all(&parent)?;
        let mut nonce = [0u8; 6];
        getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
        let base = parent.join(hex::encode(nonce));
        fs::create_dir(&base)?; // fails if it exists: sessions are never shared
        let s = Self { base };
        for d in [
            s.minecraft_dir(),
            s.appdata_dir(),
            s.localappdata_dir(),
            s.tmp_dir(),
        ] {
            fs::create_dir_all(d)?;
        }
        // Installers refuse to run in a directory that does not look like a Minecraft folder.
        fs::write(
            s.minecraft_dir().join("launcher_profiles.json"),
            br#"{"profiles":{},"version":3}"#,
        )?;
        Ok(s)
    }

    pub fn base(&self) -> &Path {
        &self.base
    }
    /// The installer's target ("the .minecraft"): stage-local, never the global one.
    pub fn minecraft_dir(&self) -> PathBuf {
        self.base.join("mc")
    }
    /// Stands in for the user profile (`USERPROFILE`, `HOME`, `user.home`).
    pub fn home_dir(&self) -> PathBuf {
        self.base.join("home")
    }
    pub fn appdata_dir(&self) -> PathBuf {
        self.base.join("home").join("AppData").join("Roaming")
    }
    pub fn localappdata_dir(&self) -> PathBuf {
        self.base.join("home").join("AppData").join("Local")
    }
    /// Stands in for the system TEMP directory.
    pub fn tmp_dir(&self) -> PathBuf {
        self.base.join("tmp")
    }

    /// Installers patch the vanilla client, so they need its files in the target. Copies
    /// `minecraft/versions/<mc>/` from the shared store into the stage.
    pub fn seed_vanilla(&self, root: &PortableRoot, mc: &McVersion) -> Result<(), InstallError> {
        let src = root.minecraft_dir().join("versions").join(mc.as_str());
        let dst = self.minecraft_dir().join("versions").join(mc.as_str());
        fs::create_dir_all(&dst)?;
        for entry in fs::read_dir(&src)? {
            let entry = entry?;
            let path = entry.path();
            if is_link(&path)? || !entry.file_type()?.is_file() {
                return Err(violation(format!(
                    "unexpected entry in the vanilla version store: {}",
                    path.display()
                )));
            }
            fs::copy(&path, dst.join(entry.file_name()))?;
        }
        Ok(())
    }

    pub fn cleanup(self) -> io::Result<()> {
        fs::remove_dir_all(&self.base)
    }
}

/// Removes staging trees left behind by a crash. Safe to call at every start.
pub fn sweep_stale(root: &PortableRoot) -> io::Result<usize> {
    let parent = root.cache_dir().join(STAGE_PARENT);
    let Ok(rd) = fs::read_dir(&parent) else {
        return Ok(0);
    };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if is_link(&p).unwrap_or(true) {
            continue; // never follow or delete through a link
        }
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            fs::remove_dir_all(&p)?;
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------
// The sanitized installer command
// ---------------------------------------------------------------------------------------

/// The only machine-specific value the installer process may see: the read-only Windows
/// system directory (Java needs it to load system DLLs). Passed in so tests are deterministic.
#[derive(Debug, Clone)]
pub struct SystemEnv {
    pub system_root: PathBuf,
}

impl SystemEnv {
    pub fn from_process() -> Self {
        // portability-allow: read-only lookup of the Windows system directory, never persisted
        let root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            // portability-allow: fallback for the read-only Windows system directory
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        Self { system_root: root }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The complete environment. The process starts from an empty one; nothing is inherited.
    pub env: BTreeMap<String, String>,
}

/// Environment variables that inject JVM options or classes and so could bypass the redirection.
const FORBIDDEN_ENV: &[&str] = &[
    "JAVA_HOME",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "CLASSPATH",
    "JAVA_OPTS",
];
/// Env keys allowed to hold the system directory.
const SYSTEM_ENV_KEYS: &[&str] = &["SystemRoot", "windir", "PATH"];

/// How the official installer for each loader is told to install a client. NOTE: these flags
/// are taken from the installer sources (`--installClient [directory]`) but have not been run
/// against the current installer jars here. Confirm with the real jars before enabling the
/// pipeline. Safety does not depend on them: the environment redirection and [`Tripwire`]
/// protect the machine even if an installer ignores or rejects an argument.
fn client_install_flags(kind: LoaderKind, target: &str) -> Result<Vec<String>, InstallError> {
    match kind {
        LoaderKind::Forge | LoaderKind::NeoForge => {
            Ok(vec!["--installClient".to_owned(), target.to_owned()])
        }
        LoaderKind::Fabric | LoaderKind::Quilt => Err(InstallError::NoInstallerNeeded(kind)),
    }
}

pub fn build_installer_command(
    root: &PortableRoot,
    session: &StageSession,
    java: &ManagedJava,
    installer_jar: &Path,
    kind: LoaderKind,
    sys: &SystemEnv,
) -> Result<InstallerCommand, InstallError> {
    let tmp = path_str(&session.tmp_dir())?;
    let home = path_str(&session.home_dir())?;
    let target = path_str(&session.minecraft_dir())?;
    let sys_root = path_str(&sys.system_root)?;

    let mut args = vec![
        format!("-Djava.io.tmpdir={tmp}"),
        format!("-Duser.home={home}"),
        "-Dfile.encoding=UTF-8".to_owned(),
        "-Djava.awt.headless=true".to_owned(),
        "-jar".to_owned(),
        path_str(installer_jar)?,
    ];
    args.extend(client_install_flags(kind, &target)?);

    let mut env = BTreeMap::new();
    for (k, v) in [
        ("SystemRoot", sys_root.clone()),
        ("windir", sys_root.clone()),
        ("PATH", format!(r"{sys_root}\System32")),
        ("TEMP", tmp.clone()),
        ("TMP", tmp.clone()),
        ("TMPDIR", tmp),
        ("USERPROFILE", home.clone()),
        ("HOME", home),
        ("APPDATA", path_str(&session.appdata_dir())?),
        ("LOCALAPPDATA", path_str(&session.localappdata_dir())?),
    ] {
        env.insert(k.to_owned(), v);
    }

    let cmd = InstallerCommand {
        program: java.exe.clone(),
        args,
        cwd: session.base().to_path_buf(),
        env,
    };
    verify_command(&cmd, root, sys)?;
    Ok(cmd)
}

/// Fails closed if the command could touch anything outside the launcher root (apart from the
/// Windows system directory). Called when building and must be called again right before spawning.
pub fn verify_command(
    cmd: &InstallerCommand,
    root: &PortableRoot,
    sys: &SystemEnv,
) -> Result<(), InstallError> {
    let r = root.path();
    if !is_inside(&root.java_runtimes_dir(), &cmd.program) {
        return Err(violation(format!(
            "program {} is not a launcher-managed runtime",
            cmd.program.display()
        )));
    }
    if !is_inside(&root.cache_dir().join(STAGE_PARENT), &cmd.cwd) {
        return Err(violation(
            "working directory is not an installer staging directory",
        ));
    }
    for a in &cmd.args {
        if a == "--mirror" || a.starts_with("--mirror=") {
            return Err(violation(
                "installers must not be pointed at a custom mirror",
            ));
        }
        let candidate = embedded_path(a);
        if looks_absolute(candidate) && !is_inside(r, Path::new(candidate)) {
            return Err(violation(format!(
                "argument points outside the launcher folder: {a}"
            )));
        }
        if a.contains("..") && candidate.contains(['/', '\\']) {
            return Err(violation(format!(
                "argument contains a parent-directory reference: {a}"
            )));
        }
    }
    for (k, v) in &cmd.env {
        if FORBIDDEN_ENV.iter().any(|f| f.eq_ignore_ascii_case(k)) {
            return Err(violation(format!(
                "environment variable {k} could bypass the sandbox"
            )));
        }
        if looks_absolute(v) {
            let allowed = if SYSTEM_ENV_KEYS.iter().any(|s| s.eq_ignore_ascii_case(k)) {
                // PATH may hold only the system directory, with no extra entries.
                is_inside(&sys.system_root, Path::new(v)) && !v.contains(';')
            } else {
                is_inside(r, Path::new(v))
            };
            if !allowed {
                return Err(violation(format!(
                    "environment variable {k} points outside the launcher folder"
                )));
            }
        } else if SYSTEM_ENV_KEYS.iter().all(|s| !s.eq_ignore_ascii_case(k))
            && v.contains(['\\', '/'])
        {
            return Err(violation(format!(
                "environment variable {k} holds a relative or odd path"
            )));
        }
    }
    // The redirections that make the scrubbed environment meaningful must all be present.
    for needed in [
        "TEMP",
        "TMP",
        "USERPROFILE",
        "HOME",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if !cmd.env.contains_key(needed) {
            return Err(violation(format!(
                "environment variable {needed} must be redirected"
            )));
        }
    }
    for needed in ["-Djava.io.tmpdir=", "-Duser.home="] {
        if !cmd.args.iter().any(|a| a.starts_with(needed)) {
            return Err(violation(format!("missing JVM redirection {needed}")));
        }
    }
    Ok(())
}

impl InstallerCommand {
    /// Builds the process: environment cleared first, then only our explicit variables. On
    /// Windows no console window is created. The caller must have run [`verify_command`].
    pub fn to_process(&self) -> std::process::Command {
        // portability-allow: the single place installer processes are created; env is scrubbed below
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

// ---------------------------------------------------------------------------------------
// Tripwire: detect writes to the real global locations
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    exists: bool,
    entries: usize,
    total_len: u64,
    newest: Option<SystemTime>,
}

fn snapshot(path: &Path) -> Snapshot {
    let mut snap = Snapshot {
        exists: path.exists(),
        entries: 0,
        total_len: 0,
        newest: None,
    };
    if !snap.exists {
        return snap;
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        if snap.entries >= MAX_WALK_ENTRIES {
            break;
        }
        let Ok(md) = fs::symlink_metadata(&p) else {
            continue;
        };
        snap.entries += 1;
        snap.total_len += md.len();
        if let Ok(m) = md.modified() {
            snap.newest = Some(snap.newest.map_or(m, |n| n.max(m)));
        }
        if md.is_dir() {
            if let Ok(rd) = fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        }
    }
    snap
}

/// Watches directories the launcher must never write to.
#[derive(Debug)]
pub struct Tripwire {
    watched: Vec<(PathBuf, Snapshot)>,
}

impl Tripwire {
    pub fn arm(paths: &[PathBuf]) -> Self {
        Self {
            watched: paths.iter().map(|p| (p.clone(), snapshot(p))).collect(),
        }
    }

    /// The real global Minecraft directories on this PC. This is the only place the launcher
    /// reads machine-specific locations, and it only *observes* them.
    pub fn global_minecraft_dirs() -> Vec<PathBuf> {
        let mut v = Vec::new();
        // portability-allow: read-only tripwire; these paths are never written, stored or used
        for var in ["APPDATA", "USERPROFILE"] {
            if let Some(base) = std::env::var_os(var) {
                v.push(PathBuf::from(base).join(".minecraft"));
            }
        }
        v
    }

    /// Returns the watched paths that changed since `arm`.
    pub fn tripped(&self) -> Vec<PathBuf> {
        self.watched
            .iter()
            .filter(|(p, before)| snapshot(p) != *before)
            .map(|(p, _)| p.clone())
            .collect()
    }

    pub fn check(&self) -> Result<(), InstallError> {
        let changed = self.tripped();
        if changed.is_empty() {
            Ok(())
        } else {
            Err(InstallError::EscapedSandbox(changed))
        }
    }
}

// ---------------------------------------------------------------------------------------
// Merging the installer's output into the shared immutable stores
// ---------------------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub libraries_added: usize,
    pub files_already_present: usize,
    /// Version profiles that were new to the shared store.
    pub new_versions: Vec<VersionId>,
}

struct Planned {
    src: PathBuf,
    dst: PathBuf,
    present: bool,
}

fn walk_files(
    dir: &Path,
    base: &Path,
    out: &mut Vec<(RelPath, PathBuf)>,
) -> Result<(), InstallError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if is_link(&p)? {
            return Err(violation(format!(
                "installer produced a link: {}",
                p.display()
            )));
        }
        let ty = e.file_type()?;
        if ty.is_dir() {
            walk_files(&p, base, out)?;
        } else if ty.is_file() {
            let rel = p
                .strip_prefix(base)
                .map_err(|_| violation("file outside the staging area"))?;
            let parts: Vec<&str> = rel
                .components()
                .map(|c| {
                    c.as_os_str()
                        .to_str()
                        .ok_or_else(|| violation("non-Unicode file name from installer"))
                })
                .collect::<Result<_, _>>()?;
            let rel = RelPath::new(&parts.join("/"))
                .map_err(|e| violation(format!("unsafe path from installer: {e}")))?;
            out.push((rel, p));
        }
    }
    Ok(())
}

/// Moves `libraries/` and `versions/` from the stage into the shared stores. Two passes: all
/// paths and conflicts are validated first, so a bad installer result changes nothing.
pub fn merge_stage_into_shared(
    session: &StageSession,
    root: &PortableRoot,
) -> Result<MergeReport, InstallError> {
    let mc = session.minecraft_dir();
    let mut plan: Vec<Planned> = Vec::new();
    let mut new_versions: Vec<VersionId> = Vec::new();

    let mut libs = Vec::new();
    walk_files(&mc.join("libraries"), &mc.join("libraries"), &mut libs)?;
    for (rel, src) in libs {
        plan.push(Planned {
            dst: rel.to_path(&root.libraries_dir()),
            src,
            present: false,
        });
    }

    let mut vers = Vec::new();
    walk_files(&mc.join("versions"), &mc.join("versions"), &mut vers)?;
    let shared_versions = root.minecraft_dir().join("versions");
    for (rel, src) in vers {
        let first = rel.components().next().unwrap_or_default().to_owned();
        let id = VersionId::new(&first).map_err(|e| violation(e.to_string()))?;
        if !shared_versions.join(id.as_str()).exists() && !new_versions.contains(&id) {
            new_versions.push(id);
        }
        plan.push(Planned {
            dst: rel.to_path(&shared_versions),
            src,
            present: false,
        });
    }

    // Pass 1: nothing may be overwritten with different bytes, and no destination may sit behind a link.
    for p in &mut plan {
        crate::portable::fsutil::reject_links_between(root.path(), &p.dst)
            .map_err(|e| violation(e.to_string()))?;
        if p.dst.exists() {
            if sha256_hex(&p.src)? != sha256_hex(&p.dst)? {
                return Err(InstallError::Conflict(p.dst.display().to_string()));
            }
            p.present = true;
        }
    }

    // Pass 2: move into place (same volume: both are inside the root).
    let mut report = MergeReport {
        new_versions: new_versions.into_iter().collect(),
        ..Default::default()
    };
    for p in plan {
        if p.present {
            report.files_already_present += 1;
            continue;
        }
        if let Some(parent) = p.dst.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::rename(&p.src, &p.dst).is_err() {
            fs::copy(&p.src, &p.dst)?;
        }
        if p.dst.starts_with(root.libraries_dir()) {
            report.libraries_added += 1;
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------------------
// Libraries named by an untrusted profile (Fabric/Quilt need no installer)
// ---------------------------------------------------------------------------------------

fn coord_token_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// Where a library goes under `libraries/`, from its Maven coordinate
/// `group:artifact:version[:classifier][@ext]`, in the standard Maven layout. The coordinate
/// comes from downloaded JSON, so every part is validated and the result is a `RelPath`.
pub fn library_relpath(coordinate: &str) -> Result<RelPath, InstallError> {
    let bad = || InstallError::BadCoordinate(coordinate.to_owned());
    let (main, ext) = match coordinate.split_once('@') {
        Some((m, e)) => (m, e),
        None => (coordinate, "jar"),
    };
    let parts: Vec<&str> = main.split(':').collect();
    if !(3..=4).contains(&parts.len()) || !coord_token_ok(ext) {
        return Err(bad());
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    let classifier = parts.get(3).copied();
    let group_dirs: Vec<&str> = group.split('.').collect();
    if group_dirs
        .iter()
        .any(|g| !coord_token_ok(g) || g.contains('.'))
        || !coord_token_ok(artifact)
        || !coord_token_ok(version)
    {
        return Err(bad());
    }
    if let Some(c) = classifier {
        if !coord_token_ok(c) {
            return Err(bad());
        }
    }
    let file = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    let path = format!("{}/{artifact}/{version}/{file}", group_dirs.join("/"));
    RelPath::new(&path).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::root::DIRS;

    fn setup() -> (tempfile::TempDir, PortableRoot, ManagedJava, SystemEnv) {
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("Launcher"));
        root.ensure_layout().unwrap();
        let home = root.java_runtimes_dir().join("jre-test");
        fs::create_dir_all(home.join("bin")).unwrap();
        let exe = home.join("bin").join("java.exe");
        fs::write(&exe, b"MZ").unwrap();
        let java = ManagedJava {
            id: "jre-test".into(),
            java_major: 21,
            exe,
            home,
        };
        let sys = SystemEnv {
            system_root: PathBuf::from(r"C:\Windows"),
        };
        (d, root, java, sys)
    }

    fn build(
        root: &PortableRoot,
        java: &ManagedJava,
        sys: &SystemEnv,
        kind: LoaderKind,
    ) -> (StageSession, InstallerCommand) {
        let session = StageSession::create(root).unwrap();
        let jar = root.downloads_dir().join("forge-installer.jar");
        let cmd = build_installer_command(root, &session, java, &jar, kind, sys).unwrap();
        (session, cmd)
    }

    #[test]
    fn the_stage_lives_in_cache_with_a_stub_profile_and_everything_redirected() {
        let (_d, root, _j, _s) = setup();
        let s = StageSession::create(&root).unwrap();
        for p in [
            s.minecraft_dir(),
            s.home_dir(),
            s.appdata_dir(),
            s.localappdata_dir(),
            s.tmp_dir(),
        ] {
            assert!(p.is_dir());
            assert!(
                is_inside(&root.cache_dir().join("loader-install"), &p),
                "{}",
                p.display()
            );
        }
        assert!(s.minecraft_dir().join("launcher_profiles.json").is_file());
        // Two sessions never share a directory.
        assert_ne!(StageSession::create(&root).unwrap().base(), s.base());
    }

    #[test]
    fn forge_and_neoforge_commands_stay_entirely_inside_the_launcher_root() {
        let (_d, root, java, sys) = setup();
        for kind in [LoaderKind::Forge, LoaderKind::NeoForge] {
            let (session, cmd) = build(&root, &java, &sys, kind);
            assert!(
                is_inside(&root.java_runtimes_dir(), &cmd.program),
                "always the launcher-managed Java"
            );
            assert!(cmd
                .args
                .contains(&format!("-Djava.io.tmpdir={}", session.tmp_dir().display())));
            assert!(cmd
                .args
                .contains(&format!("-Duser.home={}", session.home_dir().display())));
            // The installer is explicitly pointed at the portable, stage-local Minecraft directory.
            assert_eq!(
                cmd.args.last().unwrap(),
                &session.minecraft_dir().display().to_string()
            );
            assert!(cmd.args.iter().any(|a| a == "--installClient"));
            assert!(!cmd.args.iter().any(|a| a.contains("--mirror")));
            // Every machine-specific location is redirected into the stage, not the system's.
            for key in [
                "TEMP",
                "TMP",
                "TMPDIR",
                "USERPROFILE",
                "HOME",
                "APPDATA",
                "LOCALAPPDATA",
            ] {
                assert!(
                    is_inside(session.base(), Path::new(&cmd.env[key])),
                    "{key}={}",
                    cmd.env[key]
                );
            }
            assert!(!cmd.env.keys().any(|k| FORBIDDEN_ENV.contains(&k.as_str())));
        }
    }

    #[test]
    fn fabric_and_quilt_never_get_an_installer_process() {
        let (_d, root, java, sys) = setup();
        let session = StageSession::create(&root).unwrap();
        for kind in [LoaderKind::Fabric, LoaderKind::Quilt] {
            let r = build_installer_command(&root, &session, &java, Path::new("x.jar"), kind, &sys);
            assert!(matches!(r, Err(InstallError::NoInstallerNeeded(_))));
        }
    }

    #[test]
    fn verify_command_catches_every_way_out_of_the_sandbox() {
        let (_d, root, java, sys) = setup();
        let (_s, good) = build(&root, &java, &sys, LoaderKind::Forge);
        assert!(verify_command(&good, &root, &sys).is_ok());

        let mutate = |f: &dyn Fn(&mut InstallerCommand)| {
            let mut c = good.clone();
            f(&mut c);
            verify_command(&c, &root, &sys).is_err()
        };
        // A system Java instead of the managed one.
        assert!(mutate(
            &|c| c.program = PathBuf::from(r"C:\Program Files\Java\bin\java.exe")
        ));
        // System TEMP, the global .minecraft, the real user profile, a stray PATH entry.
        assert!(mutate(&|c| {
            c.env
                .insert("TEMP".into(), r"C:\Users\someone\AppData\Local\Temp".into());
        }));
        assert!(mutate(&|c| {
            c.env
                .insert("APPDATA".into(), r"C:\Users\someone\AppData\Roaming".into());
        }));
        assert!(mutate(&|c| {
            c.env
                .insert("USERPROFILE".into(), r"C:\Users\someone".into());
        }));
        assert!(mutate(&|c| {
            c.env
                .insert("PATH".into(), r"C:\Windows\System32;C:\Tools".into());
        }));
        assert!(mutate(&|c| {
            c.env.insert("PATH".into(), r"C:\Tools".into());
        }));
        // Variables that inject JVM options or point at a system Java.
        for k in [
            "JAVA_HOME",
            "JAVA_TOOL_OPTIONS",
            "_JAVA_OPTIONS",
            "CLASSPATH",
        ] {
            assert!(
                mutate(&|c| {
                    c.env.insert(k.into(), "x".into());
                }),
                "{k}"
            );
        }
        // Targets outside the root, parent-directory tricks, mirrors, wrong working directory.
        assert!(mutate(&|c| *c.args.last_mut().unwrap() =
            r"C:\Users\someone\AppData\Roaming\.minecraft".into()));
        assert!(mutate(&|c| c.args.push(r"-Dsome.dir=D:\elsewhere".into())));
        assert!(mutate(&|c| c
            .args
            .push("--mirror=https://example.org/".into())));
        assert!(mutate(&|c| c.args.push("--mirror".into())));
        assert!(mutate(&|c| c.args.push("../../escape".into())));
        assert!(mutate(&|c| c.cwd = root.path().to_path_buf()));
        assert!(mutate(&|c| c.cwd = PathBuf::from(r"C:\Windows\System32")));
        // Dropping a redirection is also a failure.
        assert!(mutate(&|c| {
            c.env.remove("LOCALAPPDATA");
        }));
        assert!(mutate(&|c| c
            .args
            .retain(|a| !a.starts_with("-Duser.home="))));
        assert!(mutate(&|c| c
            .args
            .retain(|a| !a.starts_with("-Djava.io.tmpdir="))));
    }

    #[test]
    fn containment_check_is_lexical_case_insensitive_on_windows_and_rejects_dotdot() {
        let base = Path::new(r"F:\Launcher");
        assert!(is_inside(base, Path::new(r"F:\Launcher\cache\tmp")));
        assert!(
            !is_inside(base, Path::new(r"F:\LauncherEvil\x")),
            "prefix of a name is not containment"
        );
        assert!(!is_inside(base, Path::new(r"F:\Launcher\..\Other")));
        assert!(!is_inside(base, Path::new(r"E:\Launcher\x")));
        if cfg!(windows) {
            assert!(is_inside(base, Path::new(r"f:\launcher\CACHE")));
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_real_child_process_sees_only_the_scrubbed_environment() {
        let (_d, root, _j, sys) = setup();
        let session = StageSession::create(&root).unwrap();
        let system32 = sys.system_root.join("System32");
        let cmd = InstallerCommand {
            program: system32.join("cmd.exe"),
            args: vec!["/c".into(), "set".into()],
            cwd: session.base().to_path_buf(),
            env: BTreeMap::from([
                (
                    "SystemRoot".to_owned(),
                    sys.system_root.display().to_string(),
                ),
                (
                    "APPDATA".to_owned(),
                    session.appdata_dir().display().to_string(),
                ),
                ("TEMP".to_owned(), session.tmp_dir().display().to_string()),
            ]),
        };
        let out = cmd.to_process().output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        let appdata_line = text
            .lines()
            .find(|l| l.starts_with("appdata="))
            .unwrap_or_default();
        assert!(
            appdata_line.contains("loader-install"),
            "APPDATA was not redirected: {appdata_line}"
        );
        // Variables of the real user session are not inherited.
        for leaked in [
            "userprofile=",
            "localappdata=",
            "java_home=",
            "username=",
            "computername=",
        ] {
            assert!(
                !text.lines().any(|l| l.starts_with(leaked)),
                "{leaked} leaked into the child process"
            );
        }
    }

    #[test]
    fn tripwire_detects_writes_to_a_watched_directory_and_ignores_others() {
        let d = tempfile::tempdir().unwrap();
        let watched = d.path().join("fake-global").join(".minecraft");
        let other = d.path().join("elsewhere");
        fs::create_dir_all(&watched).unwrap();
        fs::write(watched.join("options.txt"), b"fov:70").unwrap();
        let trip = Tripwire::arm(&[
            watched.clone(),
            d.path().join("fake-global2").join(".minecraft"),
        ]);
        assert!(trip.check().is_ok());

        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("unrelated.txt"), b"x").unwrap();
        assert!(
            trip.check().is_ok(),
            "changes elsewhere are not our business"
        );

        fs::write(watched.join("launcher_profiles.json"), b"{}").unwrap();
        let err = trip.check().unwrap_err();
        assert!(
            matches!(err, InstallError::EscapedSandbox(ref v) if v == std::slice::from_ref(&watched))
        );

        // A directory that did not exist when armed and was created afterwards also trips it.
        let t2 = Tripwire::arm(&[d.path().join("new").join(".minecraft")]);
        fs::create_dir_all(d.path().join("new").join(".minecraft")).unwrap();
        assert!(t2.check().is_err());
    }

    fn stage_with_outputs(root: &PortableRoot) -> StageSession {
        let s = StageSession::create(root).unwrap();
        let mc = s.minecraft_dir();
        fs::create_dir_all(mc.join("libraries/net/minecraftforge/forge/1.21.4-54.1.14")).unwrap();
        fs::write(
            mc.join("libraries/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14.jar"),
            b"forge-jar",
        )
        .unwrap();
        fs::create_dir_all(mc.join("versions/1.21.4-forge-54.1.14")).unwrap();
        fs::write(
            mc.join("versions/1.21.4-forge-54.1.14/1.21.4-forge-54.1.14.json"),
            b"{\"id\":\"x\"}",
        )
        .unwrap();
        // Installer by-products that must never reach the shared stores.
        fs::write(mc.join("installer.log"), b"log").unwrap();
        fs::create_dir_all(mc.join(".cache")).unwrap();
        fs::write(mc.join(".cache/junk"), b"junk").unwrap();
        s
    }

    #[test]
    fn merge_takes_only_libraries_and_versions_into_the_shared_stores() {
        let (_d, root, _j, _s) = setup();
        let s = stage_with_outputs(&root);
        let r = merge_stage_into_shared(&s, &root).unwrap();
        assert_eq!(r.libraries_added, 1);
        assert_eq!(
            r.new_versions,
            vec![VersionId::new("1.21.4-forge-54.1.14").unwrap()]
        );
        assert_eq!(
            fs::read(
                root.libraries_dir()
                    .join("net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14.jar")
            )
            .unwrap(),
            b"forge-jar"
        );
        assert!(root
            .minecraft_dir()
            .join("versions/1.21.4-forge-54.1.14/1.21.4-forge-54.1.14.json")
            .is_file());
        // By-products and the stub profile were not merged anywhere in the root.
        assert!(
            !root.minecraft_dir().join("installer.log").exists()
                && !root.minecraft_dir().join(".cache").exists()
        );
        assert!(!root.minecraft_dir().join("launcher_profiles.json").exists());
        s.cleanup().unwrap();
    }

    #[test]
    fn merge_is_idempotent_and_never_overwrites_different_content() {
        let (_d, root, _j, _s) = setup();
        merge_stage_into_shared(&stage_with_outputs(&root), &root).unwrap();
        // Same content again: fine, nothing changes.
        let again = merge_stage_into_shared(&stage_with_outputs(&root), &root).unwrap();
        assert_eq!(
            (
                again.libraries_added,
                again.files_already_present,
                again.new_versions.len()
            ),
            (0, 2, 0)
        );

        // A different file at an existing path is a conflict, and nothing else is moved.
        let s = stage_with_outputs(&root);
        fs::write(
            s.minecraft_dir()
                .join("libraries/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14.jar"),
            b"TAMPERED",
        )
        .unwrap();
        fs::write(s.minecraft_dir().join("libraries/extra.jar"), b"extra").unwrap();
        assert!(matches!(
            merge_stage_into_shared(&s, &root),
            Err(InstallError::Conflict(_))
        ));
        assert!(
            !root.libraries_dir().join("extra.jar").exists(),
            "a failed merge must be all-or-nothing"
        );
    }

    #[test]
    fn merge_rejects_hostile_names_from_an_installer() {
        let (_d, root, _j, _s) = setup();
        let s = StageSession::create(&root).unwrap();
        // A version directory name that is not a valid version id.
        fs::create_dir_all(s.minecraft_dir().join("versions/bad;name")).unwrap();
        fs::write(s.minecraft_dir().join("versions/bad;name/x.json"), b"{}").unwrap();
        assert!(matches!(
            merge_stage_into_shared(&s, &root),
            Err(InstallError::Violation(_))
        ));
    }

    #[test]
    fn seeding_copies_the_vanilla_version_into_the_stage_only() {
        let (_d, root, _j, _s) = setup();
        let mc = McVersion::new("1.21.4").unwrap();
        let vdir = root.minecraft_dir().join("versions").join("1.21.4");
        fs::create_dir_all(&vdir).unwrap();
        fs::write(vdir.join("1.21.4.jar"), b"client").unwrap();
        fs::write(vdir.join("1.21.4.json"), b"{}").unwrap();
        let s = StageSession::create(&root).unwrap();
        s.seed_vanilla(&root, &mc).unwrap();
        assert_eq!(
            fs::read(s.minecraft_dir().join("versions/1.21.4/1.21.4.jar")).unwrap(),
            b"client"
        );
        // The seeded copy is identical, so merging it back is a no-op rather than a conflict.
        let r = merge_stage_into_shared(&s, &root).unwrap();
        assert_eq!((r.files_already_present, r.new_versions.len()), (2, 0));
    }

    #[test]
    fn stale_stages_from_a_crash_are_swept_and_nothing_else_is_touched() {
        let (_d, root, _j, _s) = setup();
        let a = StageSession::create(&root).unwrap();
        let _b = StageSession::create(&root).unwrap();
        fs::write(root.cache_dir().join("keep.txt"), b"keep").unwrap();
        assert_eq!(sweep_stale(&root).unwrap(), 2);
        assert!(!a.base().exists());
        assert!(root.cache_dir().join("keep.txt").exists());
        assert_eq!(sweep_stale(&root).unwrap(), 0);
    }

    #[test]
    fn library_paths_follow_the_maven_layout() {
        let p = |c: &str| library_relpath(c).unwrap().to_string();
        assert_eq!(
            p("net.fabricmc:fabric-loader:0.16.9"),
            "net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );
        assert_eq!(
            p("org.ow2.asm:asm:9.7.1"),
            "org/ow2/asm/asm/9.7.1/asm-9.7.1.jar"
        );
        assert_eq!(
            p("org.lwjgl:lwjgl:3.3.3:natives-windows"),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"
        );
        assert_eq!(
            p("org.example:thing:1.0@zip"),
            "org/example/thing/1.0/thing-1.0.zip"
        );
    }

    #[test]
    fn hostile_library_coordinates_from_a_profile_cannot_escape_the_libraries_folder() {
        for bad in [
            "../../evil:x:1",
            "net.fabricmc:../../x:1",
            "a:b:../../c",
            "a:b:1:../../x",
            "a..b:c:1",
            ".:c:1",
            "a:b",
            "a:b:c:d:e",
            "",
            "a:b:1@../x",
            "a b:c:1",
            "a:b:1/../../x",
            "C:\\evil:x:1",
            "a:b:1@jar;calc",
            "con:x:1",
        ] {
            assert!(library_relpath(bad).is_err(), "{bad:?} must be rejected");
        }
        // A well-formed coordinate always resolves to a path under libraries/.
        let (_d, root, _j, _s) = setup();
        let rel = library_relpath("net.fabricmc:fabric-loader:0.16.9").unwrap();
        assert!(is_inside(
            &root.libraries_dir(),
            &rel.to_path(&root.libraries_dir())
        ));
    }

    #[test]
    fn the_shared_layout_has_a_place_for_everything_the_installer_may_touch() {
        // Staging lives under cache/, shared output under libraries/ and minecraft/: all in DIRS.
        for needed in [
            "cache",
            "cache/tmp",
            "cache/downloads",
            "libraries",
            "minecraft",
        ] {
            assert!(DIRS.contains(&needed), "{needed}");
        }
    }
}
