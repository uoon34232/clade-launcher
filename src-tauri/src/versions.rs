//! Validated version identifiers. They end up in file names, URLs and (later) command lines,
//! so each is restricted to a conservative alphabet and can never start with `.` or `-`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::portable::RelPath;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid {what}: {value:?}")]
pub struct InvalidToken {
    pub what: &'static str,
    pub value: String,
}

macro_rules! token_newtype {
    ($(#[$doc:meta])* $name:ident, $what:literal, $space:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: &str) -> Result<Self, InvalidToken> {
                let ok = !s.is_empty()
                    && s.len() <= 64
                    && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-') || ($space && c == ' '))
                    // A leading '-' could be mistaken for a command-line flag, '.' for a hidden/relative name.
                    && !s.starts_with(['.', '-'])
                    // Where spaces are allowed (real Mojang ids such as "1.14 Pre-Release 1"): single, interior only.
                    && !s.starts_with(' ')
                    && !s.ends_with(' ')
                    && !s.contains("  ")
                    && RelPath::new(s).is_ok();
                if ok {
                    Ok(Self(s.to_owned()))
                } else {
                    Err(InvalidToken { what: $what, value: s.to_owned() })
                }
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                $name::new(&String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

token_newtype!(
    /// A Minecraft version id exactly as Mojang's version manifest spells it (`1.21.4`, `24w10a`,
    /// `1.21.5-pre1`, `b1.7.3`, `rd-132211`, ...). Never enumerated or restricted by the launcher.
    McVersion,
    "Minecraft version",
    true // twelve real ids contain spaces, e.g. "1.14.2 Pre-Release 4" and "3D Shareware v1.34"
);
token_newtype!(
    /// A mod loader's own version string (`0.16.9`, `54.1.14`, `21.4.158`, `0.20.0-beta.9`).
    LoaderVersion,
    "loader version",
    false
);
token_newtype!(
    /// The id of an installed, launchable version profile under `minecraft/versions/`
    /// (`1.21.4`, `fabric-loader-0.16.9-1.21.4`, `neoforge-21.4.158`).
    VersionId,
    "version id",
    true // a Vanilla profile's launchable id is its Minecraft version id
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_real_world_ids() {
        for v in [
            "1.21.4",
            "24w10a",
            "1.21.5-pre1",
            "b1.7.3",
            "rd-132211",
            "0.20.0-beta.9",
            "54.1.14",
            "fabric-loader-0.16.9-1.21.4",
            "1.21.4-forge-54.1.14",
            "21.4.158-beta",
        ] {
            assert!(
                McVersion::new(v).is_ok()
                    && LoaderVersion::new(v).is_ok()
                    && VersionId::new(v).is_ok(),
                "{v}"
            );
        }
    }

    #[test]
    fn real_mojang_ids_with_spaces_are_accepted_for_minecraft_versions_only() {
        for v in [
            "1.14.2 Pre-Release 4",
            "1.14 Pre-Release 1",
            "3D Shareware v1.34",
        ] {
            assert!(
                McVersion::new(v).is_ok() && VersionId::new(v).is_ok(),
                "{v}"
            );
            assert!(
                LoaderVersion::new(v).is_err(),
                "loader versions never contain spaces: {v}"
            );
        }
    }

    #[test]
    fn rejects_anything_that_could_escape_or_act_like_a_flag() {
        for bad in [
            "",
            "../1.21",
            "1.21/4",
            "a\\b",
            "C:1",
            "con",
            " 1.21",
            "1.21 ",
            "1  21",
            "a	b",
            "-rf",
            "--version",
            ".hidden",
            "a;b",
            "a&b",
            "x\n",
            &"9".repeat(65),
        ] {
            assert!(McVersion::new(bad).is_err(), "{bad:?}");
            assert!(LoaderVersion::new(bad).is_err(), "{bad:?}");
            assert!(VersionId::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn serde_validates_on_load() {
        assert!(serde_json::from_str::<McVersion>("\"1.21.4\"").is_ok());
        assert!(serde_json::from_str::<McVersion>("\"../x\"").is_err());
        assert_eq!(
            serde_json::to_string(&LoaderVersion::new("0.16.9").unwrap()).unwrap(),
            "\"0.16.9\""
        );
    }
}
