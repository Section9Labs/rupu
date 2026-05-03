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

use crate::error::{classify_scm_error, ScmError};
use crate::platform::Platform;

const CACHE_CAP: usize = 256;
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 5;

#[derive(Clone)]
pub struct GithubClient {
    pub(crate) inner: Octocrab,
    pub(crate) token: String,
    semaphore: Arc<Semaphore>,
    cache: Arc<Mutex<LruCache<String, CacheEntry>>>,
}

struct CacheEntry {
    etag: String,
    body: serde_json::Value,
    inserted_at: Instant,
}

impl GithubClient {
    pub fn new(token: String, base_url: Option<String>, max_concurrency: Option<usize>) -> Self {
        let mut builder = Octocrab::builder().personal_token(token.clone());
        if let Some(url) = base_url {
            builder = builder.base_uri(url).expect("valid base_url");
        }
        let inner = builder.build().expect("octocrab builder");
        let semaphore = concurrency::semaphore_for("github", max_concurrency);
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(CACHE_CAP).unwrap(),
        )));
        Self {
            inner,
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
}

fn backoff(attempt: u32) -> Duration {
    let base = 2u64.saturating_pow(attempt).min(60);
    let jitter_ms: u64 = (rand::random::<u8>() as u64) % 500;
    Duration::from_millis(base * 1000 + jitter_ms)
}

/// Classify an octocrab error into the rupu ScmError vocabulary.
pub fn classify_octocrab_error(err: octocrab::Error) -> ScmError {
    use octocrab::Error as OE;
    match err {
        OE::GitHub { source, .. } => {
            // octocrab's GitHubError carries status + message; we don't
            // get headers easily, so missing-scope can't be detected
            // here. Fall back to status-only classification.
            let status = source.status_code.as_u16();
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
