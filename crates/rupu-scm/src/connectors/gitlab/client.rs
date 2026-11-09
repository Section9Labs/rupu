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
    operator_http: reqwest_middleware::ClientWithMiddleware,
    forge_http: reqwest_middleware::ClientWithMiddleware,
    discussion_http: reqwest_middleware::ClientWithMiddleware,
    pub(crate) base_url: String,
    review: crate::review_transport::ReviewTransport,
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
    pub(crate) async fn forge_session(
        &self,
    ) -> Result<crate::forge_write::ForgeSession, crate::forge_write::ForgeError> {
        use crate::forge_write::ForgeError;
        let origin = crate::operator_issues::safe_origin(&self.base_url)
            .ok_or_else(ForgeError::unsupported)?;
        // Normal OAuth refresh is allowed here, before the durable dispatch marker.
        // The frozen request never uses send_authed or a 401 replacement retry.
        let (token, credential_epoch) = self.token.token_snapshot().await.map_err(|_| {
            ForgeError::fixed(
                "account_unavailable",
                "The selected credential is unavailable",
            )
        })?;
        let mut auth = reqwest::header::HeaderMap::new();
        auth.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| ForgeError::unsupported())?,
        );
        let generation = self.review.identity_epoch(credential_epoch);
        let review = self.review.clone();
        let tokens = self.token.clone();
        crate::forge_write::ForgeSession::new(
            crate::IssueTracker::Gitlab,
            origin,
            generation,
            self.forge_http.clone(),
            auth,
            Arc::new(move || review.identity_epoch(tokens.credential_epoch())),
        )
    }

    pub(crate) async fn discussion_session(
        &self,
    ) -> Result<crate::issue_discussion::Session, ScmError> {
        let origin = crate::operator_issues::safe_origin(&self.base_url)
            .ok_or_else(crate::issue_discussion::unsupported)?;
        let web = origin
            .strip_suffix("/api/v4")
            .ok_or_else(crate::issue_discussion::unsupported)?;
        let graphql = format!("{web}/api/graphql");
        // Fail closed if preemptive or concurrent replacement occurs while
        // acquiring auth. Never label old auth with a newly sampled epoch.
        let epoch = self.review_identity_epoch();
        let token = self
            .token
            .token()
            .await
            .map_err(|_| crate::issue_discussion::failed())?;
        if epoch != self.review_identity_epoch() {
            return Err(crate::issue_discussion::changed());
        }
        let mut auth = reqwest::header::HeaderMap::new();
        auth.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| crate::issue_discussion::unsupported())?,
        );
        let review = self.review.clone();
        let tokens = self.token.clone();
        crate::issue_discussion::Session::new(
            crate::IssueTracker::Gitlab,
            origin,
            graphql,
            epoch,
            self.discussion_http.clone(),
            auth,
            Arc::new(move || review.identity_epoch(tokens.credential_epoch())),
        )
    }

    pub fn review_identity_epoch(&self) -> String {
        self.review.identity_epoch(self.token.credential_epoch())
    }
    pub fn review_api_profile(&self) -> String {
        self.review.api_profile()
    }
    pub fn review_matches_origin(&self, origin: &str) -> bool {
        self.review.matches_origin(origin)
    }
    pub async fn review_get(
        &self,
        path: &str,
    ) -> Result<crate::review_transport::ReviewResponse, ScmError> {
        self.review_get_bounded(path, crate::review_transport::MAX_REVIEW_BODY)
            .await
    }
    pub async fn review_get_bounded(
        &self,
        path: &str,
        max_bytes: usize,
    ) -> Result<crate::review_transport::ReviewResponse, ScmError> {
        crate::review_transport::validate_budget(max_bytes)?;
        let (request, url) = self.review.get_request(path)?;
        let _permit = self.permit().await;
        let token = self
            .token
            .token()
            .await
            .map_err(|_| crate::review_transport::sanitized_auth(Platform::Gitlab))?;
        let response = request
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| crate::review_transport::transport_failure())?;
        self.review.finish(response, &url, max_bytes).await
    }

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
        let forge_http = crate::forge_write::http("gitlab", sink.clone())
            .expect("gitlab operator write client build");
        let discussion_http = crate::issue_discussion::http("gitlab", sink.clone())
            .expect("gitlab discussion client build");
        let operator_http = crate::operator_issues::http("gitlab", sink.clone())
            .expect("gitlab operator client build");
        let review = crate::review_transport::ReviewTransport::new(
            Platform::Gitlab,
            &base,
            opts.timeout,
            sink.clone(),
        );
        let http = opts
            .netflow_client("gitlab", sink)
            .expect("gitlab netflow client build");
        let semaphore = concurrency::semaphore_for("gitlab", opts.max_concurrency);
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(CACHE_CAP).unwrap(),
        )));
        Self {
            review,
            http,
            operator_http,
            forge_http,
            discussion_http,
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

    pub(crate) async fn operator_json(&self, path: &str) -> Result<serde_json::Value, ScmError> {
        let origin = crate::operator_issues::safe_origin(&self.base_url)
            .ok_or_else(crate::operator_issues::rejected)?;
        let response = self
            .send_authed(|| self.operator_http.get(format!("{origin}{path}")))
            .await?;
        crate::operator_issues::json(response).await
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
