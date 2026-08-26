//! `LlmProvider` decorators that make `[providers.<name>]` tuning observable
//! on every LLM call.
//!
//! Two thin wrappers, both applied by `rupu_runtime::provider_factory`:
//!
//! * [`ThrottledProvider`] — acquires a permit from the provider's semaphore
//!   for the duration of a call (ISSUES.md I-11). Before this, `semaphore_for`
//!   had exactly two call sites, both SCM clients, so `max_concurrency` and
//!   the whole `default_permits` table were unreachable for LLM traffic.
//! * [`RetryingProvider`] — re-issues a call that failed with a retryable
//!   error, up to the configured budget (ISSUES.md I-10).
//!
//! The decorators are deliberately separate, and the nesting order is load
//! bearing: **retrying wraps throttling** —
//! `RetryingProvider(ThrottledProvider(client))`. Each attempt then acquires
//! its own permit and drops it as that attempt returns, so the exponential
//! backoff sleeps *outside* the semaphore. Nested the other way round, one
//! rate-limited call parks a permit in `tokio::time::sleep` for the entire
//! 2s/4s/8s ladder and starves every other caller of the same provider.
//! [`tests::a_backoff_sleep_does_not_hold_a_concurrency_permit`] pins the
//! property; `the_inverted_order_holds_the_permit_across_the_backoff` pins why
//! the order is not arbitrary.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;
use tracing::warn;

use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::types::{LlmRequest, LlmResponse, StreamEvent};

// ---------------------------------------------------------------------------
// Throttling (I-11)
// ---------------------------------------------------------------------------

/// Holds a permit from the provider's semaphore for the whole call.
pub struct ThrottledProvider {
    inner: Box<dyn LlmProvider>,
    semaphore: Arc<Semaphore>,
}

impl ThrottledProvider {
    pub fn new(inner: Box<dyn LlmProvider>, semaphore: Arc<Semaphore>) -> Self {
        Self { inner, semaphore }
    }

    /// Wrap `inner` with the process-wide semaphore for `provider`, sized by
    /// `tuning.max_concurrency`.
    pub fn wrap(
        inner: Box<dyn LlmProvider>,
        provider: &str,
        tuning: &crate::tuning::ProviderTuning,
    ) -> Self {
        Self::new(inner, tuning.semaphore(provider))
    }
}

#[async_trait]
impl LlmProvider for ThrottledProvider {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ProviderError::Other(anyhow::anyhow!("provider semaphore closed: {e}")))?;
        self.inner.send(request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ProviderError::Other(anyhow::anyhow!("provider semaphore closed: {e}")))?;
        self.inner.stream(request, on_event).await
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    fn provider_id(&self) -> ProviderId {
        self.inner.provider_id()
    }

    // I-76: forwarded now that `LlmProvider: Send + Sync` (see provider.rs)
    // makes it possible to call the inner boxed provider's real
    // `list_models` through the trait object at all — see
    // `tests::throttled_provider_forwards_list_models_through_a_boxed_decorator`.
    async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
        self.inner.list_models().await
    }
}

// ---------------------------------------------------------------------------
// Retrying (I-10)
// ---------------------------------------------------------------------------

/// True when re-issuing the identical request could plausibly succeed:
/// rate limits, 5xx, and transport-level failures. A 4xx other than 429, an
/// auth failure, or a malformed request is permanent — retrying it just burns
/// the user's time.
pub fn is_retryable(e: &ProviderError) -> bool {
    match e {
        ProviderError::RateLimited { .. } | ProviderError::Transient(_) | ProviderError::Http(_) => {
            true
        }
        ProviderError::Api { status, .. } => *status == 429 || *status == 529 || *status >= 500,
        _ => false,
    }
}

/// Ceiling on a server-supplied `Retry-After` (I-83). Chosen to match the
/// ceiling `tuning::retry_backoff` already imposes on its own computed
/// ladder (2s, 4s, 8s, …, capped at 60s): a well-behaved API's rate-limit
/// window shouldn't need to outrun the worst case we'd already wait on our
/// own backoff, and a hostile or misconfigured `Retry-After: 86400` must not
/// be able to hang a run for a day.
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(60);

/// Extracts `retry_after` from a `RateLimited` error, if any.
fn retry_after_of(e: &ProviderError) -> Option<Duration> {
    match e {
        ProviderError::RateLimited { retry_after } => *retry_after,
        _ => None,
    }
}

/// The delay to actually sleep before the next attempt: the server's
/// `Retry-After` (capped) when the failed response carried one, otherwise
/// the computed exponential backoff. The server knows its own rate-limit
/// window better than a fixed ladder does.
fn effective_delay(computed: Duration, retry_after: Option<Duration>) -> Duration {
    match retry_after {
        Some(d) => d.min(RETRY_AFTER_CAP),
        None => computed,
    }
}

/// Re-issues a failed call up to `max_retries` times with exponential
/// backoff — or the server's `Retry-After` when the error carried one
/// (I-83).
pub struct RetryingProvider {
    inner: Box<dyn LlmProvider>,
    max_retries: u32,
    backoff: fn(u32) -> Duration,
}

impl RetryingProvider {
    pub fn new(inner: Box<dyn LlmProvider>, max_retries: u32) -> Self {
        Self {
            inner,
            max_retries,
            backoff: crate::tuning::retry_backoff,
        }
    }

    /// Swap the backoff schedule. Tests use this to assert attempt counts
    /// without sleeping out a real 2s/4s/8s ladder.
    pub fn with_backoff(mut self, backoff: fn(u32) -> Duration) -> Self {
        self.backoff = backoff;
        self
    }

    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }
}

#[async_trait]
impl LlmProvider for RetryingProvider {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let mut attempt = 0u32;
        loop {
            match self.inner.send(request).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if attempt >= self.max_retries || !is_retryable(&e) {
                        return Err(e);
                    }
                    let delay = effective_delay((self.backoff)(attempt), retry_after_of(&e));
                    warn!(
                        attempt = attempt + 1,
                        budget = self.max_retries,
                        backoff_ms = delay.as_millis() as u64,
                        error = %e,
                        "retryable provider error; retrying"
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        let mut attempt = 0u32;
        loop {
            // Only a failure that produced NO output may be retried — otherwise
            // the caller would see the first attempt's partial text followed by
            // a full second response.
            let mut emitted = false;
            let mut probe = |ev: StreamEvent| {
                emitted = true;
                on_event(ev);
            };
            let outcome = self.inner.stream(request, &mut probe).await;
            match outcome {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if emitted || attempt >= self.max_retries || !is_retryable(&e) {
                        return Err(e);
                    }
                    let delay = effective_delay((self.backoff)(attempt), retry_after_of(&e));
                    warn!(
                        attempt = attempt + 1,
                        budget = self.max_retries,
                        backoff_ms = delay.as_millis() as u64,
                        error = %e,
                        "retryable provider error before any output; re-issuing stream"
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    fn provider_id(&self) -> ProviderId {
        self.inner.provider_id()
    }

    // I-76: see the matching comment on ThrottledProvider above.
    async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
        self.inner.list_models().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tuning::ProviderTuning;
    use crate::types::{ContentBlock, Message, StopReason, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req() -> LlmRequest {
        LlmRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        }
    }

    fn ok_response() -> LlmResponse {
        LlmResponse {
            id: "msg".into(),
            model: "m".into(),
            content: vec![ContentBlock::Text { text: "ok".into() }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
        }
    }

    /// Fails `fail_times` with `RateLimited`, then succeeds. Records how many
    /// permits the shared semaphore had at the moment the call started.
    struct Probe {
        calls: Arc<AtomicUsize>,
        fail_times: usize,
        permits_seen: Arc<AtomicUsize>,
        semaphore: Option<Arc<Semaphore>>,
        emit_before_error: bool,
    }

    impl Probe {
        fn new(fail_times: usize) -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                fail_times,
                permits_seen: Arc::new(AtomicUsize::new(usize::MAX)),
                semaphore: None,
                emit_before_error: false,
            }
        }
    }

    #[async_trait]
    impl LlmProvider for Probe {
        async fn send(&mut self, _r: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            if let Some(s) = &self.semaphore {
                self.permits_seen.store(s.available_permits(), Ordering::SeqCst);
            }
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_times {
                return Err(ProviderError::RateLimited { retry_after: None });
            }
            Ok(ok_response())
        }

        async fn stream(
            &mut self,
            _r: &LlmRequest,
            on_event: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_times {
                if self.emit_before_error {
                    on_event(StreamEvent::TextDelta("partial".into()));
                }
                return Err(ProviderError::RateLimited { retry_after: None });
            }
            on_event(StreamEvent::TextDelta("ok".into()));
            Ok(ok_response())
        }

        fn default_model(&self) -> &str {
            "m"
        }

        fn provider_id(&self) -> ProviderId {
            ProviderId::Anthropic
        }

        async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
            vec![probe_model_info("probe-model")]
        }
    }

    /// Minimal fixture, mirroring `github_copilot::make_model_info`.
    fn probe_model_info(id: &str) -> crate::model_pool::ModelInfo {
        crate::model_pool::ModelInfo {
            id: id.to_string(),
            provider: ProviderId::Anthropic,
            context_window: 0,
            max_output_tokens: 0,
            capabilities: Vec::new(),
            cost: crate::model_pool::ModelCost::default(),
            status: crate::model_pool::ModelStatus::default(),
        }
    }

    fn no_sleep(_attempt: u32) -> Duration {
        Duration::from_millis(0)
    }

    // ── I-11: an LLM call actually holds a permit ────────────────────────────

    #[tokio::test]
    async fn llm_call_holds_a_permit_from_the_configured_semaphore() {
        let tuning = ProviderTuning {
            max_concurrency: 2,
            ..ProviderTuning::for_provider("throttle-test-a")
        };
        let sem = tuning.semaphore("throttle-test-a");
        assert_eq!(sem.available_permits(), 2);

        let mut probe = Probe::new(0);
        probe.semaphore = Some(sem.clone());
        let seen = probe.permits_seen.clone();

        let mut p = ThrottledProvider::wrap(Box::new(probe), "throttle-test-a", &tuning);
        p.send(&req()).await.unwrap();

        // Inside the call one permit was held: 2 configured − 1 = 1 available.
        assert_eq!(seen.load(Ordering::SeqCst), 1);
        // Released afterwards.
        assert_eq!(sem.available_permits(), 2);
    }

    #[tokio::test]
    async fn max_concurrency_one_serializes_two_llm_calls() {
        let tuning = ProviderTuning {
            max_concurrency: 1,
            ..ProviderTuning::for_provider("throttle-test-b")
        };
        let sem = tuning.semaphore("throttle-test-b");
        let mut p = ThrottledProvider::wrap(Box::new(Probe::new(0)), "throttle-test-b", &tuning);

        // Hold the only permit; the throttled call must not complete.
        let held = sem.clone().acquire_owned().await.unwrap();
        let request = req();
        let call = p.send(&request);
        tokio::pin!(call);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut call)
                .await
                .is_err(),
            "call ran while the only permit was held"
        );
        drop(held);
        call.await.unwrap();
    }

    // ── decorator ORDER: a backoff sleep must not hold a permit ──────────────

    /// `available_permits()` on `backoff-permit-test`'s semaphore, sampled from
    /// inside the retry loop's backoff hook — i.e. between two attempts, at the
    /// exact moment the stack would be sleeping. `backoff` is a plain `fn`
    /// pointer (no captures), so the sample lands in a static and the semaphore
    /// is re-fetched from the process-wide registry by name.
    static PERMITS_DURING_BACKOFF: AtomicUsize = AtomicUsize::new(usize::MAX);
    static PERMITS_DURING_INVERTED_BACKOFF: AtomicUsize = AtomicUsize::new(usize::MAX);

    fn sample_permits_then_no_sleep(_attempt: u32) -> Duration {
        let sem = crate::concurrency::semaphore_for("backoff-permit-test", Some(1));
        PERMITS_DURING_BACKOFF.store(sem.available_permits(), Ordering::SeqCst);
        Duration::from_millis(0)
    }

    fn sample_inverted_permits_then_no_sleep(_attempt: u32) -> Duration {
        let sem = crate::concurrency::semaphore_for("backoff-permit-test-inverted", Some(1));
        PERMITS_DURING_INVERTED_BACKOFF.store(sem.available_permits(), Ordering::SeqCst);
        Duration::from_millis(0)
    }

    /// The production stack built by `provider_factory::decorate`: retry
    /// OUTSIDE throttle. With `max_concurrency = 1` and a provider that 429s
    /// once, the single permit must be free again while the retry sleeps —
    /// otherwise a rate-limited call blocks every other call to that provider
    /// for the whole backoff ladder.
    #[tokio::test]
    async fn a_backoff_sleep_does_not_hold_a_concurrency_permit() {
        const NAME: &str = "backoff-permit-test";
        let tuning = ProviderTuning {
            max_concurrency: 1,
            ..ProviderTuning::for_provider(NAME)
        };
        let sem = tuning.semaphore(NAME);
        assert_eq!(sem.available_permits(), 1);

        let mut probe = Probe::new(1); // 429 once, then succeed
        probe.semaphore = Some(sem.clone());
        let seen_in_call = probe.permits_seen.clone();
        let calls = probe.calls.clone();

        let throttled = ThrottledProvider::wrap(Box::new(probe), NAME, &tuning);
        let mut p = RetryingProvider::new(Box::new(throttled), 3)
            .with_backoff(sample_permits_then_no_sleep);

        PERMITS_DURING_BACKOFF.store(usize::MAX, Ordering::SeqCst);
        p.send(&req()).await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2, "expected one retry");
        // Inside an attempt the permit IS held: 1 configured − 1 = 0 available.
        assert_eq!(seen_in_call.load(Ordering::SeqCst), 0);
        // But it is released before the backoff sleeps.
        assert_eq!(
            PERMITS_DURING_BACKOFF.load(Ordering::SeqCst),
            1,
            "the backoff slept while holding a concurrency permit"
        );
        assert_eq!(sem.available_permits(), 1, "permit leaked after the call");
    }

    /// The inverse nesting — throttle outermost — is what the factory used to
    /// build. Kept as an executable counter-example so the assertion above is
    /// visibly non-vacuous: here the permit is pinned at 0 across the backoff.
    #[tokio::test]
    async fn the_inverted_order_holds_the_permit_across_the_backoff() {
        const NAME: &str = "backoff-permit-test-inverted";
        let tuning = ProviderTuning {
            max_concurrency: 1,
            ..ProviderTuning::for_provider(NAME)
        };
        let sem = tuning.semaphore(NAME);

        let retried = RetryingProvider::new(Box::new(Probe::new(1)), 3)
            .with_backoff(sample_inverted_permits_then_no_sleep);
        let mut p = ThrottledProvider::wrap(Box::new(retried), NAME, &tuning);

        PERMITS_DURING_INVERTED_BACKOFF.store(usize::MAX, Ordering::SeqCst);
        p.send(&req()).await.unwrap();

        assert_eq!(
            PERMITS_DURING_INVERTED_BACKOFF.load(Ordering::SeqCst),
            0,
            "expected the outer throttle to pin the permit across the backoff"
        );
        assert_eq!(sem.available_permits(), 1);
    }

    // ── I-10: max_retries is the real budget ─────────────────────────────────

    #[tokio::test]
    async fn retry_budget_bounds_the_number_of_attempts() {
        // budget 0 ⇒ exactly one attempt.
        let probe = Probe::new(9);
        let calls = probe.calls.clone();
        let mut p = RetryingProvider::new(Box::new(probe), 0).with_backoff(no_sleep);
        assert!(p.send(&req()).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // budget 3 ⇒ 1 + 3 attempts, then the error surfaces.
        let probe = Probe::new(9);
        let calls = probe.calls.clone();
        let mut p = RetryingProvider::new(Box::new(probe), 3).with_backoff(no_sleep);
        assert!(p.send(&req()).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 4);

        // budget 5 with 2 transient failures ⇒ succeeds on the third attempt.
        let probe = Probe::new(2);
        let calls = probe.calls.clone();
        let mut p = RetryingProvider::new(Box::new(probe), 5).with_backoff(no_sleep);
        assert!(p.send(&req()).await.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn permanent_errors_are_not_retried() {
        struct BadRequest(Arc<AtomicUsize>);
        #[async_trait]
        impl LlmProvider for BadRequest {
            async fn send(&mut self, _r: &LlmRequest) -> Result<LlmResponse, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::BadRequest {
                    message: "max_tokens too large".into(),
                })
            }
            async fn stream(
                &mut self,
                _r: &LlmRequest,
                _e: &mut (dyn FnMut(StreamEvent) + Send),
            ) -> Result<LlmResponse, ProviderError> {
                unreachable!()
            }
            fn default_model(&self) -> &str {
                "m"
            }
            fn provider_id(&self) -> ProviderId {
                ProviderId::Anthropic
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let mut p =
            RetryingProvider::new(Box::new(BadRequest(calls.clone())), 5).with_backoff(no_sleep);
        assert!(p.send(&req()).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stream_that_already_emitted_output_is_not_re_issued() {
        let mut probe = Probe::new(1);
        probe.emit_before_error = true;
        let calls = probe.calls.clone();
        let mut p = RetryingProvider::new(Box::new(probe), 5).with_backoff(no_sleep);
        let mut seen = Vec::new();
        let r = p
            .stream(&req(), &mut |ev| seen.push(format!("{ev:?}")))
            .await;
        assert!(r.is_err(), "partial stream must surface the error");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "must not duplicate output");
    }

    #[tokio::test]
    async fn a_stream_that_failed_before_any_output_is_re_issued() {
        let probe = Probe::new(1);
        let calls = probe.calls.clone();
        let mut p = RetryingProvider::new(Box::new(probe), 5).with_backoff(no_sleep);
        let mut seen = Vec::new();
        let r = p
            .stream(&req(), &mut |ev| seen.push(format!("{ev:?}")))
            .await;
        assert!(r.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn retryability_matches_the_documented_error_classes() {
        assert!(is_retryable(&ProviderError::RateLimited { retry_after: None }));
        assert!(is_retryable(&ProviderError::Transient(anyhow::anyhow!("x"))));
        assert!(is_retryable(&ProviderError::Api {
            status: 503,
            message: String::new()
        }));
        assert!(is_retryable(&ProviderError::Api {
            status: 429,
            message: String::new()
        }));
        assert!(!is_retryable(&ProviderError::Api {
            status: 400,
            message: String::new()
        }));
        assert!(!is_retryable(&ProviderError::BadRequest {
            message: String::new()
        }));
        // Preflight failures happen before any request is attempted (e.g. a
        // workflow step whose agent file failed to load) — retrying the
        // identical request can never succeed.
        assert!(!is_retryable(&ProviderError::Preflight("x".into())));
    }

    #[test]
    fn preflight_displays_its_message_verbatim() {
        // No `provider error:`/`auth config error:` attribution — the caller
        // owns the whole message (it describes a failure that happened before
        // any provider was involved).
        let e = ProviderError::Preflight("agent not found: x".into());
        assert_eq!(e.to_string(), "agent not found: x");
    }

    // ── I-83: server-supplied Retry-After wins over the computed backoff ────

    #[test]
    fn effective_delay_prefers_server_retry_after_over_computed_backoff() {
        let computed = Duration::from_secs(2);
        let server = Some(Duration::from_secs(3));
        assert_eq!(effective_delay(computed, server), Duration::from_secs(3));
    }

    #[test]
    fn effective_delay_falls_back_to_computed_backoff_without_a_header() {
        let computed = Duration::from_secs(2);
        assert_eq!(effective_delay(computed, None), computed);
    }

    #[test]
    fn effective_delay_caps_a_hostile_retry_after() {
        let computed = Duration::from_secs(2);
        let hostile = Some(Duration::from_secs(86_400));
        assert_eq!(effective_delay(computed, hostile), RETRY_AFTER_CAP);
    }

    /// A 429 that carries `Retry-After: 3` must sleep ~3s, not the 2s the
    /// configured backoff schedule would compute for attempt 0. Uses paused
    /// tokio time so the test doesn't actually wait 3 real seconds: at t+2.5s
    /// (past the computed backoff, short of the server's) the retry must not
    /// have fired yet; at t+3.1s it must have.
    #[tokio::test(start_paused = true)]
    async fn retry_after_header_is_honored_over_computed_backoff() {
        struct RetryAfterProbe {
            calls: Arc<AtomicUsize>,
            retry_after: Option<Duration>,
        }
        #[async_trait]
        impl LlmProvider for RetryAfterProbe {
            async fn send(&mut self, _r: &LlmRequest) -> Result<LlmResponse, ProviderError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    return Err(ProviderError::RateLimited {
                        retry_after: self.retry_after,
                    });
                }
                Ok(ok_response())
            }
            async fn stream(
                &mut self,
                _r: &LlmRequest,
                _e: &mut (dyn FnMut(StreamEvent) + Send),
            ) -> Result<LlmResponse, ProviderError> {
                unreachable!()
            }
            fn default_model(&self) -> &str {
                "m"
            }
            fn provider_id(&self) -> ProviderId {
                ProviderId::Anthropic
            }
        }

        fn fixed_2s(_attempt: u32) -> Duration {
            Duration::from_secs(2)
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let mut p = RetryingProvider::new(
            Box::new(RetryAfterProbe {
                calls: calls.clone(),
                retry_after: Some(Duration::from_secs(3)),
            }),
            1,
        )
        .with_backoff(fixed_2s);

        let handle = tokio::spawn(async move {
            p.send(&req()).await.unwrap();
        });
        // Let the spawned task run up to its sleep point (registering the
        // backoff timer) before the clock moves — otherwise `advance` below
        // would find no timer yet and the "still sleeping" assertion would
        // pass vacuously regardless of which delay was actually chosen.
        tokio::task::yield_now().await;

        // Past the computed 2s backoff, short of the server's 3s: must still
        // be sleeping.
        tokio::time::advance(Duration::from_millis(2_500)).await;
        tokio::task::yield_now().await;
        assert!(
            !handle.is_finished(),
            "retried before the server's Retry-After elapsed \
             (computed backoff was used instead)"
        );

        // Past the server's 3s: must have completed.
        tokio::time::advance(Duration::from_millis(700)).await;
        handle.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// The counter-example: without a `Retry-After` header, the computed
    /// backoff is used as before — a 429 with `retry_after: None` must
    /// retry once the *computed* delay elapses, not wait for anything else.
    #[tokio::test(start_paused = true)]
    async fn no_retry_after_header_falls_back_to_computed_backoff() {
        let probe = Probe::new(1); // RateLimited { retry_after: None }, then ok
        let calls = probe.calls.clone();

        fn fixed_2s(_attempt: u32) -> Duration {
            Duration::from_secs(2)
        }

        let mut p = RetryingProvider::new(Box::new(probe), 1).with_backoff(fixed_2s);

        let handle = tokio::spawn(async move {
            p.send(&req()).await.unwrap();
        });
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_millis(1_900)).await;
        tokio::task::yield_now().await;
        assert!(
            !handle.is_finished(),
            "retried before the computed backoff elapsed"
        );

        tokio::time::advance(Duration::from_millis(200)).await;
        handle.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    // ── I-76: decorators forward list_models ─────────────────────────────

    /// Through a `Box<dyn LlmProvider>` — not the concrete inner type — a
    /// `ThrottledProvider` must return the wrapped provider's real models,
    /// not the trait's empty-vec default.
    #[tokio::test]
    async fn throttled_provider_forwards_list_models_through_a_boxed_decorator() {
        let tuning = ProviderTuning::for_provider("list-models-throttle-test");
        let throttled: Box<dyn LlmProvider> = Box::new(ThrottledProvider::wrap(
            Box::new(Probe::new(0)),
            "list-models-throttle-test",
            &tuning,
        ));
        let models = throttled.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "probe-model");
    }

    /// Same property for `RetryingProvider`.
    #[tokio::test]
    async fn retrying_provider_forwards_list_models_through_a_boxed_decorator() {
        let retrying: Box<dyn LlmProvider> =
            Box::new(RetryingProvider::new(Box::new(Probe::new(0)), 1));
        let models = retrying.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "probe-model");
    }

    /// Both decorators stacked, as `provider_factory::decorate` actually
    /// builds them (`RetryingProvider(ThrottledProvider(client))`) — the
    /// realistic factory-built shape the bug report was about.
    #[tokio::test]
    async fn stacked_decorators_forward_list_models() {
        let tuning = ProviderTuning::for_provider("list-models-stacked-test");
        let throttled =
            ThrottledProvider::wrap(Box::new(Probe::new(0)), "list-models-stacked-test", &tuning);
        let stacked: Box<dyn LlmProvider> = Box::new(RetryingProvider::new(Box::new(throttled), 1));
        let models = stacked.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "probe-model");
    }
}
