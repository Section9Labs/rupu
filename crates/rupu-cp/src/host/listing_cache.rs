//! Short-lived, single-flight cache of one SSH host's LISTING commands (spec
//! `docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md`
//! §7.2).
//!
//! SSH listings are all-or-nothing (`rupu run list --limit 10000`,
//! `rupu autoflow history`), and every `RemoteExec::run` is a fresh `ssh`
//! with a full handshake. The web loads each host independently and pages it
//! with offset cursors, and the dashboard, the Activity tables and ⌘K can
//! all ask one host at once. Without this cache, each of those requests
//! re-ran the whole remote listing.
//!
//! - An `Ok` listing younger than `ttl` is served as is. An older one waits
//!   for a fresh fetch. Stale data is never served.
//! - Concurrent callers for one key share ONE in-flight fetch, including its
//!   error. An error is never stored past that fetch.
//! - [`ListingCache::clear`], which every mutating connector method calls via
//!   [`ClearOnDrop`], drops everything. A fetch that STARTED before the clear
//!   never stores its (pre-mutation) answer.

use crate::host::connector::HostConnectorError;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a listing is served from the cache.
///
/// List responses carry no capture time, so the web strip measures a host's
/// age from when the browser received the answer. That makes this TTL the
/// most a cached answer can be older than it looks. It matches the strip's
/// `LIVE_THRESHOLD_MS` (5 s, `web/src/components/dashboard/HostFreshnessStrip.tsx`).
/// Raise it only together with a server-stamped capture time (spec §11).
pub const LISTING_TTL: Duration = Duration::from_secs(5);

/// One listing's rows, shared between every caller that received it.
pub type Rows = Arc<Vec<serde_json::Value>>;

type Fetch = Shared<BoxFuture<'static, Result<Rows, HostConnectorError>>>;

#[derive(Default)]
struct State {
    /// Bumped by `clear`. A fetch started under an older epoch never stores.
    epoch: u64,
    /// Identity of each in-flight fetch, so only ITS first waiter retires it.
    next_id: u64,
    fresh: HashMap<String, (Instant, Rows)>,
    inflight: HashMap<String, (u64, u64, Fetch)>,
}

/// See the module docs.
pub struct ListingCache {
    ttl: Duration,
    state: Mutex<State>,
}

impl Default for ListingCache {
    fn default() -> Self {
        Self::new(LISTING_TTL)
    }
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl ListingCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            state: Mutex::default(),
        }
    }

    /// Drop every listing, and make every fetch already in flight unable to
    /// store its answer.
    pub fn clear(&self) {
        let mut s = lock(&self.state);
        s.epoch += 1;
        s.fresh.clear();
        s.inflight.clear();
    }

    /// Clear when the returned guard drops, i.e. when the mutating call
    /// holding it returns, success or failure (a failed mutation may still
    /// have partly applied on the remote).
    pub fn clear_on_drop(&self) -> ClearOnDrop<'_> {
        ClearOnDrop(self)
    }

    /// `key`'s rows: fresh from the cache, joined onto an in-flight fetch, or
    /// fetched now via `fetch`. `fetch` is only called when neither exists.
    pub async fn get<F, Fut>(&self, key: &str, fetch: F) -> Result<Rows, HostConnectorError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Vec<serde_json::Value>, HostConnectorError>> + Send + 'static,
    {
        let (epoch, id, fut) = {
            let mut s = lock(&self.state);
            if let Some((at, rows)) = s.fresh.get(key) {
                if at.elapsed() < self.ttl {
                    return Ok(Arc::clone(rows));
                }
            }
            if let Some(joined) = s.inflight.get(key).cloned() {
                joined
            } else {
                s.next_id += 1;
                let (epoch, id) = (s.epoch, s.next_id);
                let fut: Fetch = fetch().map(|r| r.map(Arc::new)).boxed().shared();
                s.inflight.insert(key.to_string(), (epoch, id, fut.clone()));
                (epoch, id, fut)
            }
        };
        let out = fut.await;
        let mut s = lock(&self.state);
        // The first waiter back retires the entry. On success, and only if no
        // `clear` happened since the fetch started, it stores the rows.
        if matches!(s.inflight.get(key), Some((_, cur, _)) if *cur == id) {
            s.inflight.remove(key);
            if let Ok(rows) = &out {
                if s.epoch == epoch {
                    s.fresh
                        .insert(key.to_string(), (Instant::now(), Arc::clone(rows)));
                }
            }
        }
        out
    }
}

/// Clears its [`ListingCache`] when dropped. See [`ListingCache::clear_on_drop`].
pub struct ClearOnDrop<'a>(&'a ListingCache);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    type Out = Result<Vec<serde_json::Value>, HostConnectorError>;

    fn rows(n: u64) -> Vec<serde_json::Value> {
        (0..n).map(|i| serde_json::json!({ "id": i })).collect()
    }

    /// A fetch that counts its calls, sleeps `delay_ms`, then returns `out`.
    fn counted(
        calls: &Arc<AtomicU32>,
        out: Out,
        delay_ms: u64,
    ) -> impl FnOnce() -> BoxFuture<'static, Out> + Send {
        let calls = Arc::clone(calls);
        move || {
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                out
            }
            .boxed()
        }
    }

    fn down() -> HostConnectorError {
        HostConnectorError::Unreachable("connection timed out".into())
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_fetch() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let (a, b) = tokio::join!(
            cache.get("k", counted(&calls, Ok(rows(2)), 30)),
            cache.get("k", counted(&calls, Ok(rows(9)), 30)),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(a.unwrap().len(), 2);
        assert_eq!(
            b.unwrap().len(),
            2,
            "the second caller joined the first fetch"
        );
    }

    #[tokio::test]
    async fn concurrent_callers_share_an_error() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let (a, b) = tokio::join!(
            cache.get("k", counted(&calls, Err(down()), 30)),
            cache.get("k", counted(&calls, Err(down()), 30)),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(a.is_err() && b.is_err());
    }

    #[tokio::test]
    async fn a_fresh_listing_is_served_without_refetching() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_expired_listing_is_refetched() {
        let cache = ListingCache::new(Duration::from_millis(20));
        let calls = Arc::new(AtomicU32::new(0));
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_error_is_not_stored() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        assert!(cache
            .get("k", counted(&calls, Err(down()), 0))
            .await
            .is_err());
        let ok = cache
            .get("k", counted(&calls, Ok(rows(3)), 0))
            .await
            .unwrap();
        assert_eq!(ok.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn clear_drops_a_fresh_listing() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        cache.clear();
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_fetch_started_before_clear_never_stores() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let (first, ()) = tokio::join!(cache.get("k", counted(&calls, Ok(rows(1)), 50)), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cache.clear();
        },);
        first.unwrap();
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the pre-clear answer must not have been stored"
        );
    }

    #[tokio::test]
    async fn clear_on_drop_clears() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        {
            let _guard = cache.clear_on_drop();
        }
        cache
            .get("k", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn keys_are_independent() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache
            .get("a", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        cache
            .get("b", counted(&calls, Ok(rows(1)), 0))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
