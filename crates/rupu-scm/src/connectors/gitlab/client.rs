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
use reqwest::{Client, Method};
use rupu_providers::concurrency;
use tokio::sync::Semaphore;

use crate::error::{classify_scm_error, ScmError};
use crate::platform::Platform;

const CACHE_CAP: usize = 256;
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 5;

#[allow(dead_code)]
#[derive(Clone)]
pub struct GitlabClient {
    pub(crate) http: Client,
    pub(crate) base_url: String,
    pub(crate) token: String,
    semaphore: Arc<Semaphore>,
    cache: Arc<Mutex<LruCache<String, CacheEntry>>>,
}

struct CacheEntry {
    etag: String,
    body: serde_json::Value,
    inserted_at: Instant,
}

impl GitlabClient {
    pub fn new(token: String, base_url: Option<String>, max_concurrency: Option<usize>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://gitlab.com/api/v4".to_string());
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest builder");
        let semaphore = concurrency::semaphore_for("gitlab", max_concurrency);
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(CACHE_CAP).unwrap(),
        )));
        Self {
            http,
            base_url: base,
            token,
            semaphore,
            cache,
        }
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

    /// Issue a JSON GET against `<base_url><path>` with the GitLab
    /// `PRIVATE-TOKEN` auth header, honoring the LRU ETag cache and
    /// classifying error responses via `classify_scm_error(Platform::Gitlab, ...)`.
    pub async fn get_json(&self, path: &str) -> Result<serde_json::Value, ScmError> {
        let url = format!("{}{}", self.base_url, path);
        let cache_key = url.clone();
        let cached = self.cache_get(&cache_key);

        let mut req = self.http.get(&url).header("PRIVATE-TOKEN", &self.token);
        if let Some((etag, _)) = &cached {
            req = req.header("If-None-Match", etag);
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() || e.is_connect() {
                ScmError::Network(anyhow::anyhow!("gitlab transport: {e}"))
            } else {
                ScmError::Transient(anyhow::anyhow!("gitlab: {e}"))
            }
        })?;

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
            .http
            .request(method, &url)
            .header("PRIVATE-TOKEN", &self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() || e.is_connect() {
                    ScmError::Network(anyhow::anyhow!("gitlab transport: {e}"))
                } else {
                    ScmError::Transient(anyhow::anyhow!("gitlab: {e}"))
                }
            })?;
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
        let resp = self
            .http
            .get(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() || e.is_connect() {
                    ScmError::Network(anyhow::anyhow!("gitlab transport: {e}"))
                } else {
                    ScmError::Transient(anyhow::anyhow!("gitlab: {e}"))
                }
            })?;
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

fn backoff(attempt: u32) -> Duration {
    let base = 2u64.saturating_pow(attempt).min(60);
    let jitter_ms: u64 = (rand::random::<u8>() as u64) % 500;
    Duration::from_millis(base * 1000 + jitter_ms)
}
