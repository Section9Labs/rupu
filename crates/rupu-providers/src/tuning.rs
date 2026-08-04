//! Per-provider runtime tuning — the consumer side of `[providers.<name>]`.
//!
//! ISSUES.md I-9/I-10/I-11/I-12: `timeout_ms`, `max_retries`, `max_concurrency`
//! and `org_id` were declared in `rupu-config`, documented with specific
//! defaults, and editable in CP Settings, yet nothing ever read them. This
//! module is where those four values land, and every decision they drive is a
//! pure function so it can be asserted without a live provider.
//!
//! `rupu-providers` deliberately does NOT depend on `rupu-config` (hexagonal
//! rule 1 — the runtime knows traits, not config files). `rupu-runtime`'s
//! `provider_factory::provider_tuning` is the single adapter that turns a
//! `rupu_config::ProviderConfig` into a [`ProviderTuning`].

use std::time::Duration;

/// Per-request inactivity timeout when `[providers.<name>].timeout_ms` is
/// absent. Matches the documented default in `docs/providers.md`.
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Retries (in addition to the first attempt) on a retryable provider error
/// when `[providers.<name>].max_retries` is absent.
///
/// One, not five. Until I-10 the only retry loop in the codebase was
/// Anthropic's `MAX_RATE_LIMIT_RETRIES = 1`, chosen deliberately so
/// `ProviderRouter` can fail over to another vendor quickly rather than
/// spending ~30s of exponential backoff on a single rate-limited provider.
/// `docs/providers.md` claimed `5`; the doc was corrected to match the code
/// rather than the reverse, because raising the default would slow every
/// cross-provider fallback. Users who want a bigger budget set the key.
pub const DEFAULT_MAX_RETRIES: u32 = 1;

/// Initial backoff before the first retry; doubles per attempt.
pub const DEFAULT_INITIAL_BACKOFF_MS: u64 = 2_000;

/// Resolved knobs for one provider. Always fully populated — `None` in config
/// has already been collapsed into the documented default here, so consumers
/// never re-implement defaulting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTuning {
    /// Connect + read (inactivity) timeout for this provider's HTTP client.
    pub timeout: Duration,
    /// Retries after the first attempt on a retryable error.
    pub max_retries: u32,
    /// Permits in this provider's semaphore.
    pub max_concurrency: usize,
    /// OpenAI organization scope (`OpenAI-Organization` header).
    pub org_id: Option<String>,
    /// Vertex region. Accepted and carried, but no shipped Gemini client
    /// targets a regional Vertex endpoint — see `docs/providers.md`.
    pub region: Option<String>,
}

impl Default for ProviderTuning {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            max_retries: DEFAULT_MAX_RETRIES,
            max_concurrency: crate::concurrency::default_permits(""),
            org_id: None,
            region: None,
        }
    }
}

impl ProviderTuning {
    /// Defaults for a named provider (its `max_concurrency` comes from the
    /// per-vendor table in [`crate::concurrency::default_permits`]).
    pub fn for_provider(provider: &str) -> Self {
        Self {
            max_concurrency: crate::concurrency::default_permits(provider),
            ..Default::default()
        }
    }

    /// This provider's process-wide semaphore, sized by `max_concurrency`.
    pub fn semaphore(&self, provider: &str) -> std::sync::Arc<tokio::sync::Semaphore> {
        crate::concurrency::semaphore_for(provider, Some(self.max_concurrency))
    }

    /// A tuned [`reqwest::ClientBuilder`] honoring [`Self::timeout`].
    ///
    /// The timeout is applied as `connect_timeout` + `read_timeout`, NOT as
    /// reqwest's total `timeout`. A total deadline would abort a legitimately
    /// long streaming generation mid-flight; an inactivity deadline only fires
    /// when the server has genuinely stopped sending. This matches the
    /// pre-existing hand-rolled `STREAM_IDLE_TIMEOUT_SECS = 120` in the
    /// Anthropic client, which is where the documented 120000 ms default
    /// comes from.
    ///
    /// Callers must never `.build()` this directly (rupu-netflow Plan 1 Task
    /// 10 / Task 11's `clippy.toml` lint) — pass it through
    /// `rupu_netflow::http::client_from` so the resulting client is
    /// instrumented, preserving every option set here.
    pub fn http_client_builder(&self) -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .connect_timeout(self.timeout)
            .read_timeout(self.timeout)
    }
}

/// `timeout_ms` → the client's inactivity deadline. Absent ⇒
/// [`DEFAULT_TIMEOUT_MS`]. `0` is treated as absent: a zero-length deadline
/// would fail every request instantly, which is never what a user means.
pub fn client_timeout(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(
        timeout_ms
            .filter(|ms| *ms > 0)
            .unwrap_or(DEFAULT_TIMEOUT_MS),
    )
}

/// `max_retries` → retries after the first attempt. Absent ⇒
/// [`DEFAULT_MAX_RETRIES`]. `Some(0)` is honored: "never retry" is a
/// legitimate request.
pub fn retry_budget(max_retries: Option<u32>) -> u32 {
    max_retries.unwrap_or(DEFAULT_MAX_RETRIES)
}

/// `max_concurrency` → semaphore permits. Absent (or 0, which would deadlock
/// every call) ⇒ the per-vendor default from
/// [`crate::concurrency::default_permits`].
pub fn concurrency_permits(provider: &str, max_concurrency: Option<usize>) -> usize {
    max_concurrency
        .filter(|n| *n > 0)
        .unwrap_or_else(|| crate::concurrency::default_permits(provider))
}

/// Backoff before retry `attempt` (0-based): 2s, 4s, 8s, …, capped at 60s.
pub fn retry_backoff(attempt: u32) -> Duration {
    let ms = DEFAULT_INITIAL_BACKOFF_MS.saturating_mul(1u64 << attempt.min(20));
    Duration::from_millis(ms.min(60_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_timeout_defaults_to_documented_120s() {
        assert_eq!(client_timeout(None), Duration::from_millis(120_000));
        assert_eq!(client_timeout(Some(5_000)), Duration::from_millis(5_000));
        // 0 would abort every request before it starts — treat as unset.
        assert_eq!(client_timeout(Some(0)), Duration::from_millis(120_000));
    }

    #[test]
    fn retry_budget_defaults_to_one_and_honors_zero() {
        assert_eq!(retry_budget(None), 1);
        assert_eq!(retry_budget(Some(5)), 5);
        assert_eq!(retry_budget(Some(0)), 0);
    }

    #[test]
    fn concurrency_permits_falls_back_to_vendor_table() {
        assert_eq!(concurrency_permits("openai", None), 8);
        assert_eq!(concurrency_permits("anthropic", None), 4);
        assert_eq!(concurrency_permits("anthropic", Some(2)), 2);
        // 0 permits would deadlock the first call.
        assert_eq!(concurrency_permits("openai", Some(0)), 8);
    }

    #[test]
    fn for_provider_uses_the_vendor_permit_table() {
        assert_eq!(ProviderTuning::for_provider("openai").max_concurrency, 8);
        assert_eq!(ProviderTuning::for_provider("anthropic").max_concurrency, 4);
    }

    #[test]
    fn retry_backoff_doubles_and_caps() {
        assert_eq!(retry_backoff(0), Duration::from_millis(2_000));
        assert_eq!(retry_backoff(1), Duration::from_millis(4_000));
        assert_eq!(retry_backoff(2), Duration::from_millis(8_000));
        assert_eq!(retry_backoff(30), Duration::from_millis(60_000));
    }

    /// I-9 at the consumer: the configured `timeout_ms` actually reaches the
    /// HTTP stack. A 60ms deadline against a server that stalls for 2s must
    /// surface a timeout, not hang.
    #[tokio::test]
    async fn configured_timeout_aborts_a_stalled_request() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/slow");
            then.status(200).delay(Duration::from_secs(2)).body("late");
        });
        let tuning = ProviderTuning {
            timeout: Duration::from_millis(60),
            ..ProviderTuning::for_provider("anthropic")
        };
        let client = tuning.http_client_builder().build().expect("client");
        let err = client
            .get(server.url("/slow"))
            .send()
            .await
            .expect_err("stalled request must not succeed");
        assert!(err.is_timeout(), "expected a timeout, got: {err}");
    }

    /// …and the same client with the documented default does NOT abort a
    /// request that answers promptly.
    #[tokio::test]
    async fn default_timeout_lets_a_prompt_response_through() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/fast");
            then.status(200).body("ok");
        });
        let client = ProviderTuning::for_provider("anthropic")
            .http_client_builder()
            .build()
            .expect("client");
        let resp = client.get(server.url("/fast")).send().await.expect("ok");
        assert_eq!(resp.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn semaphore_is_sized_by_max_concurrency() {
        let t = ProviderTuning {
            max_concurrency: 3,
            ..ProviderTuning::for_provider("tuning-test-provider")
        };
        assert_eq!(t.semaphore("tuning-test-provider").available_permits(), 3);
    }
}
