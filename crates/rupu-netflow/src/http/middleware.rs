//! The capture middleware.
//!
//! Hard rule: this code must never panic and never fail a request. Every
//! fallible step degrades to a less complete record.

use crate::ctx::FlowCtx;
use crate::http::resolver::RecordingResolver;
use crate::record::{Fidelity, FlowId, FlowRecord, Outcome};
use crate::sink::FlowSink;
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MwResult};
use std::sync::Arc;
use std::time::Instant;

pub struct NetflowMiddleware {
    pub(crate) ctx: FlowCtx,
    pub(crate) sink: Arc<dyn FlowSink>,
    pub(crate) resolver: RecordingResolver,
}

#[async_trait::async_trait]
impl Middleware for NetflowMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> MwResult<Response> {
        // The caller may mint the id itself (via `with_extension`) so it
        // can finalize a streamed body later — the presence of a
        // caller-supplied id is itself the signal that the caller intends
        // to drain and complete the body out-of-band (Task 9), so this
        // middleware must not mark the body complete at header time even
        // if the response happens to carry a known `Content-Length`.
        // Otherwise generate one and account the body normally.
        let caller_id = extensions.get::<FlowId>().copied();
        let caller_supplied = caller_id.is_some();
        let id = caller_id.unwrap_or_else(FlowId::new);

        let url = req.url().clone();
        let method = req.method().to_string();
        let host = url.host_str().unwrap_or_default().to_string();
        let port = url.port_or_known_default().unwrap_or(0);
        let scheme = url.scheme().to_string();
        // Query-stripped. Query strings routinely carry tokens.
        let path = url.path().to_string();
        // No body (GET, HEAD, …) is a genuinely known zero, not an unknown
        // — `req.body()` returning `None` means "there is no body", which
        // is exactly as observed as any other count. Only an unbuffered
        // streaming body (`body()` is `Some` but `as_bytes()` is `None`
        // because it isn't held in memory) is genuinely unknown. Collapsing
        // both cases to `None` would turn a known zero into "we could not
        // see it" — invariant 3, inverted — and `host_rollup`'s (correct)
        // collapse-on-unknown rule would then permanently blank
        // `bytes_out` for any host that ever served a GET.
        let bytes_out = match req.body() {
            None => Some(0),
            Some(b) => b.as_bytes().map(|s| s.len() as u64),
        };

        let started = Instant::now();
        let result = next.run(req, extensions).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let mut record = FlowRecord {
            id,
            ts: chrono::Utc::now(),
            ctx: self.ctx.clone(),
            fidelity: Fidelity::Http,
            method,
            scheme,
            resolved_ips: self.resolver.answers_for(&host),
            host,
            port,
            path,
            peer_ip: None,
            http_version: None,
            status: None,
            outcome: Outcome::TransportError,
            error: None,
            bytes_out,
            bytes_in: None,
            body_complete: false,
            ttfb_ms: Some(elapsed_ms),
            duration_ms: None,
        };

        match &result {
            Ok(resp) => {
                record.status = Some(resp.status().as_u16());
                record.http_version = Some(format!("{:?}", resp.version()));
                record.peer_ip = resp.remote_addr().map(|a| a.ip());
                record.outcome = if resp.status().is_success() || resp.status().is_redirection() {
                    Outcome::Ok
                } else {
                    Outcome::HttpError
                };
                // Known length means the body is fully accounted for now;
                // a streamed body (signalled by a caller-supplied id) is
                // finalized later via `complete` instead.
                if !caller_supplied {
                    if let Some(len) = resp.content_length() {
                        record.bytes_in = Some(len);
                        record.body_complete = true;
                        record.duration_ms = Some(elapsed_ms);
                    }
                }
            }
            Err(e) => {
                // Type-based, NOT substring matching on the message.
                // `reqwest::Error`'s Display appends " for url (<url>)",
                // so matching on "timeout" would misclassify an ordinary
                // connection failure to any host whose URL contains that
                // word.
                record.outcome = if e.is_timeout() {
                    Outcome::Timeout
                } else {
                    Outcome::TransportError
                };
                record.error = Some(e.to_string());
                record.duration_ms = Some(elapsed_ms);
                record.body_complete = true;
            }
        }

        // Spawned rather than awaited inline so a panicking sink can never
        // propagate into the request path — invariant 1, non-negotiable
        // per spec §10 ("A panic in the middleware must not propagate into
        // the request path"). `FanoutSink` isolates panics per-child the
        // same way, but this middleware is the one place EVERY sink runs
        // through regardless of whether it's wrapped in a `FanoutSink`, so
        // the isolation has to live here to hold unconditionally.
        let sink = self.sink.clone();
        if tokio::task::spawn(async move { sink.record(record).await })
            .await
            .is_err()
        {
            tracing::warn!(
                "netflow sink panicked while recording a flow; request unaffected, record lost"
            );
        }
        result
    }
}
