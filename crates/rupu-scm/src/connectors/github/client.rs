//! Internal HTTP client for the GitHub adapter.
//!
//! Wraps `octocrab` with:
//!   - per-platform Semaphore (shared with other platforms via
//!     `rupu_providers::concurrency::semaphore_for("github", _)`)
//!   - in-memory LRU ETag cache for `get_*` responses
//!   - retry-with-backoff for RateLimited / Transient (Plan 1's
//!     ProviderError-style table, but classified via classify_scm_error)
//!   - hardened error mapping at the boundary

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use octocrab::Octocrab;
use rupu_providers::concurrency;
use tokio::sync::Semaphore;
use url::Url;

use crate::client_options::{CloneProtocol, ScmClientOptions};
use crate::error::{classify_scm_error, ScmError};
use crate::platform::Platform;

const CACHE_CAP: usize = 256;
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 5;

#[derive(Clone)]
pub struct GithubClient {
    pub(crate) inner: Octocrab,
    pub(crate) token: String,
    graphql_url: String,
    semaphore: Arc<Semaphore>,
    cache: Arc<Mutex<LruCache<String, CacheEntry>>>,
    /// `[scm.github].timeout_ms` (ISSUES.md I-17). Applied to octocrab AND to
    /// the two ad-hoc reqwest clients below, which previously had none.
    timeout: Duration,
    /// `[scm.github].clone_protocol` (ISSUES.md I-16).
    clone_protocol: CloneProtocol,
    /// Host this client talks to. Derived once from `graphql_url` so a
    /// GitHub Enterprise install is recorded as itself, not hardcoded as
    /// `api.github.com`. Computed once at construction — `with_retry_octocrab`
    /// needs a `&str` per attempt and must not derive or leak one per call.
    host_label: String,
    /// `octocrab` owns its own hyper/tower stack, so it never goes through
    /// `rupu_netflow::http::client_with` for its main traffic — this is the
    /// only client field that observes it, via the `Fidelity::Coarse` record
    /// `retry_loop` writes directly. Also reused by `fetch_token_scopes` /
    /// `graphql_json`, the two ad-hoc reqwest paths on this type.
    sink: Arc<dyn rupu_netflow::FlowSink>,
}

struct CacheEntry {
    etag: String,
    body: serde_json::Value,
    inserted_at: Instant,
}

impl GithubClient {
    /// Convenience constructor with default `[scm.github]` options.
    pub fn new(
        token: String,
        base_url: Option<String>,
        max_concurrency: Option<usize>,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        Self::with_options(
            token,
            &ScmClientOptions {
                base_url,
                max_concurrency,
                ..Default::default()
            },
            sink,
        )
    }

    /// Build from resolved `[scm.github]` options — `base_url`,
    /// `max_concurrency`, `timeout_ms` (I-17), `clone_protocol` (I-16).
    pub fn with_options(
        token: String,
        opts: &ScmClientOptions,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        let graphql_url =
            graphql_url_for(opts.base_url.as_deref()).expect("valid github graphql url");
        let host_label = Url::parse(&graphql_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "api.github.com".to_string());
        let mut builder = Octocrab::builder()
            .personal_token(token.clone())
            .set_connect_timeout(Some(opts.timeout))
            .set_read_timeout(Some(opts.timeout));
        if let Some(url) = opts.base_url.clone() {
            builder = builder.base_uri(url).expect("valid base_url");
        }
        let inner = builder.build().expect("octocrab builder");
        let semaphore = concurrency::semaphore_for("github", opts.max_concurrency);
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(CACHE_CAP).unwrap(),
        )));
        Self {
            inner,
            token,
            graphql_url,
            semaphore,
            cache,
            timeout: opts.timeout,
            clone_protocol: opts.clone_protocol,
            host_label,
            sink,
        }
    }

    /// The host this client talks to (e.g. `api.github.com`, or a GitHub
    /// Enterprise install's domain). Computed once at construction from
    /// `graphql_url` — see the `host_label` field doc.
    pub(crate) fn host_label(&self) -> &str {
        &self.host_label
    }

    /// The configured clone protocol, read by `GithubRepoConnector::clone_to`.
    pub fn clone_protocol(&self) -> CloneProtocol {
        self.clone_protocol
    }

    /// The configured HTTP deadline.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Acquire a permit from the per-platform semaphore.
    pub async fn permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("github semaphore closed")
    }

    /// Cache lookup for a `get_*` style URL key. Returns the cached
    /// JSON value if fresh AND the ETag was reused on a 304.
    pub fn cache_get(&self, key: &str) -> Option<(String, serde_json::Value)> {
        let mut guard = self.cache.lock().ok()?;
        let entry = guard.get(key)?;
        if entry.inserted_at.elapsed() > CACHE_TTL {
            return None;
        }
        Some((entry.etag.clone(), entry.body.clone()))
    }

    pub fn cache_put(&self, key: String, etag: String, body: serde_json::Value) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.put(
                key,
                CacheEntry {
                    etag,
                    body,
                    inserted_at: Instant::now(),
                },
            );
        }
    }

    /// Fetch the comma-separated list of OAuth scopes that GitHub
    /// reports for the current token. Reads `X-OAuth-Scopes` from a
    /// cheap `GET /user` call (one round-trip; the response body is
    /// discarded). Returns `None` on network / 4xx / parse failure
    /// — diagnostics built on top of this should treat absence as
    /// "unknown" rather than "definitely missing scopes".
    ///
    /// Used by `rupu repos list` to surface a missing-`repo`-scope
    /// warning when private repos are unexpectedly absent. octocrab's
    /// typed builder API doesn't expose response headers cleanly, so
    /// this goes through reqwest directly.
    pub async fn fetch_token_scopes(&self) -> Option<Vec<String>> {
        // Infallible w.r.t. this method's `Option` return — a client-build
        // failure collapses to `None`, the same as every other failure path
        // below (network error, non-2xx, missing header). No fallback to an
        // uninstrumented client.
        let http = rupu_netflow::http::client_with(
            rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm("github".into())),
            reqwest::Client::builder().timeout(self.timeout),
            self.sink.clone(),
        )
        .ok()?;
        let resp = http
            .get("https://api.github.com/user")
            .header(reqwest::header::USER_AGENT, "rupu/0")
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let raw = resp.headers().get("X-OAuth-Scopes")?.to_str().ok()?;
        Some(
            raw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        )
    }

    /// Execute a GitHub GraphQL query against the authenticated API.
    /// Returns the `data` object on success.
    pub async fn graphql_json(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, ScmError> {
        let query = query.to_string();
        let variables = variables.clone();
        let url = self.graphql_url.clone();
        let token = self.token.clone();

        self.with_retry(|| {
            let query = query.clone();
            let variables = variables.clone();
            let url = url.clone();
            let token = token.clone();
            async move {
                let _permit = self.permit().await;
                // `graphql_json` already returns `Result<_, ScmError>`, so a
                // client-build failure propagates as a real error instead of
                // falling back to an uninstrumented client.
                let http = rupu_netflow::http::client_with(
                    rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm("github".into())),
                    reqwest::Client::builder().timeout(self.timeout),
                    self.sink.clone(),
                )
                .map_err(|e| {
                    ScmError::Transient(anyhow::anyhow!("github graphql client build: {e}"))
                })?;
                let resp = http
                    .post(&url)
                    .header(reqwest::header::USER_AGENT, "rupu/0")
                    .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                    .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
                    .json(&serde_json::json!({
                        "query": query,
                        "variables": variables,
                    }))
                    .send()
                    .await
                    .map_err(|e| {
                        ScmError::Network(anyhow::anyhow!("github graphql request: {e}"))
                    })?;

                let status = resp.status().as_u16();
                let headers = resp.headers().clone();
                let body: serde_json::Value = resp.json().await.map_err(|e| {
                    ScmError::Transient(anyhow::anyhow!("github graphql decode: {e}"))
                })?;

                if status >= 400 {
                    let message = graphql_error_message(&body)
                        .unwrap_or_else(|| "github graphql request failed".to_string());
                    return Err(classify_scm_error(
                        Platform::Github,
                        status,
                        &message,
                        &headers,
                    ));
                }

                if body
                    .get("errors")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|errors| !errors.is_empty())
                {
                    let message = graphql_error_message(&body)
                        .unwrap_or_else(|| "github graphql returned errors".to_string());
                    return Err(ScmError::Transient(anyhow::anyhow!(
                        "github graphql: {message}"
                    )));
                }

                Ok(body.get("data").cloned().unwrap_or(serde_json::Value::Null))
            }
        })
        .await
    }

    /// Run `f` with retry-with-backoff. Recoverable RateLimited /
    /// Transient errors are retried up to MAX_RETRIES with exponential
    /// jitter (cap 60s). Unrecoverable errors abort immediately.
    ///
    /// Does NOT emit a netflow record per attempt. The one caller that
    /// uses this directly — `graphql_json` — already sends its request
    /// through `rupu_netflow::http::client_from`, which records an
    /// accurate `Fidelity::Http` entry per real send via the netflow
    /// middleware. Recording a second, less-precise `Coarse` entry here
    /// for the same connection would duplicate data, not complement it.
    /// Octocrab-backed callers, whose transport genuinely is invisible to
    /// us, use `with_retry_octocrab` below instead.
    pub async fn with_retry<F, Fut, T>(&self, f: F) -> Result<T, ScmError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ScmError>>,
    {
        self.retry_loop(f, false).await
    }

    /// As `with_retry`, but also records one `Fidelity::Coarse` flow (see
    /// `coarse_flow`) per attempt — every octocrab-backed call site uses
    /// this, since `octocrab` owns its own hyper/tower stack and this
    /// retry boundary is the only place we can observe those attempts at
    /// all. One record per attempt, not per logical call: a retried call
    /// is a real repeated connection.
    pub async fn with_retry_octocrab<F, Fut, T>(&self, f: F) -> Result<T, ScmError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ScmError>>,
    {
        self.retry_loop(f, true).await
    }

    async fn retry_loop<F, Fut, T>(&self, mut f: F, record_coarse: bool) -> Result<T, ScmError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ScmError>>,
    {
        let mut attempt: u32 = 0;
        loop {
            let attempt_started = Instant::now();
            let result = f().await;
            if record_coarse {
                self.sink
                    .record(coarse_flow(
                        self.host_label(),
                        "*",
                        result.as_ref().err(),
                        attempt_started.elapsed().as_millis() as u64,
                    ))
                    .await;
            }
            match result {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let is_retryable =
                        matches!(&e, ScmError::RateLimited { .. } | ScmError::Transient(_));
                    if !is_retryable || attempt >= MAX_RETRIES {
                        return Err(e);
                    }
                    let delay = match &e {
                        ScmError::RateLimited {
                            retry_after: Some(d),
                        } => *d,
                        _ => backoff(attempt),
                    };
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }
}

/// Build a `Coarse`-fidelity record for one `octocrab` attempt.
///
/// `octocrab` owns its own hyper/tower stack, so bytes, peer IP and raw
/// status are genuinely unavailable here. They stay `None` — the
/// `Fidelity::Coarse` marker is how a view knows the difference between
/// "zero bytes" and "we could not see the bytes". `method` and `path` are
/// `"*"` for the same reason: `with_retry_octocrab` is generic over the
/// closure and does not know the verb or the route it retried.
///
/// `error` is `None` on success, `Some(&ScmError)` on failure — NOT a
/// plain `bool`. Every `ScmError` variant except `Network` was already
/// classified from an HTTP status code (`classify_scm_error` /
/// `classify_octocrab_error`'s `GitHub { .. }` arm): `RateLimited` is a
/// 403/429, `NotFound` a 404, `Forbidden` a header-less 403, and so on.
/// Only `Network` (octocrab's `Hyper`/`Service` variants) represents a
/// genuine transport fault where no HTTP response was ever received.
/// Collapsing that distinction to a bare `bool` (the pre-fix signature)
/// mapped every failure — including a plain 404 or a rate limit — to
/// `Outcome::TransportError`, which is invariant 3 inverted: a real HTTP
/// exchange gets recorded as a network fault that never happened.
pub fn coarse_flow(
    host: &str,
    path: &str,
    error: Option<&ScmError>,
    duration_ms: u64,
) -> rupu_netflow::FlowRecord {
    rupu_netflow::FlowRecord {
        id: rupu_netflow::FlowId::new(),
        ts: chrono::Utc::now(),
        ctx: rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm("github".into())),
        fidelity: rupu_netflow::Fidelity::Coarse,
        method: "*".into(),
        scheme: "https".into(),
        host: host.to_string(),
        port: 443,
        path: path.to_string(),
        peer_ip: None,
        resolved_ips: Vec::new(),
        http_version: None,
        status: None,
        outcome: match error {
            None => rupu_netflow::Outcome::Ok,
            Some(ScmError::Network(_)) => rupu_netflow::Outcome::TransportError,
            Some(_) => rupu_netflow::Outcome::HttpError,
        },
        error: None,
        bytes_out: None,
        bytes_in: None,
        // No body was observed here by construction — `octocrab` never
        // hands this boundary a byte count either way — so asserting
        // completeness would claim coverage this record does not have
        // (invariant 3). See `record.rs`'s `body_complete` doc: it is
        // only ever `true` for a record that has an actual observed
        // count behind it (a `Content-Length` or a `LedgerLine::Complete`
        // fold), neither of which applies to a Coarse record.
        body_complete: false,
        ttfb_ms: None,
        duration_ms: Some(duration_ms),
    }
}

fn graphql_url_for(base_url: Option<&str>) -> Result<String, url::ParseError> {
    let Some(base_url) = base_url else {
        return Ok("https://api.github.com/graphql".to_string());
    };
    let mut url = Url::parse(base_url)?;
    let path = url.path().trim_end_matches('/');
    let graphql_path = if path == "/api/v3" {
        "/api/graphql".to_string()
    } else if path == "/api" {
        "/graphql".to_string()
    } else if path.is_empty() || path == "/" {
        if url.domain() == Some("api.github.com") {
            "/graphql".to_string()
        } else {
            "/api/graphql".to_string()
        }
    } else {
        format!("{path}/graphql")
    };
    url.set_path(&graphql_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn graphql_error_message(body: &serde_json::Value) -> Option<String> {
    body.get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            body.get("errors")
                .and_then(serde_json::Value::as_array)
                .and_then(|errors| errors.first())
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

fn backoff(attempt: u32) -> Duration {
    let base = 2u64.saturating_pow(attempt).min(60);
    let jitter_ms: u64 = (rand::random::<u8>() as u64) % 500;
    Duration::from_millis(base * 1000 + jitter_ms)
}

/// Classify an octocrab error into the rupu ScmError vocabulary.
///
/// Special-case 403: `classify_scm_error`'s 403 branch needs the
/// response headers to disambiguate "missing scope" (non-retryable)
/// from "rate limited" (retryable). octocrab's `GitHubError` doesn't
/// hand us the headers, so we look at the message body instead — and
/// when in doubt, default to a NON-retryable `Forbidden`. The previous
/// behavior unconditionally classified header-less 403s as
/// `RateLimited`, which sent the retry loop into a 60s+ exponential
/// backoff for what was actually a permanent permission denial. The
/// symptom: `scm.files.read` against an SSO-gated org repo would
/// stall for ~120s per call before surfacing the error.
pub fn classify_octocrab_error(err: octocrab::Error) -> ScmError {
    use octocrab::Error as OE;
    match err {
        OE::GitHub { source, .. } => {
            let status = source.status_code.as_u16();
            if status == 403 {
                let msg = source.message.to_lowercase();
                let looks_rate_limited = msg.contains("rate limit")
                    || msg.contains("api rate")
                    || msg.contains("abuse detection")
                    || msg.contains("secondary rate");
                if looks_rate_limited {
                    return ScmError::RateLimited { retry_after: None };
                }
                // Default 403 → permanent denial. SSO-gated repos,
                // missing-scope tokens, and "you don't have permission"
                // all land here; none should be retried.
                return ScmError::Forbidden {
                    platform: Platform::Github.as_str().into(),
                    message: source.message,
                };
            }
            classify_scm_error(
                Platform::Github,
                status,
                &source.message,
                &Default::default(),
            )
        }
        OE::Hyper { .. } | OE::Service { .. } => {
            ScmError::Network(anyhow::anyhow!("github transport: {err}"))
        }
        other => ScmError::Transient(anyhow::anyhow!("github: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    #[test]
    fn graphql_url_defaults_to_public_github() {
        assert_eq!(
            graphql_url_for(None).unwrap(),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            graphql_url_for(Some("https://api.github.com")).unwrap(),
            "https://api.github.com/graphql"
        );
    }

    #[test]
    fn graphql_url_maps_enterprise_rest_root() {
        assert_eq!(
            graphql_url_for(Some("https://ghe.example.com/api/v3")).unwrap(),
            "https://ghe.example.com/api/graphql"
        );
    }

    /// I-16 / I-17: `[scm.github]` reaches the client that acts on it.
    #[tokio::test]
    async fn platform_config_reaches_the_client() {
        crate::install_default_crypto_provider();
        let cfg = rupu_config::ScmPlatformConfig {
            timeout_ms: Some(4_000),
            clone_protocol: Some("ssh".into()),
            ..Default::default()
        };
        let opts = ScmClientOptions::from_platform_config(Some(&cfg));
        let c = GithubClient::with_options("ghp".into(), &opts, Arc::new(rupu_netflow::NullSink));
        assert_eq!(c.clone_protocol(), CloneProtocol::Ssh);
        assert_eq!(c.timeout(), Duration::from_millis(4_000));

        // No [scm.github] table ⇒ documented defaults.
        let d = GithubClient::with_options(
            "ghp".into(),
            &ScmClientOptions::from_platform_config(None),
            Arc::new(rupu_netflow::NullSink),
        );
        assert_eq!(d.clone_protocol(), CloneProtocol::Https);
        assert_eq!(d.timeout(), Duration::from_millis(30_000));
    }

    /// The clone path asks the client for the protocol and builds the URL from
    /// it — `ssh` yields the ssh form, nothing else changed.
    #[tokio::test]
    async fn configured_ssh_protocol_produces_an_ssh_clone_url() {
        crate::install_default_crypto_provider();
        let cfg = rupu_config::ScmPlatformConfig {
            clone_protocol: Some("ssh".into()),
            ..Default::default()
        };
        let c = GithubClient::with_options(
            "ghp_secret".into(),
            &ScmClientOptions::from_platform_config(Some(&cfg)),
            Arc::new(rupu_netflow::NullSink),
        );
        let url = crate::client_options::clone_url(
            "github.com",
            "section9labs",
            "rupu",
            c.clone_protocol(),
            &c.token,
        );
        assert_eq!(url, "git@github.com:section9labs/rupu.git");
    }

    #[tokio::test]
    async fn graphql_json_posts_query_and_returns_data() {
        crate::install_default_crypto_provider();
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/graphql")
                .header("authorization", "Bearer ghp_test")
                .json_body(json!({
                    "query": "query Test($id: ID!) { node(id: $id) { __typename } }",
                    "variables": { "id": "node-1" }
                }));
            then.status(200).json_body(json!({
                "data": {
                    "node": { "__typename": "Issue" }
                }
            }));
        });

        let client = GithubClient::new(
            "ghp_test".into(),
            Some(server.base_url()),
            Some(2),
            Arc::new(rupu_netflow::NullSink),
        );
        let data = client
            .graphql_json(
                "query Test($id: ID!) { node(id: $id) { __typename } }",
                json!({ "id": "node-1" }),
            )
            .await
            .expect("graphql ok");

        mock.assert();
        assert_eq!(data["node"]["__typename"], "Issue");
    }
}
