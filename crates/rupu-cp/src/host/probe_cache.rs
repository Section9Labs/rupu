//! Last health probe of each remote host, shared by every `GET /api/hosts`.
//!
//! The shell's host footer polls `GET /api/hosts` from every page, and a
//! probe of an SSH host is two remote commands (`info()` + an active-run
//! listing) — seconds per request, holding one of the browser's six
//! per-origin connections while the page's own fetches queue behind it.
//! [`HostProbeCache`] answers from the last probe instead:
//!
//! - younger than `fresh_for` → returned as is;
//! - older → returned at once, and ONE background probe refreshes it (a
//!   polling page never waits on SSH, and a burst of page loads never fans
//!   out a burst of SSH sessions);
//! - older than `max_stale` (the CP sat idle), or never probed (first request
//!   after start, a newly added host) → the request waits for a probe,
//!   joining one already in flight, rather than show a status that old.
//!
//! A probe is only ever started by a request, so an idle CP opens no SSH
//! sessions at all.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a probe is served without starting a refresh.
pub const FRESH_FOR: Duration = Duration::from_secs(15);
/// Past this age a probe is no longer shown at all; the request waits for a
/// new one. Comfortably above the shell's 60s host poll, so a page left open
/// never waits.
pub const MAX_STALE: Duration = Duration::from_secs(300);

struct Slot<P> {
    last: Mutex<Option<(Instant, P)>>,
    /// Held for the duration of a probe: single-flight per host.
    probing: Arc<tokio::sync::Mutex<()>>,
}

impl<P: Clone> Slot<P> {
    fn last(&self) -> Option<(Instant, P)> {
        lock(&self.last).clone()
    }

    fn store(&self, probe: P) {
        *lock(&self.last) = Some((Instant::now(), probe));
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// See the module docs. `P` is whatever one probe produces.
pub struct HostProbeCache<P> {
    fresh_for: Duration,
    max_stale: Duration,
    slots: Mutex<HashMap<String, Arc<Slot<P>>>>,
}

impl<P> Default for HostProbeCache<P> {
    fn default() -> Self {
        Self::new(FRESH_FOR, MAX_STALE)
    }
}

impl<P> HostProbeCache<P> {
    pub fn new(fresh_for: Duration, max_stale: Duration) -> Self {
        Self {
            fresh_for,
            max_stale,
            slots: Mutex::default(),
        }
    }

    /// Drop a host's probe (it was removed from the registry).
    pub fn forget(&self, host_id: &str) {
        lock(&self.slots).remove(host_id);
    }
}

impl<P: Clone + Send + 'static> HostProbeCache<P> {
    fn slot(&self, host_id: &str) -> Arc<Slot<P>> {
        Arc::clone(
            lock(&self.slots)
                .entry(host_id.to_string())
                .or_insert_with(|| {
                    Arc::new(Slot {
                        last: Mutex::new(None),
                        probing: Arc::new(tokio::sync::Mutex::new(())),
                    })
                }),
        )
    }

    /// `host_id`'s probe, per the module docs. `probe` is only run when a
    /// probe is due and none is already running for this host. Must be
    /// called from within a tokio runtime (a stale hit spawns the refresh).
    pub async fn get<F, Fut>(&self, host_id: &str, probe: F) -> P
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = P> + Send + 'static,
    {
        let slot = self.slot(host_id);
        if let Some((at, cached)) = slot.last() {
            let age = at.elapsed();
            if age < self.max_stale {
                if age >= self.fresh_for {
                    if let Ok(guard) = Arc::clone(&slot.probing).try_lock_owned() {
                        let slot = Arc::clone(&slot);
                        tokio::spawn(async move {
                            let fresh = probe().await;
                            slot.store(fresh);
                            drop(guard);
                        });
                    }
                }
                return cached;
            }
        }
        let _guard = slot.probing.lock().await;
        // A probe that held the lock while we waited has answered for us.
        if let Some((at, cached)) = slot.last() {
            if at.elapsed() < self.max_stale {
                return cached;
            }
        }
        let fresh = probe().await;
        slot.store(fresh.clone());
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn counting(
        calls: &Arc<AtomicU32>,
        value: u32,
    ) -> impl FnOnce() -> std::pin::Pin<Box<dyn Future<Output = u32> + Send>> + Send + 'static {
        let calls = Arc::clone(calls);
        move || {
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                value
            })
        }
    }

    #[tokio::test]
    async fn a_fresh_probe_is_reused() {
        let cache = HostProbeCache::new(Duration::from_secs(3600), Duration::MAX);
        let calls = Arc::new(AtomicU32::new(0));
        assert_eq!(cache.get("h", counting(&calls, 1)).await, 1);
        assert_eq!(cache.get("h", counting(&calls, 2)).await, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_first_requests_share_one_probe() {
        let cache = Arc::new(HostProbeCache::new(
            Duration::from_secs(3600),
            Duration::MAX,
        ));
        let calls = Arc::new(AtomicU32::new(0));
        let gets = (0..5).map(|i| {
            let cache = Arc::clone(&cache);
            let probe = counting(&calls, i);
            tokio::spawn(async move { cache.get("h", probe).await })
        });
        let got: Vec<u32> = futures_util::future::join_all(gets)
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one SSH probe, not five");
        assert!(got.iter().all(|v| *v == got[0]));
    }

    #[tokio::test]
    async fn a_stale_probe_is_served_at_once_and_refreshed_once_in_the_background() {
        let cache = Arc::new(HostProbeCache::new(Duration::ZERO, Duration::MAX));
        let calls = Arc::new(AtomicU32::new(0));
        assert_eq!(cache.get("h", counting(&calls, 1)).await, 1);

        // Stale: answered from the cache while one refresh runs; requests
        // during that refresh start no other.
        let started = Instant::now();
        assert_eq!(cache.get("h", counting(&calls, 2)).await, 1);
        assert_eq!(cache.get("h", counting(&calls, 3)).await, 1);
        assert!(
            started.elapsed() < Duration::from_millis(20),
            "never waits on a probe"
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.get("h", counting(&calls, 4)).await, 2);
    }

    #[tokio::test]
    async fn a_probe_past_max_stale_is_awaited_not_served() {
        let cache = HostProbeCache::new(Duration::ZERO, Duration::from_millis(30));
        let calls = Arc::new(AtomicU32::new(0));
        assert_eq!(cache.get("h", counting(&calls, 1)).await, 1);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(cache.get("h", counting(&calls, 2)).await, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn hosts_are_independent_and_forget_reprobes() {
        let cache = HostProbeCache::new(Duration::from_secs(3600), Duration::MAX);
        let calls = Arc::new(AtomicU32::new(0));
        assert_eq!(cache.get("a", counting(&calls, 1)).await, 1);
        assert_eq!(cache.get("b", counting(&calls, 2)).await, 2);
        cache.forget("a");
        assert_eq!(cache.get("a", counting(&calls, 3)).await, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}
