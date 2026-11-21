//! Manual update check in Settings against GitHub Releases.
//!
//! Never runs automatically at startup or in the background: only executed when the user
//! explicitly clicks "Check for updates". Uses an isolated single-host [`DownloadPolicy`]
//! (`UPDATE_CHECK_HOSTS`, `api.github.com` only, no redirects, no cookies) completely separate from
//! `DEFAULT_ALLOWED_HOSTS`. Download/install endpoints are restricted separately via
//! `UPDATE_DOWNLOAD_HOSTS`. "Open release" constructs only canonical Clade release URLs in Rust.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::coded_error::CodedError;
use crate::download_policy::{DownloadPolicy, UPDATE_CHECK_HOSTS, UPDATE_DOWNLOAD_HOSTS};
use crate::net::{FetchError, HttpFetcher, HttpRequest, Method, Transport};
use crate::portable::PortableRoot;

pub const RELEASES_API_URL: &str =
    "https://api.github.com/repos/uoon34232/clade-launcher/releases?per_page=20";
pub const RELEASES_WEB_BASE: &str = "https://github.com/uoon34232/clade-launcher/releases";
pub const PORTABLE_ZIP_ASSET: &str = "Clade-Launcher-Portable.zip";
pub const PORTABLE_SHA256_ASSET: &str = "Clade-Launcher-Portable.zip.sha256";
const MAX_RELEASES_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    Preview(u32),
    Rc(u32),
    Stable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub channel: Channel,
}

fn parse_num(s: &str) -> Option<u32> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse::<u32>().ok()
}

pub fn parse_release_tag(raw: &str) -> Option<(String, ReleaseVersion)> {
    let rest = raw.strip_prefix('v').unwrap_or(raw);
    let (core, suffix) = match rest.split_once('-') {
        Some((c, s)) => (c, Some(s)),
        None => (rest, None),
    };
    let mut parts = core.split('.');
    let (Some(maj), Some(min), Some(pat), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let major = parse_num(maj)?;
    let minor = parse_num(min)?;
    let patch = parse_num(pat)?;
    let channel = match suffix {
        None => Channel::Stable,
        Some("preview") => Channel::Preview(0),
        Some(s) if s.starts_with("preview.") => {
            Channel::Preview(parse_num(s.strip_prefix("preview.")?)?)
        }
        Some("rc") => Channel::Rc(0),
        Some(s) if s.starts_with("rc.") => Channel::Rc(parse_num(s.strip_prefix("rc.")?)?),
        _ => return None,
    };
    Some((
        format!("v{rest}"),
        ReleaseVersion {
            major,
            minor,
            patch,
            channel,
        },
    ))
}

pub fn installed_version_tag() -> &'static str {
    match option_env!("CLADE_RELEASE_TAG") {
        Some(tag) if !tag.is_empty() => tag,
        _ => concat!("v", env!("CARGO_PKG_VERSION")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckDto {
    pub status: UpdateStatus,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub can_install: bool,
    pub asset_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedRelease {
    pub tag: String,
    pub zip_size: Option<u64>,
    pub api_sha256: Option<String>,
    pub can_install: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

fn parse_api_digest(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix("sha256:")?;
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hex.to_ascii_lowercase())
    } else {
        None
    }
}

pub fn parse_portable_sha256(text: &str) -> Result<String, CodedError> {
    let line = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let b = line.as_bytes();
    if b.len() <= 66
        || !b[..64].iter().all(u8::is_ascii_hexdigit)
        || b[64] != b' '
        || !(b[65] == b' ' || b[65] == b'*')
        || &line[66..] != PORTABLE_ZIP_ASSET
    {
        return Err(CodedError::new(
            "update_verify_failed",
            "unexpected portable update checksum format",
        ));
    }
    Ok(line[..64].to_ascii_lowercase())
}

pub async fn check_with<T: Transport>(
    transport: &T,
    current_raw: &str,
) -> Result<UpdateCheckDto, CodedError> {
    check_with_meta(transport, current_raw)
        .await
        .map(|(dto, _)| dto)
}

pub async fn check_with_meta<T: Transport>(
    transport: &T,
    current_raw: &str,
) -> Result<(UpdateCheckDto, Option<CheckedRelease>), CodedError> {
    let (current_version, current_ver) = parse_release_tag(current_raw)
        .or_else(|| parse_release_tag(concat!("v", env!("CARGO_PKG_VERSION"))))
        .ok_or_else(|| CodedError::new("update_check_failed", "invalid build version"))?;
    let url = Url::parse(RELEASES_API_URL)
        .map_err(|e| CodedError::new("update_check_failed", e.to_string()))?;
    let resp = transport
        .send(HttpRequest {
            method: Method::Get,
            url,
            headers: vec![
                ("Accept", "application/vnd.github+json".to_owned()),
                ("X-GitHub-Api-Version", "2022-11-28".to_owned()),
            ],
            body: Vec::new(),
            max_response_bytes: MAX_RELEASES_BYTES,
        })
        .await
        .map_err(|e| match e {
            FetchError::Network(m) => CodedError::new("offline", m),
            other => CodedError::new("update_check_failed", other.to_string()),
        })?;
    match resp.status {
        200 => {}
        403 | 429 => {
            return Err(CodedError::new(
                "update_rate_limited",
                format!("GitHub API status {}", resp.status),
            ));
        }
        status => {
            return Err(CodedError::new(
                "update_check_failed",
                format!("GitHub API status {status}"),
            ));
        }
    }
    let releases: Vec<GitHubRelease> = serde_json::from_slice(&resp.body)
        .map_err(|e| CodedError::new("update_check_failed", e.to_string()))?;
    let latest = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| parse_release_tag(&r.tag_name).map(|(tag, ver)| (tag, ver, r.assets)))
        .max_by(|a, b| a.1.cmp(&b.1));
    Ok(match latest {
        None => (
            UpdateCheckDto {
                status: UpdateStatus::UpToDate,
                current_version,
                latest_version: None,
                can_install: false,
                asset_size: None,
            },
            None,
        ),
        Some((tag, ver, assets)) if ver > current_ver => {
            let zip_asset = assets.iter().find(|a| a.name == PORTABLE_ZIP_ASSET);
            let has_sha = assets.iter().any(|a| a.name == PORTABLE_SHA256_ASSET);
            let zip_size = zip_asset.and_then(|a| (a.size > 0).then_some(a.size));
            let api_sha256 = zip_asset
                .and_then(|a| a.digest.as_deref())
                .and_then(parse_api_digest);
            let can_install = zip_size.is_some() && has_sha;
            let checked = CheckedRelease {
                tag: tag.clone(),
                zip_size,
                api_sha256,
                can_install,
            };
            (
                UpdateCheckDto {
                    status: UpdateStatus::UpdateAvailable,
                    current_version,
                    latest_version: Some(tag),
                    can_install,
                    asset_size: zip_size,
                },
                Some(checked),
            )
        }
        Some((tag, _, _)) => (
            UpdateCheckDto {
                status: UpdateStatus::UpToDate,
                current_version,
                latest_version: Some(tag),
                can_install: false,
                asset_size: None,
            },
            None,
        ),
    })
}

pub fn release_page_url(tag: Option<&str>) -> Result<String, CodedError> {
    match tag {
        None => Ok(RELEASES_WEB_BASE.to_owned()),
        Some(raw) => {
            let (canonical, _) = parse_release_tag(raw)
                .ok_or_else(|| CodedError::new("unsupported_value", "invalid release tag"))?;
            Ok(format!("{RELEASES_WEB_BASE}/tag/{canonical}"))
        }
    }
}

pub fn release_asset_url(tag: &str, asset_name: &str) -> Result<Url, CodedError> {
    if asset_name != PORTABLE_ZIP_ASSET && asset_name != PORTABLE_SHA256_ASSET {
        return Err(CodedError::new(
            "unsupported_value",
            "unsupported release asset",
        ));
    }
    let (canonical, _) = parse_release_tag(tag)
        .ok_or_else(|| CodedError::new("unsupported_value", "invalid release tag"))?;
    Url::parse(&format!(
        "{RELEASES_WEB_BASE}/download/{canonical}/{asset_name}"
    ))
    .map_err(|e| CodedError::new("update_check_failed", e.to_string()))
}

pub struct UpdateState {
    pub root: PortableRoot,
    pub http: Result<HttpFetcher, String>,
    pub sha_http: Result<HttpFetcher, String>,
    pub zip_http: Result<HttpFetcher, String>,
    pub checked: Mutex<Option<CheckedRelease>>,
    pub apply: crate::update_apply::ApplyRuntime,
}

impl UpdateState {
    pub fn new(root: PortableRoot) -> Self {
        Self {
            root,
            http: HttpFetcher::for_auth(DownloadPolicy::new(UPDATE_CHECK_HOSTS))
                .map_err(|e| e.to_string()),
            sha_http: HttpFetcher::new(DownloadPolicy::new(UPDATE_DOWNLOAD_HOSTS))
                .map_err(|e| e.to_string()),
            zip_http: HttpFetcher::for_downloads(DownloadPolicy::new(UPDATE_DOWNLOAD_HOSTS))
                .map_err(|e| e.to_string()),
            checked: Mutex::new(None),
            apply: crate::update_apply::ApplyRuntime::default(),
        }
    }
}

pub(crate) async fn refresh_checked_release(
    state: &UpdateState,
) -> Result<(UpdateCheckDto, Option<CheckedRelease>), CodedError> {
    let http = state
        .http
        .as_ref()
        .map_err(|m| CodedError::new("network_unavailable", m.clone()))?;
    let (dto, checked) = check_with_meta(http, installed_version_tag()).await?;
    *state.checked.lock().unwrap_or_else(|e| e.into_inner()) = checked.clone();
    Ok((dto, checked))
}

/// Queries GitHub Releases once on explicit user action to see whether a newer release exists.
#[tauri::command]
pub async fn check_for_updates(
    state: State<'_, UpdateState>,
) -> Result<UpdateCheckDto, CodedError> {
    let (dto, _) = refresh_checked_release(&state).await?;
    tracing::info!(
        status = ?dto.status,
        can_install = dto.can_install,
        "checked for updates"
    );
    Ok(dto)
}

/// Opens the canonical Clade release page in the user's system browser.
#[tauri::command]
pub async fn open_release_page(app: AppHandle, tag: Option<String>) -> Result<(), CodedError> {
    let url = release_page_url(tag.as_deref())?;
    tokio::task::spawn_blocking(move || {
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|e| CodedError::new("browser_unavailable", e.to_string()))
    })
    .await
    .map_err(|e| CodedError::new("browser_unavailable", e.to_string()))?
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::net::HttpResponse;

    use super::*;

    struct FakeTransport {
        reply: Result<HttpResponse, FetchError>,
        seen_urls: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        fn json(status: u16, body: &str) -> Self {
            Self {
                reply: Ok(HttpResponse {
                    status,
                    body: body.as_bytes().to_vec(),
                }),
                seen_urls: Mutex::new(Vec::new()),
            }
        }

        fn err(err: FetchError) -> Self {
            Self {
                reply: Err(err),
                seen_urls: Mutex::new(Vec::new()),
            }
        }
    }

    impl Transport for FakeTransport {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, FetchError> {
            self.seen_urls.lock().unwrap().push(request.url.to_string());
            match &self.reply {
                Ok(r) => Ok(HttpResponse {
                    status: r.status,
                    body: r.body.clone(),
                }),
                Err(e) => Err(e.clone()),
            }
        }
    }

    #[test]
    fn tag_ordering_handles_previews_release_candidates_and_stable_releases() {
        let ordered = [
            "v0.1.0-preview",
            "v0.1.0-preview.1",
            "v0.1.0-preview.3",
            "v0.1.0-preview.10",
            "v0.1.0-rc",
            "v0.1.0-rc.2",
            "v0.1.0",
            "v0.1.1-preview",
            "v0.2.0",
            "v1.0.0",
        ];
        let parsed: Vec<_> = ordered
            .iter()
            .map(|t| parse_release_tag(t).unwrap())
            .collect();
        for pair in parsed.windows(2) {
            assert!(pair[0].1 < pair[1].1, "{} < {}", pair[0].0, pair[1].0);
        }
        assert_eq!(parse_release_tag("0.1.0").unwrap().0, "v0.1.0");
        assert!(parse_release_tag("v01.0.0").is_none());
        assert!(parse_release_tag("v0.1.0-alpha.1").is_none());
        assert!(parse_release_tag("v0.1.0/../../evil").is_none());
        assert!(parse_release_tag("latest").is_none());
    }

    #[tokio::test]
    async fn check_with_detects_newer_preview_or_stable_ignores_drafts_and_maps_errors() {
        let body = r#"[
            {"tag_name":"v0.2.0","draft":true},
            {"tag_name":"not-a-clade-tag","draft":false},
            {"tag_name":"v0.1.0-preview.4","draft":false},
            {"tag_name":"v0.1.0-preview.2","draft":false}
        ]"#;
        let t = FakeTransport::json(200, body);
        let dto = check_with(&t, "v0.1.0-preview.3").await.unwrap();
        assert_eq!(dto.status, UpdateStatus::UpdateAvailable);
        assert_eq!(dto.current_version, "v0.1.0-preview.3");
        assert_eq!(dto.latest_version.as_deref(), Some("v0.1.0-preview.4"));
        assert_eq!(&*t.seen_urls.lock().unwrap(), &[RELEASES_API_URL]);

        let dto = check_with(&t, "v0.1.0-preview.4").await.unwrap();
        assert_eq!(dto.status, UpdateStatus::UpToDate);
        assert_eq!(dto.latest_version.as_deref(), Some("v0.1.0-preview.4"));

        // Plain dev `0.1.0` treats `v0.1.0` as stable over previews, yet detects newer minor/patch.
        let newer = FakeTransport::json(200, r#"[{"tag_name":"v0.2.0-preview.1","draft":false}]"#);
        let dto = check_with(&newer, "0.1.0").await.unwrap();
        assert_eq!(dto.status, UpdateStatus::UpdateAvailable);
        assert_eq!(dto.current_version, "v0.1.0");

        let dto = check_with(&t, "v0.1.0").await.unwrap();
        assert_eq!(dto.status, UpdateStatus::UpToDate);

        let empty = FakeTransport::json(200, "[]");
        let dto = check_with(&empty, "v0.1.0-preview.3").await.unwrap();
        assert_eq!(dto.status, UpdateStatus::UpToDate);
        assert_eq!(dto.latest_version, None);

        assert_eq!(
            check_with(&FakeTransport::json(403, ""), "v0.1.0")
                .await
                .unwrap_err()
                .code,
            "update_rate_limited"
        );
        assert_eq!(
            check_with(&FakeTransport::json(429, ""), "v0.1.0")
                .await
                .unwrap_err()
                .code,
            "update_rate_limited"
        );
        assert_eq!(
            check_with(&FakeTransport::json(500, ""), "v0.1.0")
                .await
                .unwrap_err()
                .code,
            "update_check_failed"
        );
        assert_eq!(
            check_with(&FakeTransport::json(200, "not-json"), "v0.1.0")
                .await
                .unwrap_err()
                .code,
            "update_check_failed"
        );
        assert_eq!(
            check_with(
                &FakeTransport::err(FetchError::Network("dns".into())),
                "v0.1.0"
            )
            .await
            .unwrap_err()
            .code,
            "offline"
        );
    }

    #[test]
    fn release_page_url_accepts_only_canonical_clade_tags() {
        assert_eq!(release_page_url(None).unwrap(), RELEASES_WEB_BASE);
        assert_eq!(
            release_page_url(Some("v0.1.0-preview.4")).unwrap(),
            "https://github.com/uoon34232/clade-launcher/releases/tag/v0.1.0-preview.4"
        );
        assert_eq!(
            release_page_url(Some("0.2.0")).unwrap(),
            "https://github.com/uoon34232/clade-launcher/releases/tag/v0.2.0"
        );
        for bad in [
            "javascript:alert(1)",
            "https://evil.example/",
            "v0.1.0?foo=1",
            "../x",
        ] {
            assert_eq!(
                release_page_url(Some(bad)).unwrap_err().code,
                "unsupported_value"
            );
        }
    }

    #[tokio::test]
    async fn release_assets_metadata_and_asset_helpers_validate_strictly() {
        let hex = "A".repeat(64);
        let body = format!(
            r#"[{{
                "tag_name": "v0.1.0-preview.5",
                "draft": false,
                "assets": [
                    {{"name": "Clade-Launcher-Portable.zip", "size": 319815680, "digest": "sha256:{hex}"}},
                    {{"name": "Clade-Launcher-Portable.zip.sha256", "size": 94}}
                ]
            }}]"#
        );
        let (dto, checked) = check_with_meta(&FakeTransport::json(200, &body), "v0.1.0-preview.4")
            .await
            .unwrap();
        assert!(dto.can_install);
        assert_eq!(dto.asset_size, Some(319_815_680));
        let checked = checked.unwrap();
        assert!(checked.can_install);
        assert_eq!(checked.api_sha256.as_deref(), Some("a".repeat(64).as_str()));

        let missing_sha = r#"[{"tag_name":"v0.2.0","draft":false,"assets":[{"name":"Clade-Launcher-Portable.zip","size":100}]}]"#;
        let (dto2, checked2) = check_with_meta(&FakeTransport::json(200, missing_sha), "v0.1.0")
            .await
            .unwrap();
        assert!(!dto2.can_install);
        assert!(!checked2.unwrap().can_install);

        assert_eq!(
            release_asset_url("0.1.0-preview.5", PORTABLE_ZIP_ASSET)
                .unwrap()
                .as_str(),
            "https://github.com/uoon34232/clade-launcher/releases/download/v0.1.0-preview.5/Clade-Launcher-Portable.zip"
        );
        assert_eq!(
            release_asset_url("v0.1.0", "evil.exe").unwrap_err().code,
            "unsupported_value"
        );
        assert_eq!(
            release_asset_url("../v0.1.0", PORTABLE_ZIP_ASSET)
                .unwrap_err()
                .code,
            "unsupported_value"
        );

        let parsed = parse_portable_sha256(&format!("{hex}  Clade-Launcher-Portable.zip\r\n"))
            .expect("valid sha256 line");
        assert_eq!(parsed, "a".repeat(64));
        assert!(parse_portable_sha256(&format!("{hex} *Clade-Launcher-Portable.zip")).is_ok());
        assert_eq!(
            parse_portable_sha256(&format!("{hex}  other.zip"))
                .unwrap_err()
                .code,
            "update_verify_failed"
        );
        assert_eq!(
            parse_portable_sha256("short  Clade-Launcher-Portable.zip")
                .unwrap_err()
                .code,
            "update_verify_failed"
        );
    }
}
