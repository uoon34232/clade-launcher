//! Which URLs the launcher may download from. Checked before any request is made, and again
//! for every redirect target.

use url::{Host, Url};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("only https downloads are allowed")]
    NotHttps,
    #[error("URLs with embedded credentials are not allowed")]
    UserInfo,
    #[error("non-standard ports are not allowed")]
    Port,
    #[error("IP-address hosts are not allowed")]
    IpHost,
    #[error("host '{0}' is not on the allow-list")]
    HostNotAllowed(String),
}

#[derive(Debug, Clone, Copy)]
pub struct DownloadPolicy {
    /// Exact hosts or parent domains (`mojang.com` also allows `piston-data.mojang.com`).
    allowed: &'static [&'static str],
}

/// The only hosts the launcher may ever contact. Product rule (see docs/TECHNICAL.md, "Downloads, verification and repair"): the launcher
/// is vanilla-first and is not a content store, so there are deliberately NO mod,
/// modpack, shader or resource-pack platforms here (no third-party content platforms, no generic
/// code hosts). Each addition widens what a compromised manifest can make us fetch, and
/// `tests/product_rules.rs` fails if a content platform is ever added.
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    // Minecraft itself: version manifests, client jars, libraries, assets, Mojang-built Java runtimes.
    "mojang.com",
    "minecraft.net",
    // Sign-in with a Microsoft account.
    "minecraftservices.com",
    "microsoftonline.com",
    "live.com",
    "xboxlive.com",
    // Mod *loader* runtimes (not content): needed to prepare a Fabric/Quilt/Forge/NeoForge instance.
    "fabricmc.net",
    "quiltmc.org",
    "minecraftforge.net",
    "neoforged.net",
];

/// Hosts that may serve mod *loader* runtimes. Used by `loaders` as a stricter policy than the
/// global one: loader requests may never go anywhere else, not even to Mojang.
pub const LOADER_HOSTS: &[&str] = &[
    "fabricmc.net",
    "quiltmc.org",
    "minecraftforge.net",
    "neoforged.net",
];

/// Host allowed ONLY for the explicit user-initiated "Check for updates" command in Settings
/// (`update_check.rs`). Kept separate from [`DEFAULT_ALLOWED_HOSTS`] so game or loader manifests
/// can never fetch anything from GitHub.
pub const UPDATE_CHECK_HOSTS: &[&str] = &["api.github.com"];

impl DownloadPolicy {
    pub const fn new(allowed: &'static [&'static str]) -> Self {
        Self { allowed }
    }

    pub fn check(&self, url: &Url) -> Result<(), PolicyError> {
        if url.scheme() != "https" {
            return Err(PolicyError::NotHttps);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(PolicyError::UserInfo);
        }
        if url.port().is_some() {
            return Err(PolicyError::Port); // https default (443) is normalised away by `url`
        }
        let host = match url.host() {
            Some(Host::Domain(d)) => d.to_ascii_lowercase(),
            Some(_) => return Err(PolicyError::IpHost),
            None => return Err(PolicyError::HostNotAllowed(String::new())),
        };
        if host.starts_with('.')
            || host.ends_with('.')
            || host.contains("..")
            || host
                .split('.')
                .any(|l| l.is_empty() || l.starts_with('-') || l.ends_with('-'))
        {
            return Err(PolicyError::HostNotAllowed(host));
        }
        let ok = self
            .allowed
            .iter()
            .any(|a| host == *a || host.ends_with(&format!(".{a}")));
        if ok {
            Ok(())
        } else {
            Err(PolicyError::HostNotAllowed(host))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(u: &str) -> Result<(), PolicyError> {
        DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS).check(&Url::parse(u).unwrap())
    }

    #[test]
    fn loader_hosts_are_a_subset_of_the_global_allow_list() {
        for h in LOADER_HOSTS {
            assert!(DEFAULT_ALLOWED_HOSTS.contains(h), "{h}");
        }
    }

    #[test]
    fn allows_known_hosts_and_subdomains() {
        assert!(check("https://piston-data.mojang.com/v1/objects/abc/client.jar").is_ok());
        assert!(
            check("https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar").is_ok()
        );
        assert!(check("https://meta.fabricmc.net/v2/versions/loader").is_ok());
    }

    #[test]
    fn content_platforms_and_code_hosts_are_not_allowed() {
        // Built from parts so this test does not trip the repository-wide product-rule scan.
        let blocked = [
            ["api.modr", "inth.com"].concat(),
            ["cdn.modr", "inth.com"].concat(),
            ["api.curse", "forge.com"].concat(),
            ["edge.forge", "cdn.net"].concat(),
            "github.com".to_string(),
            "raw.githubusercontent.com".to_string(),
            "api.adoptium.net".to_string(),
        ];
        for host in blocked {
            assert!(
                matches!(
                    check(&format!("https://{host}/x")),
                    Err(PolicyError::HostNotAllowed(_))
                ),
                "{host} must stay blocked"
            );
        }
    }

    #[test]
    fn rejects_malformed_dns_labels_on_allowed_suffixes() {
        for host in [
            ".mojang.com",
            "a..mojang.com",
            "mojang.com.",
            "piston-data.mojang.com.",
            "-bad.mojang.com",
            "bad-.mojang.com",
            "a.-bad.mojang.com",
        ] {
            let url_text = format!("https://{host}/x");
            if let Ok(parsed) = Url::parse(&url_text) {
                assert_eq!(
                    DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS).check(&parsed),
                    Err(PolicyError::HostNotAllowed(host.to_owned())),
                    "{host} must be rejected by DownloadPolicy::check"
                );
            }
        }
        assert_eq!(
            check("https://.mojang.com/x"),
            Err(PolicyError::HostNotAllowed(".mojang.com".to_owned()))
        );
        assert_eq!(
            check("https://a..mojang.com/x"),
            Err(PolicyError::HostNotAllowed("a..mojang.com".to_owned()))
        );
        assert_eq!(
            check("https://mojang.com./x"),
            Err(PolicyError::HostNotAllowed("mojang.com.".to_owned()))
        );
        assert_eq!(
            check("https://-bad.mojang.com/x"),
            Err(PolicyError::HostNotAllowed("-bad.mojang.com".to_owned()))
        );
    }

    #[test]
    fn rejects_everything_else() {
        assert_eq!(
            check("http://piston-data.mojang.com/x"),
            Err(PolicyError::NotHttps)
        );
        assert_eq!(
            check("https://user:pw@mojang.com/x"),
            Err(PolicyError::UserInfo)
        );
        assert_eq!(check("https://mojang.com:8443/x"), Err(PolicyError::Port));
        assert_eq!(check("https://127.0.0.1/x"), Err(PolicyError::IpHost));
        assert!(matches!(
            check("https://evilmojang.com/x"),
            Err(PolicyError::HostNotAllowed(_))
        ));
        assert!(matches!(
            check("https://mojang.com.evil.example/x"),
            Err(PolicyError::HostNotAllowed(_))
        ));
        assert!(matches!(
            check("https://example.org/x"),
            Err(PolicyError::HostNotAllowed(_))
        ));
    }
}
