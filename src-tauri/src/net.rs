//! The launcher's only door to the network.
//!
//! Network policy lives here, in Rust, and nowhere else: the frontend never fetches Minecraft or
//! loader metadata itself. Every request must pass a [`DownloadPolicy`] (https only, an explicit
//! host allow-list, no credentials, no odd ports, no IP literals); the same policy is re-applied
//! to every redirect, responses are size-capped, and connections time out quickly so an offline
//! PC falls back to the cached catalog instead of hanging.
//!
//! Callers depend on the small [`Fetcher`] trait, which keeps catalog logic testable without a
//! network. The HTTP client sends no cookies and an anonymous user agent (no OS or machine
//! details), and writes nothing to disk: persistence is the callers' job, inside `cache/`.

use std::future::Future;
use std::time::Duration;

use url::Url;

use crate::download_policy::{DownloadPolicy, PolicyError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REDIRECTS: usize = 5;
/// Longest silence tolerated while streaming a large file.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Documents,
    Credentials,
    Downloads,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("blocked by the network policy: {0}")]
    Policy(#[from] PolicyError),
    /// No connection, DNS failure, timeout, TLS failure: anything that means "cannot reach it".
    #[error("network unavailable: {0}")]
    Network(String),
    #[error("the server answered with status {0}")]
    Status(u16),
    #[error("the response is larger than the {0}-byte limit")]
    TooLarge(usize),
    /// The data arrived but could not be stored (disk full, read-only stick, cancelled).
    #[error("could not store the downloaded data: {0}")]
    Local(String),
}

/// Fetches a small document. Implemented by [`HttpFetcher`] for real and by a fake in tests.
pub trait Fetcher: Send + Sync {
    fn get(
        &self,
        url: &Url,
        max_bytes: usize,
    ) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send;
}

/// What a request may carry. Bodies are plain bytes: callers encode forms/JSON themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// A request that may carry credentials. `Debug` is redacted: headers and bodies can hold tokens,
/// so they must never reach a log or an error message.
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
    pub max_response_bytes: usize,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "HttpRequest({:?} {}, <headers and body redacted>)",
            self.method,
            self.url.host_str().unwrap_or("?")
        )
    }
}

/// A response of any status. Unlike [`Fetcher`], error statuses are returned, because the
/// identity services explain failures in the body of a 4xx.
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends requests that may carry credentials (sign-in). Same policy as [`Fetcher`]; additionally
/// redirects are never followed, so a credential can never be forwarded anywhere.
pub trait Transport: Send + Sync {
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, FetchError>> + Send;
}

/// What the server said before the body: 200 (the whole file) or 206 (the requested range).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamHead {
    pub status: u16,
    /// Length of the body about to be sent, when the server says.
    pub content_length: Option<u64>,
}

/// Receives a streamed body. `begin` is called once, before any data.
pub trait StreamSink: Send {
    fn begin(&mut self, head: &StreamHead) -> std::io::Result<()>;
    fn chunk(&mut self, data: &[u8]) -> std::io::Result<()>;
}

/// Streams large files without holding them in memory, optionally resuming from a byte offset
/// (HTTP `Range`). The same policy as [`Fetcher`] applies to every request and redirect.
pub trait Streamer: Send + Sync {
    #[allow(clippy::manual_async_fn)] // the trait spells out the borrowed lifetime
    fn stream<'a>(
        &'a self,
        url: &'a Url,
        from: u64,
        sink: &'a mut dyn StreamSink,
    ) -> impl Future<Output = Result<(), FetchError>> + Send + 'a;
}

pub struct HttpFetcher {
    client: reqwest::Client,
    policy: DownloadPolicy,
}

impl HttpFetcher {
    pub fn new(policy: DownloadPolicy) -> Result<Self, FetchError> {
        Self::build(policy, Mode::Documents)
    }

    /// For requests that carry credentials: never follows a redirect.
    pub fn for_auth(policy: DownloadPolicy) -> Result<Self, FetchError> {
        Self::build(policy, Mode::Credentials)
    }

    /// For large files: no total time limit (a slow link must not abort a big file), but every
    /// read must make progress within [`READ_TIMEOUT`], so a stalled connection fails and is retried.
    pub fn for_downloads(policy: DownloadPolicy) -> Result<Self, FetchError> {
        Self::build(policy, Mode::Downloads)
    }

    fn build(policy: DownloadPolicy, mode: Mode) -> Result<Self, FetchError> {
        let redirect = if mode == Mode::Credentials {
            reqwest::redirect::Policy::none()
        } else {
            reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if policy.check(attempt.url()).is_ok() {
                    attempt.follow()
                } else {
                    attempt.error("redirect target is not on the allow-list")
                }
            })
        };
        let builder = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("Clade/", env!("CARGO_PKG_VERSION")))
            .redirect(redirect);
        let builder = if mode == Mode::Downloads {
            builder.read_timeout(READ_TIMEOUT)
        } else {
            builder.timeout(TOTAL_TIMEOUT)
        };
        let client = builder
            .build()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        Ok(Self { client, policy })
    }
}

impl Streamer for HttpFetcher {
    #[allow(clippy::manual_async_fn)] // the trait spells out the borrowed lifetime
    fn stream<'a>(
        &'a self,
        url: &'a Url,
        from: u64,
        sink: &'a mut dyn StreamSink,
    ) -> impl Future<Output = Result<(), FetchError>> + Send + 'a {
        async move {
            self.policy.check(url)?;
            let mut request = self.client.get(url.clone());
            if from > 0 {
                request = request.header("Range", format!("bytes={from}-"));
            }
            let mut response = request
                .send()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?;
            let status = response.status().as_u16();
            if status != 200 && status != 206 {
                return Err(FetchError::Status(status));
            }
            if status == 206 {
                // The body must start exactly where we asked, or appending would corrupt the file.
                let starts_right = response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.starts_with(&format!("bytes {from}-")));
                if !starts_right {
                    return Err(FetchError::Network("unexpected byte range".into()));
                }
            }
            let head = StreamHead {
                status,
                content_length: response.content_length(),
            };
            sink.begin(&head)
                .map_err(|e| FetchError::Local(e.to_string()))?;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?
            {
                sink.chunk(&chunk)
                    .map_err(|e| FetchError::Local(e.to_string()))?;
            }
            Ok(())
        }
    }
}

impl Transport for HttpFetcher {
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, FetchError>> + Send {
        let client = self.client.clone();
        let policy = self.policy;
        async move {
            policy.check(&request.url)?;
            let mut builder = match request.method {
                Method::Get => client.get(request.url.clone()),
                Method::Post => client.post(request.url.clone()).body(request.body),
            };
            for (name, value) in &request.headers {
                builder = builder.header(*name, value);
            }
            let mut response = builder
                .send()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?;
            let status = response.status().as_u16();
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?
            {
                if body.len() + chunk.len() > request.max_response_bytes {
                    return Err(FetchError::TooLarge(request.max_response_bytes));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(HttpResponse { status, body })
        }
    }
}

impl Fetcher for HttpFetcher {
    fn get(
        &self,
        url: &Url,
        max_bytes: usize,
    ) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send {
        let client = self.client.clone();
        let policy = self.policy;
        let url = url.clone();
        async move {
            policy.check(&url)?;
            let mut response = client
                .get(url)
                .send()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?;
            if !response.status().is_success() {
                return Err(FetchError::Status(response.status().as_u16()));
            }
            if response
                .content_length()
                .is_some_and(|n| n > max_bytes as u64)
            {
                return Err(FetchError::TooLarge(max_bytes));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?
            {
                if body.len() + chunk.len() > max_bytes {
                    return Err(FetchError::TooLarge(max_bytes));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        }
    }
}

/// A scripted fetcher for tests: answers from a table, records every request, never touches a network.
#[cfg(test)]
pub mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeFetcher {
        routes: Mutex<HashMap<String, Result<Vec<u8>, FetchError>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeFetcher {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn ok(&self, url: &str, body: impl AsRef<[u8]>) {
            self.routes
                .lock()
                .unwrap()
                .insert(url.to_owned(), Ok(body.as_ref().to_vec()));
        }
        pub fn fail(&self, url: &str) {
            self.routes.lock().unwrap().insert(
                url.to_owned(),
                Err(FetchError::Network("offline (test)".into())),
            );
        }
        pub fn fail_all(&self) {
            self.routes.lock().unwrap().clear();
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetcher for FakeFetcher {
        fn get(
            &self,
            url: &Url,
            max_bytes: usize,
        ) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send {
            self.calls.lock().unwrap().push(url.to_string());
            let result = match self.routes.lock().unwrap().get(url.as_str()) {
                Some(Ok(b)) if b.len() > max_bytes => Err(FetchError::TooLarge(max_bytes)),
                Some(r) => r.clone(),
                None => Err(FetchError::Network("no route (test)".into())),
            };
            async move { result }
        }
    }
}

/// A scripted transport for tests: each URL answers from a queue, every request is recorded.
#[cfg(test)]
pub mod testing_transport {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    type Scripted = VecDeque<Result<(u16, Vec<u8>), FetchError>>;
    pub type Call = (Method, String, Vec<(String, String)>, String);

    #[derive(Default)]
    pub struct FakeTransport {
        routes: Mutex<HashMap<String, Scripted>>,
        calls: Mutex<Vec<Call>>,
    }

    impl FakeTransport {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn reply(&self, url: &str, status: u16, body: impl AsRef<[u8]>) {
            self.routes
                .lock()
                .unwrap()
                .entry(url.to_owned())
                .or_default()
                .push_back(Ok((status, body.as_ref().to_vec())));
        }
        /// Forgets what is scripted for a URL.
        pub fn clear(&self, url: &str) {
            self.routes.lock().unwrap().remove(url);
        }
        pub fn offline(&self, url: &str) {
            self.routes
                .lock()
                .unwrap()
                .entry(url.to_owned())
                .or_default()
                .push_back(Err(FetchError::Network("offline (test)".into())));
        }
        /// (method, url without query, headers, body as text)
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        pub fn urls(&self) -> Vec<String> {
            self.calls().into_iter().map(|c| c.1).collect()
        }
    }

    impl Transport for FakeTransport {
        fn send(
            &self,
            request: HttpRequest,
        ) -> impl Future<Output = Result<HttpResponse, FetchError>> + Send {
            let mut key = request.url.clone();
            key.set_query(None);
            let key = key.to_string();
            self.calls.lock().unwrap().push((
                request.method,
                key.clone(),
                request
                    .headers
                    .iter()
                    .map(|(n, v)| ((*n).to_owned(), v.clone()))
                    .collect(),
                String::from_utf8_lossy(&request.body).into_owned(),
            ));
            let next = self
                .routes
                .lock()
                .unwrap()
                .get_mut(&key)
                .and_then(VecDeque::pop_front);
            let result = match next {
                Some(Ok((status, body))) => Ok(HttpResponse { status, body }),
                Some(Err(e)) => Err(e),
                None => Err(FetchError::Network("no route (test)".into())),
            };
            async move { result }
        }
    }
}

/// A scripted streamer for tests: serves bytes (optionally with range support), can break a
/// stream part-way, and records every request as `(url, from)`.
#[cfg(test)]
pub mod testing_stream {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct File {
        body: Vec<u8>,
        ranges: bool,
        break_after: Option<usize>,
        status: Option<u16>,
    }

    #[derive(Default)]
    pub struct FakeStreamer {
        files: Mutex<HashMap<String, File>>,
        calls: Mutex<Vec<(String, u64)>>,
    }

    impl FakeStreamer {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn add(&self, url: &str, body: impl AsRef<[u8]>) {
            self.files.lock().unwrap().insert(
                url.to_owned(),
                File {
                    body: body.as_ref().to_vec(),
                    ranges: true,
                    break_after: None,
                    status: None,
                },
            );
        }
        /// The next stream of this URL breaks after `n` body bytes (once).
        pub fn break_once_after(&self, url: &str, n: usize) {
            self.files.lock().unwrap().get_mut(url).unwrap().break_after = Some(n);
        }
        pub fn answer_status(&self, url: &str, status: u16) {
            self.files.lock().unwrap().get_mut(url).unwrap().status = Some(status);
        }
        pub fn clear_status(&self, url: &str) {
            self.files.lock().unwrap().get_mut(url).unwrap().status = None;
        }
        pub fn ignore_ranges(&self, url: &str) {
            self.files.lock().unwrap().get_mut(url).unwrap().ranges = false;
        }
        pub fn replace_body(&self, url: &str, body: impl AsRef<[u8]>) {
            self.files.lock().unwrap().get_mut(url).unwrap().body = body.as_ref().to_vec();
        }
        pub fn calls(&self) -> Vec<(String, u64)> {
            self.calls.lock().unwrap().clone()
        }
        pub fn count(&self, url: &str) -> usize {
            self.calls().iter().filter(|c| c.0 == url).count()
        }
        pub fn total_calls(&self) -> usize {
            self.calls().len()
        }
    }

    impl Streamer for FakeStreamer {
        #[allow(clippy::manual_async_fn)] // the trait spells out the borrowed lifetime
        fn stream<'a>(
            &'a self,
            url: &'a Url,
            from: u64,
            sink: &'a mut dyn StreamSink,
        ) -> impl Future<Output = Result<(), FetchError>> + Send + 'a {
            async move {
                self.calls.lock().unwrap().push((url.to_string(), from));
                let (body, status, break_after) = {
                    let mut files = self.files.lock().unwrap();
                    let Some(f) = files.get_mut(url.as_str()) else {
                        return Err(FetchError::Network("no route (test)".into()));
                    };
                    if let Some(s) = f.status {
                        return Err(FetchError::Status(s));
                    }
                    let (start, status) = if from > 0 && f.ranges {
                        if from as usize > f.body.len() {
                            return Err(FetchError::Status(416));
                        }
                        (from as usize, 206)
                    } else {
                        (0, 200)
                    };
                    (f.body[start..].to_vec(), status, f.break_after.take())
                };
                sink.begin(&StreamHead {
                    status,
                    content_length: Some(body.len() as u64),
                })
                .map_err(|e| FetchError::Local(e.to_string()))?;
                let mut sent = 0usize;
                for chunk in body.chunks(64) {
                    if let Some(limit) = break_after {
                        if sent >= limit {
                            return Err(FetchError::Network("connection reset (test)".into()));
                        }
                    }
                    sink.chunk(chunk)
                        .map_err(|e| FetchError::Local(e.to_string()))?;
                    sent += chunk.len();
                    tokio::task::yield_now().await;
                }
                if break_after.is_some_and(|limit| sent <= limit) {
                    // A limit at or beyond the body size still counts as a broken connection.
                    return Err(FetchError::Network("connection reset (test)".into()));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download_policy::DEFAULT_ALLOWED_HOSTS;

    #[tokio::test]
    async fn the_real_fetcher_refuses_disallowed_urls_before_touching_the_network() {
        let f = HttpFetcher::new(DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS)).unwrap();
        for u in [
            "http://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
            "https://example.org/x",
            "https://127.0.0.1/x",
            "https://user:pw@piston-meta.mojang.com/x",
            "https://piston-meta.mojang.com:8443/x",
        ] {
            let r = f.get(&Url::parse(u).unwrap(), 1024).await;
            assert!(matches!(r, Err(FetchError::Policy(_))), "{u}: {r:?}");
        }
    }

    #[tokio::test]
    async fn the_auth_transport_obeys_the_same_policy_and_redacts_requests() {
        let t = HttpFetcher::for_auth(DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS)).unwrap();
        let req = |u: &str| HttpRequest {
            method: Method::Post,
            url: Url::parse(u).unwrap(),
            headers: vec![("Authorization", "Bearer SECRET-TOKEN".into())],
            body: b"refresh_token=SECRET-TOKEN".to_vec(),
            max_response_bytes: 1024,
        };
        for u in [
            "http://login.microsoftonline.com/x",
            "https://example.org/x",
        ] {
            let r = t.send(req(u)).await;
            assert!(matches!(r, Err(FetchError::Policy(_))), "{u}");
        }
        let shown = format!("{:?}", req("https://login.microsoftonline.com/x"));
        assert!(!shown.contains("SECRET-TOKEN"), "{shown}");
    }

    #[tokio::test]
    async fn fake_fetcher_enforces_the_size_cap_and_records_calls() {
        let f = testing::FakeFetcher::new();
        f.ok("https://a.example/x", vec![0u8; 100]);
        let url = Url::parse("https://a.example/x").unwrap();
        assert!(f.get(&url, 1000).await.is_ok());
        assert!(matches!(
            f.get(&url, 10).await,
            Err(FetchError::TooLarge(10))
        ));
        assert_eq!(f.calls().len(), 2);
    }
}
