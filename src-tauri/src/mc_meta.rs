//! Reading Mojang's version metadata: what a version needs on disk, decided only by the metadata.
//!
//! Nothing here touches the network or the disk. It turns a version JSON (or a loader's profile
//! JSON, which uses the same format) into a plan: the client jar, every library that applies to
//! Windows x64 (honouring Mojang's `rules`), which libraries hold native code to extract, the
//! asset index, the logging configuration and the Java runtime the version asks for. The JSON
//! comes from the network, so every path goes through `RelPath`, every hash is shape-checked and
//! unknown fields are ignored rather than guessed.
//!
//! Library forms handled (all seen in the official metadata, from `rd-132211` to the newest
//! snapshot):
//! - `downloads.artifact` (all modern and most historical libraries);
//! - the historical `natives` map + `downloads.classifiers` (e.g. `natives-windows`, with the
//!   `${arch}` placeholder) whose jar is *only* extracted, never put on the classpath;
//! - the modern separate `...:natives-windows` libraries (jar on the classpath as the official
//!   launcher does, and also extracted so a historical-style `java.library.path` finds them);
//! - loader profiles whose libraries carry only a Maven name, a base `url` and hashes;
//! - `url: ""` entries that an installer generates locally (Forge/NeoForge): they must exist,
//!   they cannot be downloaded.

use std::borrow::Cow;
use std::fmt;

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::loader_install::library_relpath;
use crate::portable::fsutil::Expected;
use crate::portable::RelPath;
use crate::versions::VersionId;

/// The platform libraries are selected for.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub os: &'static str,
    /// Bits: replaces `${arch}` in classifier names (`natives-windows-${arch}`).
    pub arch_bits: &'static str,
    /// Windows 10 and 11 both report 10.0.
    pub os_version: &'static str,
}

pub const WINDOWS_X64: Target = Target {
    os: "windows",
    arch_bits: "64",
    os_version: "10.0",
};

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("the version metadata is not valid JSON")]
    NotJson,
    #[error("the version metadata is missing {0}")]
    Missing(&'static str),
    #[error("the version metadata contains an unsafe or malformed {0}")]
    Bad(&'static str),
    #[error("this version cannot be prepared: {0}")]
    Unsupported(&'static str),
}

/// One file to place under a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// Where it goes, relative to its store (`libraries/` for libraries).
    pub path: RelPath,
    /// `None`: produced locally by an installer, so it can only be checked, not downloaded.
    pub url: Option<Url>,
    /// `None` only for loader libraries that publish no hash (a `.sha1` sidecar is used then).
    pub expected: Option<Expected>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Classpath,
    /// Extracted into the instance's natives folder; not on the classpath.
    Native {
        exclude: Vec<String>,
    },
    /// On the classpath and also extracted (the modern `natives-windows` libraries).
    ClasspathAndNative {
        exclude: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub name: String,
    pub artifact: Artifact,
    pub role: Role,
}

/// A download with no store path of its own (the client jar, the logging configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub url: Url,
    pub sha1: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIndexRef {
    pub id: String,
    pub download: Download,
    /// What the version JSON calls the assets (`legacy`, `pre-1.6`, `19`...).
    pub assets_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogConfig {
    /// File name under `assets/log_configs/`.
    pub file_name: String,
    pub download: Download,
    /// The JVM argument template that points the game at it (`-Dlog4j.configurationFile=${path}`).
    pub argument: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRequirement {
    /// Mojang's runtime component (`java-runtime-delta`, `jre-legacy`...).
    pub component: String,
    pub major: u32,
}

/// One entry of `arguments.jvm` / `arguments.game`: a plain string, or strings that apply only
/// when Mojang's rules allow them (operating system, architecture, launcher features).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Plain(String),
    Conditional { rules: Value, values: Vec<String> },
}

/// Launcher feature flags evaluated against Mojang `rules[].features` blocks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchFeatures {
    pub has_custom_resolution: bool,
}

impl Arg {
    /// The strings this entry contributes on `target` (launcher features are all off).
    pub fn values(&self, target: &Target) -> Vec<String> {
        self.values_with_features(target, &LaunchFeatures::default())
    }

    /// The strings this entry contributes on `target` with explicit launcher feature flags.
    pub fn values_with_features(&self, target: &Target, features: &LaunchFeatures) -> Vec<String> {
        match self {
            Arg::Plain(s) => vec![s.clone()],
            Arg::Conditional { rules, values }
                if rules_allow_features(Some(rules), target, features) =>
            {
                values.clone()
            }
            Arg::Conditional { .. } => Vec::new(),
        }
    }
}

/// The modern (1.13+) structured arguments. Older versions use `minecraft_arguments` instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Arguments {
    pub jvm: Vec<Arg>,
    pub game: Vec<Arg>,
}

fn parse_args(v: Option<&Value>) -> Result<Vec<Arg>, MetaError> {
    let mut out = Vec::new();
    for item in v.and_then(Value::as_array).into_iter().flatten() {
        match item {
            Value::String(s) => out.push(Arg::Plain(s.clone())),
            Value::Object(o) => {
                let values = match o.get("value") {
                    Some(Value::String(s)) => vec![s.clone()],
                    Some(Value::Array(a)) => a
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .ok_or(MetaError::Bad("argument"))
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err(MetaError::Bad("argument")),
                };
                out.push(Arg::Conditional {
                    rules: o.get("rules").cloned().unwrap_or(Value::Null),
                    values,
                });
            }
            _ => return Err(MetaError::Bad("argument")),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionPlan {
    pub id: String,
    pub inherits_from: Option<String>,
    pub main_class: Option<String>,
    /// Modern versions.
    pub arguments: Option<Arguments>,
    /// Versions before 1.13: one string of game arguments.
    pub minecraft_arguments: Option<String>,
    /// Mojang's `type` (`release`, `snapshot`, `old_beta`...), passed to the game as `${version_type}`.
    pub version_type: Option<String>,
    pub client: Option<Download>,
    pub libraries: Vec<Library>,
    pub asset_index: Option<AssetIndexRef>,
    pub logging: Option<LogConfig>,
    pub java: JavaRequirement,
}

impl VersionPlan {
    pub fn classpath(&self) -> impl Iterator<Item = &Library> {
        self.libraries
            .iter()
            .filter(|l| matches!(l.role, Role::Classpath | Role::ClasspathAndNative { .. }))
    }
    pub fn natives(&self) -> impl Iterator<Item = &Library> {
        self.libraries.iter().filter(|l| {
            matches!(
                l.role,
                Role::Native { .. } | Role::ClasspathAndNative { .. }
            )
        })
    }
}

// ------------------------------------------------------------------------------ rules

fn windows_version_matches(pattern: &str, version: &str) -> bool {
    // Only the literal-prefix patterns the official data uses ("^10\\.", "^10\\.0$") are understood.
    // Anything else is "does not match": a rule is never guessed to apply.
    let p = pattern.strip_prefix('^').unwrap_or(pattern);
    let (p, anchored_end) = match p.strip_suffix('$') {
        Some(x) => (x, true),
        None => (p, false),
    };
    let literal = p.replace("\\.", ".");
    if literal.chars().any(|c| !(c.is_ascii_digit() || c == '.')) {
        return false;
    }
    if anchored_end {
        version == literal
    } else {
        version.starts_with(&literal)
    }
}

fn rule_matches(rule: &Value, target: &Target, features: &LaunchFeatures) -> bool {
    if let Some(os) = rule.get("os") {
        if let Some(name) = os.get("name").and_then(Value::as_str) {
            if name != target.os {
                return false;
            }
        }
        if let Some(arch) = os.get("arch").and_then(Value::as_str) {
            if !matches!(arch, "x64" | "amd64" | "x86_64") {
                return false; // x86 and arm64 entries are not for this machine
            }
        }
        if let Some(v) = os.get("version").and_then(Value::as_str) {
            if !windows_version_matches(v, target.os_version) {
                return false;
            }
        }
    }
    if let Some(req_features) = rule.get("features").and_then(Value::as_object) {
        for (k, v) in req_features {
            let actual = match k.as_str() {
                "has_custom_resolution" => features.has_custom_resolution,
                _ => false,
            };
            if v.as_bool() != Some(actual) {
                return false;
            }
        }
    }
    true
}

/// Mojang's rule semantics: no rules means allowed; otherwise the last matching rule decides,
/// and if none matches the item is not allowed.
pub fn rules_allow(rules: Option<&Value>, target: &Target) -> bool {
    rules_allow_features(rules, target, &LaunchFeatures::default())
}

pub fn rules_allow_features(
    rules: Option<&Value>,
    target: &Target,
    features: &LaunchFeatures,
) -> bool {
    let Some(rules) = rules.and_then(Value::as_array).filter(|r| !r.is_empty()) else {
        return true;
    };
    let mut allowed = false;
    for r in rules {
        if rule_matches(r, target, features) {
            allowed = r.get("action").and_then(Value::as_str) == Some("allow");
        }
    }
    allowed
}

// ------------------------------------------------------------------------------ pieces

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn url_of(v: &Value, what: &'static str) -> Result<Url, MetaError> {
    let s = v.as_str().ok_or(MetaError::Missing(what))?;
    Url::parse(s).map_err(|_| MetaError::Bad(what))
}

fn download_of(v: &Value, what: &'static str) -> Result<Download, MetaError> {
    let sha1 = v
        .get("sha1")
        .and_then(Value::as_str)
        .filter(|s| is_hex(s, 40))
        .ok_or(MetaError::Bad(what))?;
    Ok(Download {
        url: url_of(v.get("url").ok_or(MetaError::Missing(what))?, what)?,
        sha1: sha1.to_ascii_lowercase(),
        size: v
            .get("size")
            .and_then(Value::as_u64)
            .ok_or(MetaError::Bad(what))?,
    })
}

fn expected_of(v: &Value) -> Option<Expected> {
    if let Some(h) = v
        .get("sha256")
        .and_then(Value::as_str)
        .filter(|s| is_hex(s, 64))
    {
        return Some(Expected::Sha256(h.to_ascii_lowercase()));
    }
    v.get("sha1")
        .and_then(Value::as_str)
        .filter(|s| is_hex(s, 40))
        .map(|h| Expected::Sha1(h.to_ascii_lowercase()))
}

/// `downloads.artifact`-style object: `{path, url, sha1, size}`.
fn artifact_of(v: &Value) -> Result<Artifact, MetaError> {
    let path = RelPath::new(
        v.get("path")
            .and_then(Value::as_str)
            .ok_or(MetaError::Missing("library path"))?,
    )
    .map_err(|_| MetaError::Bad("library path"))?;
    let url = match v.get("url").and_then(Value::as_str) {
        Some("") | None => None,
        Some(u) => Some(Url::parse(u).map_err(|_| MetaError::Bad("library url"))?),
    };
    Ok(Artifact {
        path,
        url,
        expected: expected_of(v),
        size: v.get("size").and_then(Value::as_u64),
    })
}

/// A library that only names a Maven coordinate (loader profiles, the oldest metadata).
fn artifact_from_coordinate(lib: &Value, name: &str) -> Result<Artifact, MetaError> {
    let path = library_relpath(name).map_err(|_| MetaError::Bad("library name"))?;
    let base = lib
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("https://libraries.minecraft.net/");
    let mut base = Url::parse(base).map_err(|_| MetaError::Bad("library url"))?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let url = base
        .join(path.as_str())
        .map_err(|_| MetaError::Bad("library url"))?;
    Ok(Artifact {
        path,
        url: Some(url),
        expected: expected_of(lib),
        size: lib.get("size").and_then(Value::as_u64),
    })
}

fn coordinate_classifier(name: &str) -> Option<&str> {
    let main = name.split('@').next().unwrap_or(name);
    main.split(':').nth(3)
}

fn is_target_native_classifier(classifier: &str, target: &Target) -> bool {
    target.os == "windows"
        && target.arch_bits == "64"
        && matches!(
            classifier,
            "natives-windows" | "natives-windows-64" | "natives-windows-x86_64"
        )
}

fn parse_library(lib: &Value, target: &Target) -> Result<Vec<Library>, MetaError> {
    let name = lib
        .get("name")
        .and_then(Value::as_str)
        .ok_or(MetaError::Missing("library name"))?;
    if !rules_allow(lib.get("rules"), target) {
        return Ok(Vec::new());
    }
    let modern_native = match coordinate_classifier(name) {
        Some(c) if c.to_ascii_lowercase().starts_with("natives-") => {
            let lower = c.to_ascii_lowercase();
            if is_target_native_classifier(&lower, target) {
                true
            } else {
                return Ok(Vec::new());
            }
        }
        _ => false,
    };
    let exclude: Vec<String> = lib
        .pointer("/extract/exclude")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let downloads = lib.get("downloads");
    let classifier = lib
        .get("natives")
        .and_then(|n| n.get(target.os))
        .and_then(Value::as_str)
        .map(|c| c.replace("${arch}", target.arch_bits));

    let mut out = Vec::new();
    let main = match downloads.and_then(|d| d.get("artifact")) {
        Some(a) => Some(artifact_of(a)?),
        None if downloads.and_then(|d| d.get("classifiers")).is_none() && classifier.is_none() => {
            Some(artifact_from_coordinate(lib, name)?)
        }
        None => None,
    };
    if let Some(artifact) = main {
        let role = if modern_native {
            Role::ClasspathAndNative {
                exclude: exclude.clone(),
            }
        } else {
            Role::Classpath
        };
        out.push(Library {
            name: name.to_owned(),
            artifact,
            role,
        });
    }
    if let Some(c) = classifier {
        let a = downloads
            .and_then(|d| d.get("classifiers"))
            .and_then(|cl| cl.get(&c))
            .ok_or(MetaError::Missing("native library for this platform"))?;
        out.push(Library {
            name: format!("{name}:{c}"),
            artifact: artifact_of(a)?,
            role: Role::Native { exclude },
        });
    }
    Ok(out)
}

fn single_component(s: &str, what: &'static str) -> Result<String, MetaError> {
    let rel = RelPath::new(s).map_err(|_| MetaError::Bad(what))?;
    if rel.components().count() != 1 {
        return Err(MetaError::Bad(what));
    }
    Ok(s.to_owned())
}

/// Parses a version JSON or loader profile into a plan.
pub fn parse_version(bytes: &[u8], target: &Target) -> Result<VersionPlan, MetaError> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| MetaError::NotJson)?;
    let raw_id = match v.get("id") {
        None | Some(Value::Null) => return Err(MetaError::Missing("id")),
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err(MetaError::Bad("id")),
    };
    let id = VersionId::new(raw_id)
        .map_err(|_| MetaError::Bad("id"))?
        .as_str()
        .to_owned();
    let inherits_from = match v.get("inheritsFrom") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(
            VersionId::new(s)
                .map_err(|_| MetaError::Bad("inheritsFrom"))?
                .as_str()
                .to_owned(),
        ),
        Some(_) => return Err(MetaError::Bad("inheritsFrom")),
    };

    let mut libraries = Vec::new();
    for lib in v
        .get("libraries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        libraries.extend(parse_library(lib, target)?);
    }

    let client = v
        .pointer("/downloads/client")
        .map(|c| download_of(c, "client download"))
        .transpose()?;

    let asset_index = match v.get("assetIndex") {
        Some(ai) => {
            let id = single_component(
                ai.get("id")
                    .and_then(Value::as_str)
                    .ok_or(MetaError::Missing("asset index id"))?,
                "asset index id",
            )?;
            Some(AssetIndexRef {
                assets_name: single_component(
                    v.get("assets").and_then(Value::as_str).unwrap_or(&id),
                    "assets name",
                )?,
                id,
                download: download_of(ai, "asset index")?,
            })
        }
        None => None,
    };

    let logging = match v.pointer("/logging/client/file") {
        Some(f) => Some(LogConfig {
            file_name: single_component(
                f.get("id")
                    .and_then(Value::as_str)
                    .ok_or(MetaError::Missing("logging id"))?,
                "logging id",
            )?,
            download: download_of(f, "logging configuration")?,
            argument: v
                .pointer("/logging/client/argument")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        None => None,
    };

    // Versions that predate the field run on the legacy Java 8 runtime.
    let java = match v.get("javaVersion") {
        Some(j) => {
            let component = j
                .get("component")
                .and_then(Value::as_str)
                .filter(|c| c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'))
                .ok_or(MetaError::Bad("java component"))?;
            JavaRequirement {
                component: component.to_owned(),
                major: j
                    .get("majorVersion")
                    .and_then(Value::as_u64)
                    .and_then(|m| u32::try_from(m).ok())
                    .ok_or(MetaError::Bad("java version"))?,
            }
        }
        None => JavaRequirement {
            component: "jre-legacy".into(),
            major: 8,
        },
    };

    let arguments = match v.get("arguments") {
        Some(a) => Some(Arguments {
            jvm: parse_args(a.get("jvm"))?,
            game: parse_args(a.get("game"))?,
        }),
        None => None,
    };
    let main_class = v
        .get("mainClass")
        .and_then(Value::as_str)
        .filter(|m| {
            !m.is_empty()
                && m.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '$'))
        })
        .map(str::to_owned);
    if v.get("mainClass").is_some() && main_class.is_none() {
        return Err(MetaError::Bad("main class"));
    }

    Ok(VersionPlan {
        id,
        inherits_from,
        main_class,
        arguments,
        minecraft_arguments: v
            .get("minecraftArguments")
            .and_then(Value::as_str)
            .map(str::to_owned),
        version_type: v.get("type").and_then(Value::as_str).map(str::to_owned),
        client,
        libraries,
        asset_index,
        logging,
        java,
    })
}

// ------------------------------------------------------------------------------ assets

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetObject {
    pub name: String,
    /// Lowercase SHA-1; also the object's file name.
    pub hash: String,
    pub size: u64,
}

impl AssetObject {
    /// `ab/abcdef...` under `assets/objects/`.
    pub fn store_path(&self) -> String {
        format!("{}/{}", &self.hash[..2], self.hash)
    }
    /// The official asset CDN.
    pub fn url(&self) -> Result<Url, MetaError> {
        Url::parse(&format!(
            "https://resources.download.minecraft.net/{}",
            self.store_path()
        ))
        .map_err(|_| MetaError::Bad("asset hash"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetIndex {
    pub objects: Vec<AssetObject>,
    /// Pre-1.7 layout: also needed as plain files under `assets/virtual/<id>/<name>`.
    pub virtual_files: bool,
    /// Pre-1.6 layout: also needed as plain files under the game directory's `resources/`.
    pub map_to_resources: bool,
}

enum HashField {
    Valid(String),
    Invalid,
}

impl<'de> Deserialize<'de> for HashField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HashVisitor;
        impl<'de> Visitor<'de> for HashVisitor {
            type Value = HashField;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a 40-character hex asset hash")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if is_hex(v, 40) {
                    Ok(HashField::Valid(v.to_ascii_lowercase()))
                } else {
                    Ok(HashField::Invalid)
                }
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(HashField::Invalid)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(HashField::Invalid)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(HashField::Invalid)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(HashField::Invalid)
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(HashField::Invalid)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(HashField::Invalid)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(HashField::Invalid)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(HashField::Invalid)
            }
        }
        deserializer.deserialize_any(HashVisitor)
    }
}

enum SizeField {
    Valid(u64),
    Invalid,
}

impl<'de> Deserialize<'de> for SizeField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SizeVisitor;
        impl<'de> Visitor<'de> for SizeVisitor {
            type Value = SizeField;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a non-negative asset size")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(SizeField::Valid(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(u64::try_from(v).map_or(SizeField::Invalid, SizeField::Valid))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(SizeField::Invalid)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(SizeField::Invalid)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(SizeField::Invalid)
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(SizeField::Invalid)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(SizeField::Invalid)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(SizeField::Invalid)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(SizeField::Invalid)
            }
        }
        deserializer.deserialize_any(SizeVisitor)
    }
}

struct BoolOrFalse(bool);

impl<'de> Deserialize<'de> for BoolOrFalse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoolVisitor;
        impl<'de> Visitor<'de> for BoolVisitor {
            type Value = BoolOrFalse;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a boolean flag")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(BoolOrFalse(v))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(BoolOrFalse(false))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(BoolOrFalse(false))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(BoolOrFalse(false))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(BoolOrFalse(false))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(BoolOrFalse(false))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(BoolOrFalse(false))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(BoolOrFalse(false))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(BoolOrFalse(false))
            }
        }
        deserializer.deserialize_any(BoolVisitor)
    }
}

struct AssetEntrySeed<'a> {
    name: String,
    err: &'a mut Option<MetaError>,
}

impl<'de> DeserializeSeed<'de> for AssetEntrySeed<'_> {
    type Value = AssetObject;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for AssetEntrySeed<'_> {
    type Value = AssetObject;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an asset object entry")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        *self.err = Some(MetaError::Bad("asset hash"));
        Err(de::Error::custom("asset hash"))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut hash: Option<HashField> = None;
        let mut size: Option<SizeField> = None;
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.as_ref() {
                "hash" => hash = Some(map.next_value::<HashField>()?),
                "size" => size = Some(map.next_value::<SizeField>()?),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let Some(HashField::Valid(hash)) = hash else {
            *self.err = Some(MetaError::Bad("asset hash"));
            return Err(de::Error::custom("asset hash"));
        };
        let Some(SizeField::Valid(size)) = size else {
            *self.err = Some(MetaError::Bad("asset size"));
            return Err(de::Error::custom("asset size"));
        };
        Ok(AssetObject {
            name: self.name,
            hash,
            size,
        })
    }
}

struct ObjectsMapSeed<'a>(&'a mut Option<MetaError>);

impl<'de> DeserializeSeed<'de> for ObjectsMapSeed<'_> {
    type Value = Option<Vec<AssetObject>>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ObjectsMapSeed<'_> {
    type Value = Option<Vec<AssetObject>>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an objects map")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::with_capacity(map.size_hint().unwrap_or(0).min(65_536));
        while let Some(name) = map.next_key::<String>()? {
            let entry = map.next_value_seed(AssetEntrySeed { name, err: self.0 })?;
            out.push(entry);
        }
        Ok(Some(out))
    }
}

struct AssetIndexSeed<'a>(&'a mut Option<MetaError>);

impl<'de> DeserializeSeed<'de> for AssetIndexSeed<'_> {
    type Value = Option<AssetIndex>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for AssetIndexSeed<'_> {
    type Value = Option<AssetIndex>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an asset index JSON object")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut objects: Option<Vec<AssetObject>> = None;
        let mut virtual_files = false;
        let mut map_to_resources = false;
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.as_ref() {
                "objects" => {
                    objects = map.next_value_seed(ObjectsMapSeed(self.0))?;
                }
                "virtual" => {
                    virtual_files = map.next_value::<BoolOrFalse>()?.0;
                }
                "map_to_resources" => {
                    map_to_resources = map.next_value::<BoolOrFalse>()?.0;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(objects.map(|objects| AssetIndex {
            objects,
            virtual_files,
            map_to_resources,
        }))
    }
}

pub fn parse_asset_index(bytes: &[u8]) -> Result<AssetIndex, MetaError> {
    let mut meta_err: Option<MetaError> = None;
    let mut de = serde_json::Deserializer::from_slice(bytes);
    match AssetIndexSeed(&mut meta_err).deserialize(&mut de) {
        Ok(maybe_index) => {
            de.end().map_err(|_| MetaError::NotJson)?;
            maybe_index.ok_or(MetaError::Missing("asset objects"))
        }
        Err(e) if e.is_syntax() || e.is_eof() || e.is_io() => Err(MetaError::NotJson),
        Err(_) => Err(meta_err.unwrap_or(MetaError::NotJson)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const H: &str = "0123456789abcdef0123456789abcdef01234567";

    fn art(path: &str) -> Value {
        json!({"path": path, "sha1": H, "size": 10, "url": format!("https://libraries.minecraft.net/{path}")})
    }

    fn version(libs: Value) -> Vec<u8> {
        json!({
            "id": "t", "mainClass": "x", "assets": "19",
            "assetIndex": {"id": "19", "sha1": H, "size": 5, "totalSize": 99, "url": "https://piston-meta.mojang.com/v1/packages/x/19.json"},
            "downloads": {"client": {"sha1": H, "size": 7, "url": "https://piston-data.mojang.com/v1/objects/x/client.jar"}},
            "logging": {"client": {"argument": "-Dx=${path}", "type": "log4j2-xml", "file": {"id": "client-1.21.2.xml", "sha1": H, "size": 3, "url": "https://piston-data.mojang.com/v1/objects/x/client-1.21.2.xml"}}},
            "javaVersion": {"component": "java-runtime-delta", "majorVersion": 21},
            "libraries": libs,
        })
        .to_string()
        .into_bytes()
    }

    fn names(plan: &VersionPlan) -> Vec<&str> {
        plan.libraries.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn a_modern_version_yields_everything_it_needs_and_nothing_for_other_platforms() {
        let plan = parse_version(
            &version(json!([
                {"name": "com.google.code.gson:gson:2.10.1", "downloads": {"artifact": art("com/google/code/gson/gson/2.10.1/gson-2.10.1.jar")}},
                {"name": "org.lwjgl:lwjgl:3.3.3", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar")}},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-windows", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar")}, "rules": [{"action": "allow", "os": {"name": "windows"}}]},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-windows-arm64", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows-arm64.jar")}, "rules": [{"action": "allow", "os": {"name": "windows", "arch": "arm64"}}]},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-windows-x86", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows-x86.jar")}, "rules": [{"action": "allow", "os": {"name": "windows", "arch": "x86"}}]},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-linux", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar")}, "rules": [{"action": "allow", "os": {"name": "linux"}}]},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-macos", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-macos.jar")}, "rules": [{"action": "allow", "os": {"name": "osx"}}]},
            ])),
            &WINDOWS_X64,
        )
        .unwrap();
        assert_eq!(
            names(&plan),
            [
                "com.google.code.gson:gson:2.10.1",
                "org.lwjgl:lwjgl:3.3.3",
                "org.lwjgl:lwjgl:3.3.3:natives-windows"
            ]
        );
        assert_eq!(plan.classpath().count(), 3);
        let natives: Vec<_> = plan.natives().map(|l| l.name.as_str()).collect();
        assert_eq!(natives, ["org.lwjgl:lwjgl:3.3.3:natives-windows"]);
        assert_eq!(plan.client.as_ref().unwrap().size, 7);
        assert_eq!(plan.asset_index.as_ref().unwrap().id, "19");
        assert_eq!(
            plan.logging.as_ref().unwrap().file_name,
            "client-1.21.2.xml"
        );
        assert_eq!(
            plan.java,
            JavaRequirement {
                component: "java-runtime-delta".into(),
                major: 21
            }
        );
    }

    #[test]
    fn historical_natives_come_from_the_classifier_map_and_are_not_on_the_classpath() {
        let plan = parse_version(
            &version(json!([{
                "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
                "natives": {"linux": "natives-linux", "windows": "natives-windows-${arch}", "osx": "natives-osx"},
                "extract": {"exclude": ["META-INF/"]},
                "downloads": {
                    "artifact": art("org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4.jar"),
                    "classifiers": {
                        "natives-windows-64": art("org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-windows-64.jar"),
                        "natives-linux": art("org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-linux.jar"),
                    }
                }
            }])),
            &WINDOWS_X64,
        )
        .unwrap();
        assert_eq!(plan.libraries.len(), 2);
        assert_eq!(plan.libraries[0].role, Role::Classpath);
        assert_eq!(
            plan.libraries[1].role,
            Role::Native {
                exclude: vec!["META-INF/".into()]
            }
        );
        assert!(plan.libraries[1]
            .artifact
            .path
            .as_str()
            .ends_with("natives-windows-64.jar"));
        assert_eq!(plan.classpath().count(), 1);
        // A native listed for Windows whose classifier jar is absent is an error, never a guess.
        let broken = version(
            json!([{"name": "a:b:1", "natives": {"windows": "natives-windows"}, "downloads": {"classifiers": {}}}]),
        );
        assert!(matches!(
            parse_version(&broken, &WINDOWS_X64),
            Err(MetaError::Missing(_))
        ));
    }

    #[test]
    fn rules_follow_mojangs_semantics() {
        let t = &WINDOWS_X64;
        let r = |v: Value| rules_allow(Some(&v), t);
        assert!(rules_allow(None, t));
        assert!(r(json!([])));
        assert!(r(json!([{"action": "allow"}])));
        assert!(r(
            json!([{"action": "allow"}, {"action": "disallow", "os": {"name": "osx"}}])
        ));
        assert!(!r(
            json!([{"action": "allow"}, {"action": "disallow", "os": {"name": "windows"}}])
        ));
        assert!(
            !r(json!([{"action": "allow", "os": {"name": "linux"}}])),
            "no rule matched"
        );
        assert!(r(
            json!([{"action": "allow", "os": {"name": "windows", "arch": "x64"}}])
        ));
        assert!(!r(
            json!([{"action": "allow", "os": {"name": "windows", "arch": "x86"}}])
        ));
        // Windows version patterns: the literal-prefix form is understood, anything else never matches.
        assert!(r(
            json!([{"action": "allow", "os": {"name": "windows", "version": "^10\\."}}])
        ));
        assert!(!r(
            json!([{"action": "allow", "os": {"name": "windows", "version": "^6\\.1"}}])
        ));
        assert!(!r(
            json!([{"action": "allow", "os": {"name": "windows", "version": "(10|11).*"}}])
        ));
        // Launcher features are all off.
        assert!(!r(
            json!([{"action": "allow", "features": {"is_demo_user": true}}])
        ));
        assert!(r(
            json!([{"action": "allow", "features": {"is_demo_user": false}}])
        ));
    }

    #[test]
    fn libraries_that_only_name_a_maven_coordinate_are_resolved_from_it() {
        let plan = parse_version(
            br#"{"id":"fabric-loader-0.16.9-1.21.4","inheritsFrom":"1.21.4","libraries":[
              {"name":"net.fabricmc:fabric-loader:0.16.9","url":"https://maven.fabricmc.net/","sha1":"0123456789abcdef0123456789abcdef01234567","size":123},
              {"name":"net.fabricmc:intermediary:1.21.4","url":"https://maven.fabricmc.net"},
              {"name":"org.ow2.asm:asm:9.7"}]}"#,
            &WINDOWS_X64,
        )
        .unwrap();
        assert_eq!(plan.inherits_from.as_deref(), Some("1.21.4"));
        assert!(plan.client.is_none() && plan.asset_index.is_none());
        let a = &plan.libraries[0].artifact;
        assert_eq!(
            a.path.as_str(),
            "net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );
        assert_eq!(
            a.url.as_ref().unwrap().as_str(),
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );
        assert_eq!(a.expected, Some(Expected::Sha1(H.into())));
        assert_eq!(a.size, Some(123));
        let b = &plan.libraries[1].artifact;
        assert!(b
            .url
            .as_ref()
            .unwrap()
            .as_str()
            .starts_with("https://maven.fabricmc.net/net/fabricmc/intermediary/"));
        assert_eq!(
            b.expected, None,
            "no hash published: a sidecar will be used"
        );
        assert!(plan.libraries[2]
            .artifact
            .url
            .as_ref()
            .unwrap()
            .as_str()
            .starts_with("https://libraries.minecraft.net/org/ow2/asm/asm/9.7/"));
    }

    #[test]
    fn installer_generated_libraries_have_no_url_and_must_exist_locally() {
        let plan = parse_version(
            &version(json!([{"name": "net.minecraftforge:forge:1.20.1-47.2.0:client", "downloads": {"artifact": {"path": "net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-client.jar", "url": "", "sha1": H, "size": 9}}}])),
            &WINDOWS_X64,
        )
        .unwrap();
        assert!(plan.libraries[0].artifact.url.is_none());
    }

    #[test]
    fn versions_without_a_java_field_use_the_legacy_runtime() {
        let v = json!({"id": "1.6.4", "downloads": {"client": {"sha1": H, "size": 1, "url": "https://piston-data.mojang.com/c.jar"}}, "libraries": []}).to_string();
        let plan = parse_version(v.as_bytes(), &WINDOWS_X64).unwrap();
        assert_eq!(
            plan.java,
            JavaRequirement {
                component: "jre-legacy".into(),
                major: 8
            }
        );
        assert!(plan.logging.is_none() && plan.asset_index.is_none());
    }

    #[test]
    fn hostile_metadata_is_refused() {
        let bad = |libs: Value| parse_version(&version(libs), &WINDOWS_X64).is_err();
        assert!(bad(
            json!([{"name": "a:b:1", "downloads": {"artifact": {"path": "../../evil.jar", "sha1": H, "size": 1, "url": "https://libraries.minecraft.net/x"}}}])
        ));
        assert!(bad(
            json!([{"name": "a:b:1", "downloads": {"artifact": {"path": "C:/evil.jar", "sha1": H, "size": 1, "url": "https://libraries.minecraft.net/x"}}}])
        ));
        assert!(bad(json!([{"name": "../a:b:1"}])));
        assert!(parse_version(b"not json", &WINDOWS_X64).is_err());
        let bad_index = json!({"id": "x", "assetIndex": {"id": "../x", "sha1": H, "size": 1, "url": "https://piston-meta.mojang.com/x"}}).to_string();
        assert!(parse_version(bad_index.as_bytes(), &WINDOWS_X64).is_err());
        let bad_hash = json!({"id": "x", "downloads": {"client": {"sha1": "zz", "size": 1, "url": "https://piston-data.mojang.com/x"}}}).to_string();
        assert!(parse_version(bad_hash.as_bytes(), &WINDOWS_X64).is_err());
    }

    #[test]
    fn modern_arguments_legacy_arguments_and_the_main_class_are_read() {
        let modern = json!({
            "id": "m", "type": "release", "mainClass": "net.minecraft.client.main.Main",
            "arguments": {
                "jvm": [
                    {"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": ["-XstartOnFirstThread"]},
                    {"rules": [{"action": "allow", "os": {"name": "windows"}}], "value": "-XX:HeapDumpPath=x"},
                    {"rules": [{"action": "allow", "os": {"arch": "x86"}}], "value": "-Xss1M"},
                    "-Djava.library.path=${natives_directory}", "-cp", "${classpath}"],
                "game": ["--username", "${auth_player_name}",
                    {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"},
                    {"rules": [{"action": "allow", "features": {"has_custom_resolution": true}}], "value": ["--width", "${resolution_width}"]}]
            }
        })
        .to_string();
        let plan = parse_version(modern.as_bytes(), &WINDOWS_X64).unwrap();
        assert_eq!(
            plan.main_class.as_deref(),
            Some("net.minecraft.client.main.Main")
        );
        assert_eq!(plan.version_type.as_deref(), Some("release"));
        let a = plan.arguments.unwrap();
        let jvm: Vec<String> = a.jvm.iter().flat_map(|x| x.values(&WINDOWS_X64)).collect();
        assert_eq!(
            jvm,
            [
                "-XX:HeapDumpPath=x",
                "-Djava.library.path=${natives_directory}",
                "-cp",
                "${classpath}"
            ]
        );
        let game: Vec<String> = a.game.iter().flat_map(|x| x.values(&WINDOWS_X64)).collect();
        assert_eq!(
            game,
            ["--username", "${auth_player_name}"],
            "demo mode and custom resolution stay off"
        );

        let legacy = br#"{"id":"1.12.2","mainClass":"net.minecraft.client.main.Main","minecraftArguments":"--username ${auth_player_name} --version ${version_name}","libraries":[]}"#;
        let plan = parse_version(legacy, &WINDOWS_X64).unwrap();
        assert!(plan.arguments.is_none());
        assert_eq!(
            plan.minecraft_arguments.as_deref(),
            Some("--username ${auth_player_name} --version ${version_name}")
        );

        assert!(parse_version(br#"{"id":"x","mainClass":"a b; rm -rf"}"#, &WINDOWS_X64).is_err());
        assert!(parse_version(br#"{"id":"x","arguments":{"jvm":[5]}}"#, &WINDOWS_X64).is_err());
    }

    #[test]
    fn asset_indexes_cover_the_modern_virtual_and_pre_1_6_layouts() {
        let idx = parse_asset_index(
            format!(r#"{{"objects":{{"icons/a.png":{{"hash":"{H}","size":4}},"minecraft/lang/x.json":{{"hash":"{H}","size":2}}}}}}"#).as_bytes(),
        )
        .unwrap();
        assert_eq!(idx.objects.len(), 2);
        assert!(!idx.virtual_files && !idx.map_to_resources);
        assert_eq!(idx.objects[0].store_path(), format!("01/{H}"));
        assert_eq!(
            idx.objects[0].url().unwrap().as_str(),
            format!("https://resources.download.minecraft.net/01/{H}")
        );
        let legacy = parse_asset_index(
            format!(r#"{{"virtual":true,"objects":{{"a":{{"hash":"{H}","size":1}}}}}}"#).as_bytes(),
        )
        .unwrap();
        assert!(legacy.virtual_files);
        let pre = parse_asset_index(
            format!(r#"{{"map_to_resources":true,"objects":{{"a":{{"hash":"{H}","size":1}}}}}}"#)
                .as_bytes(),
        )
        .unwrap();
        assert!(pre.map_to_resources);
        assert!(parse_asset_index(br#"{"objects":{"a":{"hash":"short","size":1}}}"#).is_err());
        assert!(parse_asset_index(b"{}").is_err());
    }

    #[test]
    fn validates_version_id_and_inherits_from_tokens() {
        assert!(matches!(
            parse_version(b"{}", &WINDOWS_X64),
            Err(MetaError::Missing("id"))
        ));
        assert!(matches!(
            parse_version(br#"{"id":null}"#, &WINDOWS_X64),
            Err(MetaError::Missing("id"))
        ));
        for bad_id in [
            json!(""),
            json!("../evil"),
            json!("-flag"),
            json!(".hidden"),
            json!("a/b"),
            json!("9".repeat(129)),
            json!(123),
        ] {
            let doc = json!({"id": bad_id, "libraries": []}).to_string();
            assert!(
                matches!(
                    parse_version(doc.as_bytes(), &WINDOWS_X64),
                    Err(MetaError::Bad("id"))
                ),
                "bad id not rejected: {doc}"
            );
        }

        let null_parent = parse_version(
            br#"{"id":"1.21.4","inheritsFrom":null,"libraries":[]}"#,
            &WINDOWS_X64,
        )
        .unwrap();
        assert_eq!(null_parent.inherits_from, None);

        let long_id = "f".repeat(100);
        let ok_composite = json!({
            "id": long_id,
            "inheritsFrom": "1.21.4",
            "libraries": []
        })
        .to_string();
        let plan = parse_version(ok_composite.as_bytes(), &WINDOWS_X64).unwrap();
        assert_eq!(plan.id.len(), 100);
        assert_eq!(plan.inherits_from.as_deref(), Some("1.21.4"));

        for bad_parent in [
            json!(""),
            json!("../evil"),
            json!("-flag"),
            json!(".hidden"),
            json!("9".repeat(129)),
            json!(42),
            json!(["1.21.4"]),
        ] {
            let doc =
                json!({"id": "1.21.4", "inheritsFrom": bad_parent, "libraries": []}).to_string();
            assert!(
                matches!(
                    parse_version(doc.as_bytes(), &WINDOWS_X64),
                    Err(MetaError::Bad("inheritsFrom"))
                ),
                "bad inheritsFrom not rejected: {doc}"
            );
        }
    }

    #[test]
    fn filters_foreign_native_classifiers_even_when_rules_omit_os_arch() {
        let win_only_rule = json!([{"action": "allow", "os": {"name": "windows"}}]);
        let plan = parse_version(
            &version(json!([
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-windows", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows.jar")}, "rules": win_only_rule},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-windows-64", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows-64.jar")}, "rules": win_only_rule},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-windows-x86_64", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows-x86_64.jar")}},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-windows-x86", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows-x86.jar")}, "rules": win_only_rule},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-windows-arm64", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows-arm64.jar")}, "rules": win_only_rule},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-linux", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-linux.jar")}},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-macos", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-macos.jar")}},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-macos-arm64", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-macos-arm64.jar")}},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-osx", "downloads": {"artifact": art("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-osx.jar")}},
            ])),
            &WINDOWS_X64,
        )
        .unwrap();

        assert_eq!(
            names(&plan),
            [
                "org.lwjgl:lwjgl:3.3.1:natives-windows",
                "org.lwjgl:lwjgl:3.3.1:natives-windows-64",
                "org.lwjgl:lwjgl:3.3.1:natives-windows-x86_64",
            ]
        );
        assert!(plan
            .libraries
            .iter()
            .all(|l| matches!(l.role, Role::ClasspathAndNative { .. })));
    }

    #[test]
    fn streaming_asset_index_validates_hashes_sizes_and_syntax() {
        let upper = H.to_ascii_uppercase();
        let mut entries = String::new();
        for i in 0..200 {
            if i > 0 {
                entries.push(',');
            }
            entries.push_str(&format!(
                r#""sounds/ambient_{i}.ogg":{{"extra":true,"size":{},"hash":"{upper}"}}"#,
                100 + i
            ));
        }
        let raw = format!(
            r#"{{"unknown":[1,2,3],"objects":{{{entries}}},"virtual":true,"map_to_resources":false}}"#
        );
        let parsed = parse_asset_index(raw.as_bytes()).unwrap();
        assert_eq!(parsed.objects.len(), 200);
        assert!(parsed.virtual_files);
        assert!(!parsed.map_to_resources);
        assert_eq!(parsed.objects[0].hash, H);
        assert_eq!(parsed.objects[0].size, 100);
        assert_eq!(parsed.objects[199].size, 299);

        // Syntax / EOF / trailing bytes -> NotJson
        for bad_json in [
            &b""[..],
            b"not json",
            b"{\"objects\": {\"a\": ",
            b"{\"objects\": {}} trailing",
        ] {
            assert!(
                matches!(parse_asset_index(bad_json), Err(MetaError::NotJson)),
                "{bad_json:?}"
            );
        }

        // Missing or non-map objects -> Missing("asset objects")
        for missing in [
            &b"{}"[..],
            b"[]",
            b"{\"objects\":null}",
            b"{\"objects\":[]}",
        ] {
            assert!(
                matches!(
                    parse_asset_index(missing),
                    Err(MetaError::Missing("asset objects"))
                ),
                "{missing:?}"
            );
        }

        // Invalid hash -> Bad("asset hash")
        for bad_hash in [
            r#"{"objects":{"a":{"hash":"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz","size":1}}}"#
                .to_owned(),
            r#"{"objects":{"a":{"hash":123,"size":1}}}"#.to_owned(),
            r#"{"objects":{"a":{"size":1}}}"#.to_owned(),
        ] {
            assert!(
                matches!(
                    parse_asset_index(bad_hash.as_bytes()),
                    Err(MetaError::Bad("asset hash"))
                ),
                "{bad_hash}"
            );
        }

        // Invalid size -> Bad("asset size")
        for bad_size in [
            format!(r#"{{"objects":{{"a":{{"hash":"{H}"}}}}}}"#),
            format!(r#"{{"objects":{{"a":{{"hash":"{H}","size":-1}}}}}}"#),
            format!(r#"{{"objects":{{"a":{{"hash":"{H}","size":"5"}}}}}}"#),
        ] {
            assert!(
                matches!(
                    parse_asset_index(bad_size.as_bytes()),
                    Err(MetaError::Bad("asset size"))
                ),
                "{bad_size}"
            );
        }
    }
}
