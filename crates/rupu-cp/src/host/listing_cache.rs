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
//! - A fetch whose every waiter went away (cancelled, or unwound by a panic)
//!   is dropped, never resumed for a later caller: the remote command is torn
//!   down and its old answer is never served as fresh.
//! - [`ListingCache::clear`], which every mutating connector method calls via
//!   [`ClearOnDrop`], drops everything. A fetch that STARTED before the clear
//!   never stores its (pre-mutation) answer, and a caller arriving after the
//!   clear never joins it.

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

/// One fetch in flight for a key.
struct InFlight {
    /// Never reused. Only the waiters of THIS fetch may retire or store for
    /// it, so a fetch that `clear` already dropped can never store.
    id: u64,
    fut: Fetch,
    /// Callers currently awaiting `fut`. At zero the fetch is dropped: nobody
    /// is polling it, and resuming it later would serve an old snapshot.
    waiters: usize,
}

#[derive(Default)]
struct State {
    next_id: u64,
    fresh: HashMap<String, (Instant, Rows)>,
    inflight: HashMap<String, InFlight>,
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
    /// store its answer: its entry is gone, so its id no longer matches.
    pub fn clear(&self) {
        let mut s = lock(&self.state);
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
        let (id, fut) = {
            let mut s = lock(&self.state);
            if let Some((at, rows)) = s.fresh.get(key) {
                if at.elapsed() < self.ttl {
                    return Ok(Arc::clone(rows));
                }
            }
            if let Some(joined) = s.inflight.get_mut(key) {
                joined.waiters += 1;
                (joined.id, joined.fut.clone())
            } else {
                s.next_id += 1;
                let id = s.next_id;
                let fut: Fetch = fetch().map(|r| r.map(Arc::new)).boxed().shared();
                s.inflight.insert(
                    key.to_string(),
                    InFlight {
                        id,
                        fut: fut.clone(),
                        waiters: 1,
                    },
                );
                (id, fut)
            }
        };
        // Armed before the await: it runs if this future is dropped there or
        // a panic unwinds out of it.
        let mut waiter = Waiter {
            cache: self,
            key,
            id,
            finished: false,
        };
        let out = fut.await;
        waiter.finished = true;
        let mut s = lock(&self.state);
        // The first waiter back retires the entry, and on success stores the
        // rows. If `clear` dropped the entry meanwhile (or a newer fetch
        // replaced it), the id no longer matches and nothing is stored.
        if matches!(s.inflight.get(key), Some(f) if f.id == id) {
            s.inflight.remove(key);
            if let Ok(rows) = &out {
                s.fresh
                    .insert(key.to_string(), (Instant::now(), Arc::clone(rows)));
            }
        }
        out
    }
}

/// One caller's claim on an in-flight fetch. If it is dropped before the fetch
/// answered (cancelled, or unwound by a panic), it gives the claim back, and
/// the last claim to go drops the fetch itself.
struct Waiter<'a> {
    cache: &'a ListingCache,
    key: &'a str,
    id: u64,
    finished: bool,
}

impl Drop for Waiter<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let orphaned = {
            let mut s = lock(&self.cache.state);
            let last = match s.inflight.get_mut(self.key) {
                Some(f) if f.id == self.id => {
                    f.waiters -= 1;
                    f.waiters == 0
                }
                // Retired or cleared already: nothing of ours is left.
                _ => false,
            };
            if last {
                s.inflight.remove(self.key)
            } else {
                None
            }
        };
        // Dropping the fetch tears down whatever it owns (an `ssh` child and
        // its pipes), so do it outside the lock.
        drop(orphaned);
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

    /// Orders a test against a fetch without sleeping: the fetch announces
    /// that it is running (`started`), then parks until the test opens
    /// `release`.
    #[derive(Default)]
    struct Gate {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    /// Like [`counted`], but the fetch parks on `gate` instead of sleeping.
    fn gated(
        calls: &Arc<AtomicU32>,
        gate: &Arc<Gate>,
        out: Out,
    ) -> impl FnOnce() -> BoxFuture<'static, Out> + Send {
        let (calls, gate) = (Arc::clone(calls), Arc::clone(gate));
        move || {
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                gate.started.notify_one();
                gate.release.notified().await;
                out
            }
            .boxed()
        }
    }

    fn boom() -> Out {
        panic!("the fetch blew up")
    }

    /// A fetch that counts its call, then panics.
    fn panicking(calls: &Arc<AtomicU32>) -> impl FnOnce() -> BoxFuture<'static, Out> + Send {
        let calls = Arc::clone(calls);
        move || {
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                boom()
            }
            .boxed()
        }
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
        let gate = Arc::new(Gate::default());
        let (first, ()) = tokio::join!(cache.get("k", gated(&calls, &gate, Ok(rows(1)))), async {
            // The fetch is provably running; clear, then let it finish.
            gate.started.notified().await;
            cache.clear();
            gate.release.notify_one();
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

    #[tokio::test]
    async fn a_caller_after_clear_does_not_join_the_pre_clear_fetch() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let gate = Arc::new(Gate::default());
        let (a, ()) = tokio::join!(cache.get("k", gated(&calls, &gate, Ok(rows(1)))), async {
            // A is provably in flight and parked on the gate.
            gate.started.notified().await;
            cache.clear();
            // Bounded, so a regression (B joining the parked A) fails
            // instead of hanging.
            let b = tokio::time::timeout(
                Duration::from_secs(2),
                cache.get("k", counted(&calls, Ok(rows(7)), 0)),
            )
            .await
            .expect("B must not wait on the pre-clear fetch")
            .unwrap();
            assert_eq!(b.len(), 7, "B got its own post-clear rows");
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            gate.release.notify_one();
        },);
        assert_eq!(a.unwrap().len(), 1, "A still gets its own answer");
    }

    #[tokio::test]
    async fn a_cancelled_fetch_is_not_resumed_later() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        // The only waiter goes away (a dropped HTTP handler future).
        let cancelled = tokio::time::timeout(
            Duration::from_millis(5),
            cache.get("k", counted(&calls, Ok(rows(1)), 500)),
        )
        .await;
        assert!(cancelled.is_err(), "the first caller must have timed out");
        let again = cache
            .get("k", counted(&calls, Ok(rows(4)), 0))
            .await
            .unwrap();
        assert_eq!(again.len(), 4, "served its own fetch, not the orphan's");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_cancelled_waiter_leaves_the_fetch_to_the_others() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let (cancelled, kept) = tokio::join!(
            tokio::time::timeout(
                Duration::from_millis(5),
                cache.get("k", counted(&calls, Ok(rows(2)), 40)),
            ),
            cache.get("k", counted(&calls, Ok(rows(9)), 40)),
        );
        assert!(cancelled.is_err());
        assert_eq!(
            kept.unwrap().len(),
            2,
            "the survivor finished the one fetch"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The entry survived the cancellation, so the survivor stored it.
        let again = cache
            .get("k", counted(&calls, Ok(rows(9)), 0))
            .await
            .unwrap();
        assert_eq!(again.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "served from the cache");
    }

    #[tokio::test]
    async fn a_panicking_fetch_does_not_wedge_its_key() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let blew_up = std::panic::AssertUnwindSafe(cache.get("k", panicking(&calls)))
            .catch_unwind()
            .await;
        assert!(blew_up.is_err());
        let ok = cache
            .get("k", counted(&calls, Ok(rows(3)), 0))
            .await
            .unwrap();
        assert_eq!(ok.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn get_future_is_send() {
        // `HostConnector` methods are `#[async_trait]`: their futures must be.
        fn assert_send<T: Send>(_: &T) {}
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let fut = cache.get("k", counted(&calls, Ok(rows(1)), 0));
        assert_send(&fut);
    }
}
