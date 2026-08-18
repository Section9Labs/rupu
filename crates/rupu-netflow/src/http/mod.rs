//! The instrumented client — the ONE door for rupu's outbound HTTP.
//!
//! `clippy.toml` denies `reqwest::Client::new`, `Client::default`,
//! `ClientBuilder::new`, `ClientBuilder::default`, and — the actual
//! bypass point — `ClientBuilder::build` everywhere except here (Task 11).
//! `Client::builder()` itself is deliberately allowed everywhere: every
//! legitimate caller of `client_with` must build a tuned `ClientBuilder`
//! first, so banning that call would produce a false positive at every
//! correct call site instead of catching the one real escape hatch.

pub mod middleware;
pub mod resolver;

use crate::ctx::FlowCtx;
use crate::sink::FlowSink;
use middleware::NetflowMiddleware;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use resolver::RecordingResolver;
use std::sync::Arc;

/// Build an instrumented client bound to an explicit sink.
///
/// There is deliberately no process-global sink. Resolution is per-run: a
/// long-lived process (`rupu session`, `rupu cp serve`) hosts many runs,
/// and a `OnceLock` would pin the first run's sink and route every later
/// run's flows into the first run's ledger and transcript. Callers thread
/// their run's sink through `provider_factory`.
///
/// This is the one legitimate `ClientBuilder::build()` call site in the
/// repo, confined to this file — the actual choke point the rest of
/// Task 11's lint protects.
#[allow(clippy::disallowed_methods)]
pub fn client_with(
    ctx: FlowCtx,
    builder: reqwest::ClientBuilder,
    sink: Arc<dyn FlowSink>,
) -> reqwest::Result<ClientWithMiddleware> {
    let resolver = RecordingResolver::default();
    let inner = builder.dns_resolver(Arc::new(resolver.clone())).build()?;
    Ok(ClientBuilder::new(inner)
        .with(NetflowMiddleware {
            ctx,
            sink,
            resolver,
        })
        .build())
}
