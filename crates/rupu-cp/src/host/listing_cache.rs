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
//! - The fetch runs in its own spawned task, to completion, whether or not
//!   anyone is still waiting on it. Its waiters only await its shared result,
//!   and a caller arriving while it runs joins it, even when every earlier
//!   waiter went away. So client churn (rapid tab switches, aborted page
//!   requests) never starts a second listing for a key on a host. A spawned
//!   task is always polled, so its answer is fresh when it completes, and the
//!   task itself stores it.
//! - A fetch is bounded at [`LISTING_MAX_INFLIGHT`]. Past it the fetch future
//!   is dropped (the ssh listing exec kills its child on drop), every waiter
//!   gets an `Unreachable` error, and nothing is stored.
//! - A fetch that panics fails every waiter with an error and stores nothing.
//! - [`ListingCache::clear`], which every mutating connector method calls via
//!   [`ClearOnDrop`], drops every stored listing and forgets every in-flight
//!   fetch without aborting it (its waiters asked before the mutation). A
//!   fetch that STARTED before the clear never stores its (pre-mutation)
//!   answer, and a caller arriving after the clear never joins it.

use crate::host::connector::HostConnectorError;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
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

/// The longest one listing fetch may run. Fetches run to completion even when
/// nobody waits on them any more, so without this bound a hung remote would
/// keep its key's fetch (and, for ssh, its child process) alive forever.
pub const LISTING_MAX_INFLIGHT: Duration = Duration::from_secs(60);

/// One listing's rows, shared between every caller that received it.
pub type Rows = Arc<Vec<serde_json::Value>>;

type Fetch = Shared<BoxFuture<'static, Result<Rows, HostConnectorError>>>;

/// One fetch in flight for a key.
struct InFlight {
    /// Never reused. Only the task running THIS fetch may retire its entry or
    /// store its rows, so a fetch that `clear` already forgot can never store.
    id: u64,
    /// The fetch task's result, shared by every caller awaiting it.
    fut: Fetch,
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
    /// Shared with the spawned fetch tasks, which retire and store their own
    /// entries.
    state: Arc<Mutex<State>>,
}

impl Default for ListingCache {
    fn default() -> Self {
        Self::new(LISTING_TTL)
    }
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn panicked() -> HostConnectorError {
    HostConnectorError::Unreachable("listing fetch panicked".into())
}

/// Retire `key`'s in-flight entry if it is still fetch `id`'s, and on success
/// store its rows. If `clear` forgot the entry meanwhile (or a newer fetch
/// replaced it), the id no longer matches and nothing happens.
fn retire(state: &Mutex<State>, key: &str, id: u64, out: &Result<Rows, HostConnectorError>) {
    let retired = {
        let mut s = lock(state);
        if !matches!(s.inflight.get(key), Some(f) if f.id == id) {
            return;
        }
        if let Ok(rows) = out {
            s.fresh
                .insert(key.to_string(), (Instant::now(), Arc::clone(rows)));
        }
        s.inflight.remove(key)
    };
    // The entry holds a handle on this very fetch; let it go outside the lock.
    drop(retired);
}

/// The body of one fetch's spawned task: run `fetch` (bounded, panics caught),
/// then retire and store its own entry.
async fn run_fetch<Fut>(
    state: Arc<Mutex<State>>,
    key: String,
    id: u64,
    fetch: Fut,
) -> Result<Rows, HostConnectorError>
where
    Fut: Future<Output = Result<Vec<serde_json::Value>, HostConnectorError>>,
{
    // `timeout` drops `fetch` when it fires (for ssh, `kill_on_drop` then
    // kills the child). The panic is caught so the task completes normally
    // and every waiter is woken with the same error. (The panic message
    // itself still reaches the panic hook.)
    let out =
        match tokio::time::timeout(LISTING_MAX_INFLIGHT, AssertUnwindSafe(fetch).catch_unwind())
            .await
        {
            Ok(Ok(rows)) => rows.map(Arc::new),
            Ok(Err(_)) => Err(panicked()),
            Err(_) => Err(HostConnectorError::Unreachable(format!(
                "listing took longer than {}s",
                LISTING_MAX_INFLIGHT.as_secs()
            ))),
        };
    retire(&state, &key, id, &out);
    out
}

impl ListingCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            state: Arc::default(),
        }
    }

    /// Drop every listing and forget every fetch in flight, so new callers
    /// start fresh. In-flight fetches are not aborted (their waiters asked
    /// before the mutation and still get their answer), but they can no
    /// longer store it: their entry is gone, so their id no longer matches.
    pub fn clear(&self) {
        let (fresh, inflight) = {
            let mut s = lock(&self.state);
            (
                std::mem::take(&mut s.fresh),
                std::mem::take(&mut s.inflight),
            )
        };
        drop((fresh, inflight));
    }

    /// Clear when the returned guard drops, i.e. when the mutating call
    /// holding it returns, success or failure (a failed mutation may still
    /// have partly applied on the remote).
    pub fn clear_on_drop(&self) -> ClearOnDrop<'_> {
        ClearOnDrop(self)
    }

    /// `key`'s rows: fresh from the cache, joined onto an in-flight fetch, or
    /// fetched now via `fetch`. `fetch` is only called when neither exists;
    /// its future then runs in a spawned task (see the module docs). Must be
    /// called inside a tokio runtime.
    pub async fn get<F, Fut>(&self, key: &str, fetch: F) -> Result<Rows, HostConnectorError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Vec<serde_json::Value>, HostConnectorError>> + Send + 'static,
    {
        let fut = {
            let mut s = lock(&self.state);
            if let Some((at, rows)) = s.fresh.get(key) {
                if at.elapsed() < self.ttl {
                    return Ok(Arc::clone(rows));
                }
            }
            if let Some(joined) = s.inflight.get(key) {
                joined.fut.clone()
            } else {
                s.next_id += 1;
                let id = s.next_id;
                // Spawned while the lock is held, so the entry is in place
                // before the task can try to retire it.
                let task = tokio::spawn(run_fetch(
                    Arc::clone(&self.state),
                    key.to_string(),
                    id,
                    fetch(),
                ));
                let (state, owned_key) = (Arc::clone(&self.state), key.to_string());
                let fut: Fetch = task
                    .map(move |joined| {
                        joined.unwrap_or_else(|_| {
                            // The task itself died (it catches panics, so this
                            // is the runtime shutting down): retire for it.
                            let out = Err(panicked());
                            retire(&state, &owned_key, id, &out);
                            out
                        })
                    })
                    .boxed()
                    .shared();
                s.inflight.insert(
                    key.to_string(),
                    InFlight {
                        id,
                        fut: fut.clone(),
                    },
                );
                fut
            }
        };
        fut.await
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

    #[tokio::test(start_paused = true)]
    async fn a_fetch_whose_waiters_left_still_completes_and_serves_the_next_caller() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        // The only waiter goes away (a dropped HTTP handler future).
        let cancelled = tokio::time::timeout(
            Duration::from_millis(5),
            cache.get("k", counted(&calls, Ok(rows(1)), 500)),
        )
        .await;
        assert!(cancelled.is_err(), "the only caller must have timed out");
        // Nobody waits on the fetch now; its task finishes it anyway.
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            lock(&cache.state).inflight.is_empty(),
            "the task retired its own entry"
        );
        let again = cache
            .get("k", counted(&calls, Ok(rows(4)), 0))
            .await
            .unwrap();
        assert_eq!(again.len(), 1, "served the completed fetch's rows");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no second listing");
    }

    #[tokio::test(start_paused = true)]
    async fn callers_arriving_while_an_orphaned_fetch_runs_join_it() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let cancelled = tokio::time::timeout(
            Duration::from_millis(5),
            cache.get("k", counted(&calls, Ok(rows(2)), 500)),
        )
        .await;
        assert!(cancelled.is_err(), "the first caller must have timed out");
        // The orphaned fetch is still running: the next caller joins it
        // instead of starting another listing.
        let joined = cache
            .get("k", counted(&calls, Ok(rows(9)), 0))
            .await
            .unwrap();
        assert_eq!(joined.len(), 2, "got the orphaned fetch's rows");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "still one listing");
    }

    #[tokio::test(start_paused = true)]
    async fn a_fetch_exceeding_the_max_inflight_age_is_abandoned() {
        use std::sync::atomic::AtomicBool;

        /// Flips its flag when dropped, i.e. when the fetch future is.
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let hung = {
            let (calls, dropped) = (Arc::clone(&calls), Arc::clone(&dropped));
            move || async move {
                let _flag = DropFlag(dropped);
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(LISTING_MAX_INFLIGHT + Duration::from_secs(30)).await;
                Ok(rows(1))
            }
        };
        // Two waiters on the one hung fetch.
        let (a, b) = tokio::join!(
            cache.get("k", hung),
            cache.get("k", counted(&calls, Ok(rows(9)), 0)),
        );
        for out in [a, b] {
            match out {
                Err(HostConnectorError::Unreachable(m)) => {
                    assert_eq!(m, "listing took longer than 60s")
                }
                other => panic!("want the timeout error, got {other:?}"),
            }
        }
        assert!(
            dropped.load(Ordering::SeqCst),
            "the fetch future was dropped"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let s = lock(&cache.state);
        assert!(s.fresh.is_empty(), "nothing stored");
        assert!(s.inflight.is_empty(), "the entry was retired");
    }

    #[tokio::test]
    async fn a_panicking_fetch_does_not_wedge_its_key() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let blew_up = cache.get("k", panicking(&calls)).await;
        assert!(
            matches!(blew_up, Err(HostConnectorError::Unreachable(_))),
            "the panic surfaces as an error to its caller, not as a panic"
        );
        let ok = cache
            .get("k", counted(&calls, Ok(rows(3)), 0))
            .await
            .unwrap();
        assert_eq!(ok.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// `Shared` does not wake its other waiters when the inner future panics,
    /// so a waiter parked on the fetch at that moment would hang forever and
    /// keep the key joined to a dead fetch. Needs two threads: the fetch blocks
    /// INSIDE its poll (std channels), so the shared future stays mid-poll
    /// while a second caller polls it, registers and parks.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panic_with_a_parked_waiter_fails_both_and_does_not_wedge_the_key() {
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;

        const BOUND: Duration = Duration::from_secs(5);
        let cache = Arc::new(ListingCache::default());
        let calls = Arc::new(AtomicU32::new(0));
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (go_tx, go_rx) = mpsc::channel::<()>();

        let a = {
            let (cache, calls) = (Arc::clone(&cache), Arc::clone(&calls));
            tokio::spawn(async move {
                cache
                    .get("k", move || async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        started_tx.send(()).unwrap();
                        go_rx.recv_timeout(BOUND).expect("released by the test");
                        boom()
                    })
                    .await
            })
        };
        // A's fetch is now blocking inside its poll: the shared future is
        // mid-poll and cannot finish until released.
        started_rx.recv_timeout(BOUND).expect("A's fetch started");

        let parked = Arc::new(AtomicBool::new(false));
        let b = {
            let (cache, calls, parked) =
                (Arc::clone(&cache), Arc::clone(&calls), Arc::clone(&parked));
            tokio::spawn(async move {
                let mut get = Box::pin(cache.get("k", counted(&calls, Ok(rows(9)), 0)));
                std::future::poll_fn(move |cx| {
                    let r = get.as_mut().poll(cx);
                    // B's first poll found the shared future mid-poll,
                    // registered its waker and returned Pending.
                    if r.is_pending() {
                        parked.store(true, Ordering::SeqCst);
                    }
                    r
                })
                .await
            })
        };
        tokio::time::timeout(BOUND, async {
            while !parked.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("B parks on the in-flight fetch");
        assert!(
            lock(&cache.state).inflight.contains_key("k"),
            "A's fetch is still in flight"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "B joined A's fetch rather than starting its own"
        );

        go_tx.send(()).unwrap();
        let (ra, rb) = tokio::time::timeout(BOUND, async { tokio::join!(a, b) })
            .await
            .expect("both callers must complete, none may hang on the dead fetch");
        assert!(matches!(
            ra.unwrap(),
            Err(HostConnectorError::Unreachable(_))
        ));
        assert!(matches!(
            rb.unwrap(),
            Err(HostConnectorError::Unreachable(_))
        ));

        let third = tokio::time::timeout(BOUND, cache.get("k", counted(&calls, Ok(rows(3)), 0)))
            .await
            .expect("a third caller must not join the dead fetch")
            .unwrap();
        assert_eq!(third.len(), 3, "the third caller got its own rows");
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
