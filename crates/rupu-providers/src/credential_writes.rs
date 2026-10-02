//! In-flight credential writes: OAuth token refresh-and-persist tasks.
//!
//! An OAuth token endpoint rotates the refresh token: once it answers, the
//! old refresh token is dead and only the new one, once persisted, keeps the
//! user logged in. A refresh therefore runs as its own task (see the
//! providers' `ensure_valid_token` and `rupu-auth`'s resolver), so dropping
//! the caller mid-flight (a pause, a timeout) only stops the waiting.
//!
//! That is not enough at process exit: a runtime shutting down (`main`
//! returning from `#[tokio::main]`) cancels every task it still owns. Tasks
//! spawned through [`spawn`] are counted here, so the binary can [`wait`]
//! for the outstanding ones, bounded, before it exits.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Refreshes an OAuth credential on a provider client's behalf, through the
/// store that owns it (rupu-auth's `KeychainResolver`): under that store's
/// cross-process lock, re-reading what is stored, and persisting the rotated
/// token. A client that refreshed on its own would keep the rotated token
/// only in memory, leaving a dead refresh token in the store for the next
/// process; with a refresher it never does.
#[async_trait::async_trait]
pub trait OAuthRefresher: Send + Sync {
    /// A fresh credential for `stale` (the one the client holds). If another
    /// holder already rotated the stored credential, that one comes back with
    /// no token request; otherwise the stored one is refreshed and persisted.
    async fn refresh(
        &self,
        stale: crate::auth::AuthCredentials,
    ) -> Result<crate::auth::AuthCredentials, crate::error::ProviderError>;

    /// Record `fields` in the stored credential's `extra`, under the same
    /// lock and against what is stored, so a fact a client learned about its
    /// credential (Gemini's Code Assist project) survives the next refresh
    /// and reaches the next process. `holder` is the credential the client
    /// holds: nothing is written — `Ok(false)` — when the stored credential
    /// is no longer that grant (a re-login or logout since), so a fact about
    /// one grant never lands on another. A store that cannot record says so
    /// with an error; it never drops the fields silently.
    async fn record_extra(
        &self,
        holder: crate::auth::AuthCredentials,
        fields: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<bool, crate::error::ProviderError> {
        let _ = (holder, fields);
        Err(crate::error::ProviderError::AuthConfig(
            "this credential store cannot record credential fields".into(),
        ))
    }
}

static PENDING: AtomicUsize = AtomicUsize::new(0);

fn idle() -> &'static Notify {
    static IDLE: OnceLock<Notify> = OnceLock::new();
    IDLE.get_or_init(Notify::new)
}

/// Decrements the count when the task ends: completed, failed, panicked or
/// cancelled.
struct Pending;

impl Drop for Pending {
    fn drop(&mut self) {
        if PENDING.fetch_sub(1, Ordering::SeqCst) == 1 {
            idle().notify_waiters();
        }
    }
}

/// Spawn a credential write (a token refresh and its persistence) as a
/// tracked task. The handle works like `tokio::spawn`'s; dropping it never
/// cancels the task.
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    PENDING.fetch_add(1, Ordering::SeqCst);
    let pending = Pending;
    tokio::spawn(async move {
        let _pending = pending;
        fut.await
    })
}

/// How many tracked credential writes are still running.
pub fn pending() -> usize {
    PENDING.load(Ordering::SeqCst)
}

/// Wait until no tracked credential write is running, for at most
/// `timeout`. Returns `true` when none is left.
pub async fn wait(timeout: Duration) -> bool {
    let drained = async {
        loop {
            // Registered before the check, so a notify between the check
            // and the await is not lost.
            let notified = idle().notified();
            if pending() == 0 {
                return;
            }
            notified.await;
        }
    };
    tokio::time::timeout(timeout, drained).await.is_ok()
}

/// [`wait`] for a plain OS thread with no runtime (the SIGTERM handler
/// thread): blocks the calling thread until no tracked write is running,
/// for at most `timeout`. The bound holds on this thread's own clock — it
/// does not need the runtime to make progress, so a wedged runtime still
/// lets the caller move on at the deadline. Returns `true` when none is
/// left.
pub fn wait_blocking(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if pending() == 0 {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(20)));
    }
}

/// Set once SIGTERM has arrived and the process is on its way out (the
/// handler holds the exit only for credential writes still draining). An
/// `Arc` so the signal handler itself can set it (`signal_hook::flag`),
/// async-signal-safely, the instant the signal is delivered.
fn terminating_slot() -> &'static Arc<AtomicBool> {
    static TERMINATING: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    TERMINATING.get_or_init(Default::default)
}

/// The flag [`terminating`] reads, for registering with the signal
/// handler: set to `true` inside the handler on delivery, so a check that
/// runs after the signal was delivered sees it even before the handler
/// thread has woken.
pub fn terminating_flag() -> Arc<AtomicBool> {
    terminating_slot().clone()
}

/// Mark the process as terminating. Called by the SIGTERM handler thread
/// before it waits for pending writes (the signal handler itself already
/// set the flag on delivery); never cleared.
pub fn request_termination() {
    terminating_slot().store(true, Ordering::SeqCst);
}

/// Whether SIGTERM has arrived. Every runner checks this before starting an
/// LLM call or a tool dispatch and, if set, stops its run as aborted
/// without starting that work — the process is exiting as soon as the
/// pending credential writes are done, and nothing started now would
/// finish.
pub fn terminating() -> bool {
    terminating_slot().load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    /// A tracked write is visible while it runs and completes when waited
    /// for, even though its handle was dropped.
    #[tokio::test]
    async fn a_tracked_write_is_counted_and_waited_for() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        drop(spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            flag.store(true, Ordering::SeqCst);
        }));
        assert!(pending() >= 1, "the write is registered while it runs");
        assert!(wait(Duration::from_secs(5)).await, "drained in time");
        assert!(done.load(Ordering::SeqCst), "the write ran to completion");
    }

    /// The wait is bounded: a write that outlasts it is reported, not
    /// waited out.
    #[tokio::test]
    async fn the_wait_is_bounded() {
        let handle = spawn(tokio::time::sleep(Duration::from_secs(2)));
        assert!(!wait(Duration::from_millis(50)).await);
        handle.abort();
    }

    /// The thread-side wait sees a write complete (the runtime runs it on
    /// another thread) and is bounded by its own clock.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_blocking_wait_completes_and_is_bounded() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        drop(spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            flag.store(true, Ordering::SeqCst);
        }));
        let started = Instant::now();
        let drained = tokio::task::spawn_blocking(|| wait_blocking(Duration::from_secs(5)))
            .await
            .unwrap();
        assert!(drained, "the write finished within the bound");
        assert!(done.load(Ordering::SeqCst));
        assert!(started.elapsed() >= Duration::from_millis(150));

        let handle = spawn(tokio::time::sleep(Duration::from_secs(5)));
        let started = Instant::now();
        let drained = tokio::task::spawn_blocking(|| wait_blocking(Duration::from_millis(100)))
            .await
            .unwrap();
        assert!(!drained, "a write outlasting the bound is reported");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "and the bound held: {:?}",
            started.elapsed()
        );
        handle.abort();
    }
}
