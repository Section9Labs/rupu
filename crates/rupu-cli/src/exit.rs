//! What the binary does on its way out.

use std::time::Duration;

/// How long the binary waits, before it goes away, for credential writes
/// still in flight — on the normal exit path (`main`, before the runtime
/// shuts down) and on SIGTERM (the handler thread, before the re-raise).
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

/// [`drain_credential_writes`] for the SIGTERM handler thread: a plain OS
/// thread with no runtime, so it blocks on
/// [`rupu_providers::credential_writes::wait_blocking`]. The bound is this
/// thread's own clock — a runtime that makes no progress cannot stretch
/// it. Returns `true` when nothing is left in flight.
pub fn drain_credential_writes_blocking(timeout: Duration) -> bool {
    let pending = rupu_providers::credential_writes::pending();
    if pending == 0 {
        return true;
    }
    eprintln!("rupu: waiting for {pending} credential write(s) to finish…");
    let drained = rupu_providers::credential_writes::wait_blocking(timeout);
    if !drained {
        eprintln!(
            "rupu: gave up waiting for credential writes after {}s",
            timeout.as_secs()
        );
    }
    drained
}

/// The exit status of a process killed by SIGTERM as the shell reports it
/// (128 + 15). The handler's last resort only: it normally dies by the
/// re-raised signal itself.
pub const SIGTERM_EXIT_CODE: i32 = 143;

/// Handle SIGTERM on a dedicated OS thread — not a tokio task, so a wedged
/// runtime can neither delay nor swallow it. On the signal the thread marks
/// the process terminating (every runner then refuses to start an LLM call
/// or a tool dispatch), waits — at most [`CREDENTIAL_WRITE_DRAIN`], on its
/// own clock — for credential writes still being persisted, and then
/// restores SIGTERM's default disposition and re-raises it: the process
/// dies by the signal, exactly as an unhandled one would, so a cancel, the
/// shell and `systemctl stop` all see a signal death and not an exit code.
/// With nothing pending that happens at once. The run itself is never
/// waited for.
///
/// Commands that shut down gracefully on SIGTERM themselves (`autoflow
/// serve`) don't install it.
#[cfg(unix)]
pub fn install_sigterm_handler() {
    use signal_hook::consts::SIGTERM;
    let mut signals = match signal_hook::iterator::Signals::new([SIGTERM]) {
        Ok(signals) => signals,
        Err(e) => {
            tracing::warn!(error = %e, "could not install the SIGTERM handler");
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("rupu-sigterm".into())
        .spawn(move || {
            if signals.forever().next().is_some() {
                on_sigterm();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the SIGTERM handler thread");
    }
}

/// No-op off unix: there is no SIGTERM.
#[cfg(not(unix))]
pub fn install_sigterm_handler() {}

/// What SIGTERM does, on the handler thread.
#[cfg(unix)]
fn on_sigterm() -> ! {
    rupu_providers::credential_writes::request_termination();
    drain_credential_writes_blocking(CREDENTIAL_WRITE_DRAIN);
    // Default disposition back, then the signal again: death by SIGTERM.
    let _ = signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGTERM);
    // Only reached if the re-raised signal did not end the process (some
    // other handler got installed meanwhile); exit as the shell would
    // report a SIGTERM death.
    std::process::exit(SIGTERM_EXIT_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

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

    /// The SIGTERM thread's drain (what runs before the re-raise): a
    /// pending write delays it until the write lands, and no longer than
    /// the bound. The write runs on the runtime's threads while the drain
    /// blocks a plain thread, as in the real handler.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_sigterm_drain_waits_for_a_pending_write_up_to_the_bound() {
        let _registry = REGISTRY.lock().await;
        let persisted = Arc::new(AtomicBool::new(false));
        let flag = persisted.clone();
        drop(rupu_providers::credential_writes::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            flag.store(true, Ordering::SeqCst);
        }));
        let started = Instant::now();
        let drained = tokio::task::spawn_blocking(|| {
            drain_credential_writes_blocking(Duration::from_secs(5))
        })
        .await
        .unwrap();
        assert!(drained, "the write landed within the bound");
        assert!(persisted.load(Ordering::SeqCst));
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the re-raise waited for it ({:?})",
            started.elapsed()
        );

        // A write outlasting the bound: the drain gives up at the bound.
        let stuck =
            rupu_providers::credential_writes::spawn(tokio::time::sleep(Duration::from_secs(5)));
        let started = Instant::now();
        let drained = tokio::task::spawn_blocking(|| {
            drain_credential_writes_blocking(Duration::from_millis(100))
        })
        .await
        .unwrap();
        assert!(!drained, "reported, not waited out");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "bounded: {:?}",
            started.elapsed()
        );
        stuck.abort();
        assert!(
            rupu_providers::credential_writes::wait(Duration::from_secs(2)).await,
            "the aborted write is gone from the registry"
        );
    }

    /// With nothing in flight the exit path doesn't wait at all — the
    /// (long) bound is never spent.
    #[tokio::test]
    async fn nothing_in_flight_returns_at_once() {
        let _registry = REGISTRY.lock().await;
        let started = Instant::now();
        assert!(drain_credential_writes(Duration::from_secs(10)).await);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "returned after {:?}",
            started.elapsed()
        );
        let started = Instant::now();
        assert!(drain_credential_writes_blocking(Duration::from_secs(10)));
        assert!(started.elapsed() < Duration::from_millis(200));
    }
}
