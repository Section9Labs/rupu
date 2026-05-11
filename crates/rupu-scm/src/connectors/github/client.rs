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
}

struct CacheEntry {
    etag: String,
    body: serde_json::Value,
    inserted_at: Instant,
}

impl GithubClient {
    pub fn new(token: String, base_url: Option<String>, max_concurrency: Option<usize>) -> Self {
        let graphql_url = graphql_url_for(base_url.as_deref()).expect("valid github graphql url");
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
            graphql_url,
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
        let http = reqwest::Client::builder().build().ok()?;
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
                let http = reqwest::Client::builder().build().map_err(|e| {
                    ScmError::Network(anyhow::anyhow!("github graphql client: {e}"))
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

    #[tokio::test]
    async fn graphql_json_posts_query_and_returns_data() {
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

        let client = GithubClient::new("ghp_test".into(), Some(server.base_url()), Some(2));
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
