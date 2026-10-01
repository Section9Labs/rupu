//! Internal HTTP client for the GitLab adapter.
//!
//! Mirrors github::client::GithubClient line-for-line in shape:
//!   - per-platform Semaphore via concurrency::semaphore_for("gitlab", _)
//!   - in-memory LRU ETag cache for `get_*` responses (TTL 5min)
//!   - retry-with-backoff for RateLimited / Transient classifications
//!   - boundary-level mapping to ScmError via classify_scm_error
//!
//! GitLab vocabulary differences vs GitHub the higher layers handle:
//!   - "project" ↔ Repo (translation via translate_project_to_repo)
//!   - "merge request" ↔ Pr
//!   - "owner/repo" can be a nested namespace path

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use reqwest::Method;
use rupu_providers::concurrency;
use tokio::sync::Semaphore;

use crate::client_options::{CloneProtocol, ScmClientOptions};
use crate::error::{classify_scm_error, ScmError};
use crate::platform::Platform;
use crate::token::TokenSource;

const CACHE_CAP: usize = 256;
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 5;

#[allow(dead_code)]
#[derive(Clone)]
pub struct GitlabClient {
    pub(crate) http: reqwest_middleware::ClientWithMiddleware,
    pub(crate) base_url: String,
    /// Asked before every request: an OAuth token is refreshed through the
    /// credential store when it nears expiry (see [`TokenSource`]).
    token: Arc<TokenSource>,
    semaphore: Arc<Semaphore>,
    cache: Arc<Mutex<LruCache<String, CacheEntry>>>,
    /// `[scm.gitlab].clone_protocol` (ISSUES.md I-16).
    clone_protocol: CloneProtocol,
}

struct CacheEntry {
    etag: String,
    body: serde_json::Value,
    inserted_at: Instant,
}

impl GitlabClient {
    /// Convenience constructor with default `[scm.gitlab]` options.
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

    /// Build from resolved `[scm.gitlab]` options — `base_url`,
    /// `max_concurrency`, `timeout_ms` (I-17, previously hardcoded 30s),
    /// `clone_protocol` (I-16) — with a token used as-is.
    pub fn with_options(
        token: String,
        opts: &ScmClientOptions,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        Self::with_token_source(
            Arc::new(TokenSource::fixed(Platform::Gitlab, token)),
            opts,
            sink,
        )
    }

    /// [`Self::with_options`] with the credential behind `token`, which
    /// keeps an OAuth token current.
    pub fn with_token_source(
        token: Arc<TokenSource>,
        opts: &ScmClientOptions,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        let base = opts
            .base_url
            .clone()
            .unwrap_or_else(|| "https://gitlab.com/api/v4".to_string());
        // `with_options` is infallible (`-> Self`); a client-build failure here
        // preserves the deleted `http::client()` fallback's panic-on-failure
        // behaviour rather than silently falling back to an uninstrumented
        // client (there is no such fallback any more).
        let http = opts
            .netflow_client("gitlab", sink)
            .expect("gitlab netflow client build");
        let semaphore = concurrency::semaphore_for("gitlab", opts.max_concurrency);
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(CACHE_CAP).unwrap(),
        )));
        Self {
            http,
            base_url: base,
            token,
            semaphore,
            cache,
            clone_protocol: opts.clone_protocol,
        }
    }

    /// The token to send now (refreshed first if it is an expiring OAuth
    /// token). Read by every request here and by `clone_to`, which puts it
    /// in the clone URL.
    pub(crate) async fn access_token(&self) -> Result<String, ScmError> {
        self.token.token().await
    }

    /// Send the request `build` makes, with the current token as
    /// `Authorization: Bearer` (the one header GitLab accepts for OAuth
    /// tokens and access tokens alike; `PRIVATE-TOKEN` rejects OAuth
    /// tokens). A 401 for an OAuth token is retried once with its
    /// replacement from the store — a `rupu auth login` or another
    /// process's refresh since this connector last read it.
    async fn send_authed(
        &self,
        build: impl Fn() -> reqwest_middleware::RequestBuilder,
    ) -> Result<reqwest::Response, ScmError> {
        let token = self.access_token().await?;
        let resp = build().bearer_auth(&token).send().await;
        let resp = resp.map_err(transport_error)?;
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        match self.token.replacement_for(&token).await? {
            Some(fresh) => build()
                .bearer_auth(&fresh)
                .send()
                .await
                .map_err(transport_error),
            None => Ok(resp),
        }
    }

    /// The API host when it isn't gitlab.com (a self-managed instance, or a
    /// `base_url` that doesn't parse), for `clone_to`, whose URLs only know
    /// gitlab.com.
    pub(crate) fn self_managed_host(&self) -> Option<String> {
        let host = url::Url::parse(&self.base_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
        match host.as_deref() {
            Some("gitlab.com") => None,
            Some(other) => Some(other.to_string()),
            None => Some(self.base_url.clone()),
        }
    }

    /// The configured clone protocol, read by `GitlabRepoConnector::clone_to`.
    pub fn clone_protocol(&self) -> CloneProtocol {
        self.clone_protocol
    }

    /// Acquire a permit from the per-platform semaphore.
    pub async fn permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("gitlab semaphore closed")
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

    /// Run `f` with retry-with-backoff. Recoverable RateLimited /
    /// Transient errors are retried up to MAX_RETRIES with exponential
    /// jitter (cap 60s). Unrecoverable errors abort immediately.
    pub async fn with_retry<F, Fut, T>(&self, mut f: F) -> Result<T, ScmError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ScmError>>,
    {
        let mut attempt: u32 = 0;
        loop {
            match f().await {
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

    /// Issue an authenticated JSON GET against `<base_url><path>`
    /// ([`Self::send_authed`]), honoring the LRU ETag cache and
    /// classifying error responses via
    /// `classify_scm_error(Platform::Gitlab, ...)`.
    pub async fn get_json(&self, path: &str) -> Result<serde_json::Value, ScmError> {
        let url = format!("{}{}", self.base_url, path);
        let cache_key = url.clone();
        let cached = self.cache_get(&cache_key);

        let resp = self
            .send_authed(|| {
                let req = self.http.get(&url);
                match &cached {
                    Some((etag, _)) => req.header("If-None-Match", etag),
                    None => req,
                }
            })
            .await?;

        let status = resp.status().as_u16();
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let headers = resp.headers().clone();

        // 304 → return cached body if we have one.
        if status == 304 {
            if let Some((_, body)) = cached {
                return Ok(body);
            }
            // Fall through to re-fetch (cache miss after If-None-Match
            // is unusual but possible if the cache evicted between the
            // get and the request).
        }

        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_scm_error(
                Platform::Gitlab,
                status,
                &body,
                &headers,
            ));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("gitlab json deser: {e}")))?;
        if let Some(et) = etag {
            self.cache_put(cache_key, et, body.clone());
        }
        Ok(body)
    }

    /// Non-cached write paths (POST/PUT/DELETE). Same retry/classify
    /// shape; no cache lookup or storage.
    pub async fn write_json(
        &self,
        method: Method,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, ScmError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .send_authed(|| self.http.request(method.clone(), &url).json(&body))
            .await?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_scm_error(
                Platform::Gitlab,
                status,
                &body,
                &headers,
            ));
        }
        let body_json: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok(body_json)
    }

    /// Fetch a non-JSON text body (e.g. the raw-diff endpoint).
    pub async fn get_text(&self, path: &str) -> Result<String, ScmError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.send_authed(|| self.http.get(&url)).await?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_scm_error(
                Platform::Gitlab,
                status,
                &body,
                &headers,
            ));
        }
        resp.text()
            .await
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("gitlab text: {e}")))
    }
}

fn transport_error(e: reqwest_middleware::Error) -> ScmError {
    if e.is_timeout() || e.is_connect() {
        ScmError::Network(anyhow::anyhow!("gitlab transport: {e}"))
    } else {
        ScmError::Transient(anyhow::anyhow!("gitlab: {e}"))
    }
}

fn backoff(attempt: u32) -> Duration {
    let base = 2u64.saturating_pow(attempt).min(60);
    let jitter_ms: u64 = (rand::random::<u8>() as u64) % 500;
    Duration::from_millis(base * 1000 + jitter_ms)
}
