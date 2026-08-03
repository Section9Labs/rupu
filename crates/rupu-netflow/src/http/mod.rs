//! The instrumented client — the ONE door for rupu's outbound HTTP.
//!
//! `clippy.toml` denies `reqwest::Client::new` and `::builder` outside
//! this crate (Task 11), so this factory cannot be bypassed by accident.

pub mod middleware;
pub mod resolver;

use crate::ctx::FlowCtx;
use crate::record::FlowId;
use crate::sink::{FlowSink, NullSink};
use middleware::NetflowMiddleware;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use resolver::RecordingResolver;
use std::sync::{Arc, OnceLock};

static SINK: OnceLock<Arc<dyn FlowSink>> = OnceLock::new();

/// Install the process-wide sink. First call wins; later calls are
/// ignored so a test or a second runtime cannot silently re-point it.
pub fn init(sink: Arc<dyn FlowSink>) {
    let _ = SINK.set(sink);
}

/// The process-wide sink, or a no-op one when capture was never
/// initialised (a library consumer, or a unit test).
pub fn sink() -> Arc<dyn FlowSink> {
    SINK.get().cloned().unwrap_or_else(|| Arc::new(NullSink))
}

/// An instrumented client with default settings.
///
/// If `client_from` fails, the fallback below deliberately does NOT call
/// `reqwest::Client::new()` (nor `Client::default()`, which delegates to
/// it) — both panic internally via `.expect("Client::new()")` on build
/// failure. The only fallible step upstream is TLS backend / resolver
/// initialisation inside `ClientBuilder::build()`, which is a
/// process-wide condition, not something our added `.dns_resolver(..)`
/// call can trigger on its own (it is a plain, infallible config
/// assignment) — so a second attempt is genuinely no more likely to
/// succeed than the first. We still make it, using the non-panicking
/// `.build()`, in case the environment changed between calls. There is
/// no infallible `reqwest::Client` constructor in reqwest's public API,
/// so a true double failure here means the process cannot do TLS at
/// all — every other HTTP-using subsystem in rupu would already be
/// broken identically. That residual, environment-wide case is the one
/// spot this function cannot avoid a panic; it never manufactures one
/// on its own.
pub fn client(ctx: FlowCtx) -> ClientWithMiddleware {
    client_from(ctx, default_builder()).unwrap_or_else(|_| {
        let bare = default_builder()
            .build()
            .expect("reqwest TLS backend failed to initialise; no HTTP client can be built");
        ClientBuilder::new(bare).build()
    })
}

/// Start from a caller-tuned builder — timeouts, `http1_only`, proxies.
/// The resolver is installed here, so callers must not set their own.
pub fn client_from(
    ctx: FlowCtx,
    builder: reqwest::ClientBuilder,
) -> reqwest::Result<ClientWithMiddleware> {
    client_with(ctx, builder, sink())
}

/// As `client_from`, with an explicit sink. Used by tests.
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

#[allow(clippy::disallowed_methods)]
fn default_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
}

/// Finalize a streamed body recorded earlier at header time.
///
/// The caller minted the `FlowId` and attached it with
/// `RequestBuilder::with_extension`, so it already knows which record to
/// close. See the spec §7.2 — the alternative is estimating byte counts,
/// which this design does not do.
pub async fn complete(id: FlowId, bytes_in: u64, duration_ms: u64) {
    sink().complete(id, bytes_in, duration_ms).await;
}
