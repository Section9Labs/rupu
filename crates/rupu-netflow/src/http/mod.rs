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
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

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

/// Connection-level settings for a [`shared_client`]. Two callers asking for
/// the same `Transport` share one connection pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Transport {
    /// HTTP/1.1 only (no h2 negotiation).
    pub http1_only: bool,
    /// Connect + read (inactivity) timeout.
    pub timeout: Option<Duration>,
}

impl Transport {
    fn builder(self) -> reqwest::ClientBuilder {
        let mut b = reqwest::Client::builder();
        if self.http1_only {
            b = b.http1_only();
        }
        if let Some(t) = self.timeout {
            b = b.connect_timeout(t).read_timeout(t);
        }
        b
    }
}

type SharedInner = (reqwest::Client, RecordingResolver);

/// One pool per transport AND per tokio runtime (`None`: built outside any).
/// A pooled connection's dispatch task lives on the runtime that opened it:
/// handed to a caller on another runtime, its request waits on a task
/// nothing drives, and fails with hyper's `DispatchGone` ("runtime dropped
/// the dispatch task") once that runtime shuts down. A process that does its
/// HTTP on one runtime — every rupu binary — still has one pool per
/// transport; each `#[tokio::test]` gets its own.
type PoolKey = (Transport, Option<tokio::runtime::Id>);

fn shared_pool() -> &'static Mutex<HashMap<PoolKey, SharedInner>> {
    static POOL: OnceLock<Mutex<HashMap<PoolKey, SharedInner>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Like [`client_with`], but the underlying `reqwest::Client` — and so its
/// connection pool — is shared process-wide per [`Transport`] (and per tokio
/// runtime: see `PoolKey`; send on the runtime that built the client).
/// Only the netflow middleware (`ctx` + `sink`) is per call, so flows are
/// still attributed to the caller's run.
///
/// Why: providers are built once per agent run. With a private client each,
/// every concurrent agent in a fan-out held its own idle keep-alive sockets
/// for its whole lifetime (between turns, while tools ran), so descriptor
/// use scaled with the number of agents and exhausted `RLIMIT_NOFILE`.
/// Shared, sockets scale with requests actually in flight (bounded by the
/// per-provider semaphores) and TLS handshakes are reused across runs.
///
/// Caveat inherited from [`RecordingResolver`]: `resolved_ips` is the most
/// recent resolution for the host across all sharers; `peer_ip` stays exact.
/// A build failure is not cached — the next call retries.
#[allow(clippy::disallowed_methods)]
pub fn shared_client(
    ctx: FlowCtx,
    transport: Transport,
    sink: Arc<dyn FlowSink>,
) -> reqwest::Result<ClientWithMiddleware> {
    let key = (
        transport,
        tokio::runtime::Handle::try_current().ok().map(|h| h.id()),
    );
    let (inner, resolver) = {
        let mut pool = shared_pool().lock().unwrap_or_else(|e| e.into_inner());
        match pool.get(&key) {
            Some(entry) => entry.clone(),
            None => {
                let resolver = RecordingResolver::default();
                let inner = transport
                    .builder()
                    .dns_resolver(Arc::new(resolver.clone()))
                    .build()?;
                pool.insert(key, (inner.clone(), resolver.clone()));
                (inner, resolver)
            }
        }
    };
    Ok(ClientBuilder::new(inner)
        .with(NetflowMiddleware {
            ctx,
            sink,
            resolver,
        })
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_runtime_gets_its_own_pool() {
        // A timeout unique to this test keeps its pool entries private to it.
        let transport = Transport {
            http1_only: true,
            timeout: Some(Duration::from_millis(16_180)),
        };
        let entries = || {
            let pool = shared_pool().lock().unwrap();
            pool.keys().filter(|(t, _)| *t == transport).count()
        };
        let build = || {
            let ctx = FlowCtx::system(crate::Origin::Provider("test".into()));
            shared_client(ctx, transport, Arc::new(crate::NullSink)).unwrap();
        };
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
        };
        let (a, b) = (runtime(), runtime());
        a.block_on(async {
            build();
            build();
        });
        assert_eq!(entries(), 1, "callers on one runtime share one pool");
        b.block_on(async { build() });
        assert_eq!(entries(), 2, "another runtime gets a pool of its own");
    }
}
