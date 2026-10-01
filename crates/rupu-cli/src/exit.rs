//! What the binary does on its way out.

use std::time::Duration;

/// How long `main` waits, before the runtime shuts down, for credential
/// writes still in flight.
pub const CREDENTIAL_WRITE_DRAIN: Duration = Duration::from_secs(10);

/// Wait (at most `timeout`) for OAuth token refreshes still being persisted
/// ([`rupu_providers::credential_writes`]).
///
/// A refresh runs as its own task so a dropped caller (a pause, a listing
/// timeout) can't abandon it after the server rotated the refresh token —
/// but `main` returning from `#[tokio::main]` shuts the runtime down, which
/// cancels every task it still owns, including that one (e.g. `rupu workflow
/// run` exiting right after a pause). `main` calls this last. Returns
/// `true` when nothing is left in flight.
pub async fn drain_credential_writes(timeout: Duration) -> bool {
    let pending = rupu_providers::credential_writes::pending();
    if pending == 0 {
        return true;
    }
    eprintln!("rupu: waiting for {pending} credential write(s) to finish…");
    let drained = rupu_providers::credential_writes::wait(timeout).await;
    if !drained {
        eprintln!(
            "rupu: gave up waiting for credential writes after {}s",
            timeout.as_secs()
        );
    }
    drained
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// The write registry is process-wide: these tests must not see each
    /// other's writes.
    static REGISTRY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// The exit path waits for a credential write whose caller is long
    /// gone (its handle dropped) instead of letting runtime shutdown cancel
    /// it.
    #[tokio::test]
    async fn the_exit_path_waits_for_an_in_flight_credential_write() {
        let _registry = REGISTRY.lock().await;
        let persisted = Arc::new(AtomicBool::new(false));
        let flag = persisted.clone();
        drop(rupu_providers::credential_writes::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            flag.store(true, Ordering::SeqCst);
        }));
        assert!(drain_credential_writes(Duration::from_secs(5)).await);
        assert!(persisted.load(Ordering::SeqCst));
    }

    /// With nothing in flight the exit path doesn't wait at all — the
    /// (long) bound is never spent.
    #[tokio::test]
    async fn nothing_in_flight_returns_at_once() {
        let _registry = REGISTRY.lock().await;
        let started = std::time::Instant::now();
        assert!(drain_credential_writes(Duration::from_secs(10)).await);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "returned after {:?}",
            started.elapsed()
        );
    }
}
