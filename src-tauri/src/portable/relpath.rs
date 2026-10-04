//! `RelPath`: the only way a path is persisted or accepted from untrusted input.
//!
//! A `RelPath` is always relative to the launcher root, uses `/` separators, and can
//! never express anything outside it. Validation is deliberately stricter than the
//! host OS so a folder created on a USB stick behaves the same on every filesystem
//! (NTFS, exFAT, FAT32) and every PC.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const MAX_COMPONENT_LEN: usize = 200;
const MAX_TOTAL_LEN: usize = 240;

const RESERVED_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    "conin$", "conout$",
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RelPathError {
    #[error("path is empty")]
    Empty,
    #[error("path is too long")]
    TooLong,
    #[error("absolute paths, drive letters and UNC paths are not allowed: {0:?}")]
    Absolute(String),
    #[error("backslashes are not allowed, use '/': {0:?}")]
    Backslash(String),
    #[error("'.', '..' and empty components are not allowed: {0:?}")]
    Traversal(String),
    #[error("illegal character in path component: {0:?}")]
    IllegalChar(String),
    #[error("reserved device name or trailing dot/space in component: {0:?}")]
    ReservedName(String),
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelPath(String);

impl RelPath {
    pub fn new(input: &str) -> Result<Self, RelPathError> {
        if input.is_empty() {
            return Err(RelPathError::Empty);
        }
        if input.len() > MAX_TOTAL_LEN {
            return Err(RelPathError::TooLong);
        }
        if input.contains('\\') {
            return Err(RelPathError::Backslash(input.to_owned()));
        }
        if input.starts_with('/') {
            return Err(RelPathError::Absolute(input.to_owned()));
        }
        // ':' covers drive letters ("C:") and NTFS alternate data streams ("file:stream").
        if input.contains(':') {
            return Err(RelPathError::Absolute(input.to_owned()));
        }
        for comp in input.split('/') {
            validate_component(comp, input)?;
        }
        Ok(Self(input.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, more: &str) -> Result<Self, RelPathError> {
        Self::new(&format!("{}/{}", self.0, more))
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Lower-cased form, for detecting names that collide on case-insensitive filesystems.
    pub fn fold_case(&self) -> String {
        self.0.to_lowercase()
    }

    /// Joins onto an absolute base using the platform separator.
    pub fn to_path(&self, base: &Path) -> PathBuf {
        let mut p = base.to_path_buf();
        for c in self.components() {
            p.push(c);
        }
        p
    }
}

fn validate_component(comp: &str, whole: &str) -> Result<(), RelPathError> {
    if comp.is_empty() || comp == "." || comp == ".." {
        return Err(RelPathError::Traversal(whole.to_owned()));
    }
    if comp.len() > MAX_COMPONENT_LEN {
        return Err(RelPathError::TooLong);
    }
    if comp
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(RelPathError::IllegalChar(comp.to_owned()));
    }
    if comp.ends_with('.') || comp.ends_with(' ') {
        return Err(RelPathError::ReservedName(comp.to_owned()));
    }
    // "CON", "con.txt", "NUL.tar.gz" all name devices on Windows.
    let stem = comp.split('.').next().unwrap_or(comp).to_lowercase();
    if RESERVED_DEVICE_NAMES.contains(&stem.as_str()) {
        return Err(RelPathError::ReservedName(comp.to_owned()));
    }
    Ok(())
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}
impl Serialize for RelPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        RelPath::new(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for ok in [
            "instances/My Pack 1.21",
            "runtimes/java/temurin-21/bin/javaw.exe",
            "a",
            "mods/[x]+y.jar",
        ] {
            assert!(RelPath::new(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rejects_escapes_and_oddities() {
        let bad = [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a//b",
            "a/",
            "C:/Windows",
            "C:x",
            "\\\\server\\share",
            "a\\b",
            "file.txt:stream",
            "con",
            "dir/NUL.txt",
            "LPT1",
            "trail.",
            "trail /x",
            "a/b?",
            "a/<b>",
            "tab\tname",
        ];
        for b in bad {
            assert!(RelPath::new(b).is_err(), "should reject {b:?}");
        }
    }

    #[test]
    fn rejects_overlong() {
        assert!(RelPath::new(&"a".repeat(300)).is_err());
        assert!(
            RelPath::new(&["a".repeat(100), "b".repeat(100), "c".repeat(100)].join("/")).is_err()
        );
    }

    #[test]
    fn serde_roundtrip_validates() {
        let ok: RelPath = serde_json::from_str("\"instances/a\"").unwrap();
        assert_eq!(serde_json::to_string(&ok).unwrap(), "\"instances/a\"");
        assert!(serde_json::from_str::<RelPath>("\"../a\"").is_err());
        assert!(serde_json::from_str::<RelPath>("\"C:/a\"").is_err());
    }

    #[test]
    fn to_path_uses_platform_separator() {
        let p = RelPath::new("a/b/c").unwrap().to_path(Path::new("root"));
        assert_eq!(p, Path::new("root").join("a").join("b").join("c"));
    }
}
