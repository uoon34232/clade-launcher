//! Manual update check in Settings against GitHub Releases.
//!
//! Never runs automatically at startup or in the background: only executed when the user
//! explicitly clicks "Check for updates". Uses an isolated single-host [`DownloadPolicy`]
//! (`UPDATE_CHECK_HOSTS`, `api.github.com` only, no redirects, no cookies) completely separate from
//! `DEFAULT_ALLOWED_HOSTS`. "Open release" constructs only canonical Clade release URLs in Rust.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::coded_error::CodedError;
use crate::download_policy::{DownloadPolicy, UPDATE_CHECK_HOSTS};
use crate::net::{FetchError, HttpFetcher, HttpRequest, Method, Transport};

pub const RELEASES_API_URL: &str =
    "https://api.github.com/repos/uoon34232/clade-launcher/releases?per_page=20";
pub const RELEASES_WEB_BASE: &str = "https://github.com/uoon34232/clade-launcher/releases";
const MAX_RELEASES_BYTES: usize = 256 * 1024;

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
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
}

pub async fn check_with<T: Transport>(
    transport: &T,
    current_raw: &str,
) -> Result<UpdateCheckDto, CodedError> {
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
        .filter_map(|r| parse_release_tag(&r.tag_name))
        .max_by(|a, b| a.1.cmp(&b.1));
    Ok(match latest {
        None => UpdateCheckDto {
            status: UpdateStatus::UpToDate,
            current_version,
            latest_version: None,
        },
        Some((tag, ver)) if ver > current_ver => UpdateCheckDto {
            status: UpdateStatus::UpdateAvailable,
            current_version,
            latest_version: Some(tag),
        },
        Some((tag, _)) => UpdateCheckDto {
            status: UpdateStatus::UpToDate,
            current_version,
            latest_version: Some(tag),
        },
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

pub struct UpdateState {
    pub http: Result<HttpFetcher, String>,
}

impl UpdateState {
    pub fn new() -> Self {
        Self {
            http: HttpFetcher::for_auth(DownloadPolicy::new(UPDATE_CHECK_HOSTS))
                .map_err(|e| e.to_string()),
        }
    }
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::new()
    }
}

/// Queries GitHub Releases once on explicit user action to see whether a newer release exists.
#[tauri::command]
pub async fn check_for_updates(
    state: State<'_, UpdateState>,
) -> Result<UpdateCheckDto, CodedError> {
    let http = state
        .http
        .as_ref()
        .map_err(|m| CodedError::new("network_unavailable", m.clone()))?;
    let dto = check_with(http, installed_version_tag()).await?;
    tracing::info!(status = ?dto.status, "checked for updates");
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
}
