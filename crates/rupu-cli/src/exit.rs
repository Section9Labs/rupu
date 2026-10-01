//! What the binary does on its way out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
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

/// Set by `main` once it has finished its own drain and is about to exit
/// with the command's code ([`exit_by_signal_if_terminating`]). From then
/// on a SIGTERM ends the process by the signal right inside the signal
/// handler ([`register_signal_actions`]): there is no window left in which
/// a signal delivered after `main`'s last check could let the process
/// exit by its code instead.
fn exit_committed() -> &'static Arc<AtomicBool> {
    static COMMITTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    COMMITTED.get_or_init(Default::default)
}

/// Handle SIGTERM on a dedicated OS thread — not a tokio task, so a wedged
/// runtime can neither delay nor swallow it. Two things happen inside the
/// signal handler itself, async-signal-safely, the instant the signal is
/// delivered ([`register_signal_actions`]): the process is marked
/// terminating (every runner then refuses to start an LLM call or a tool
/// dispatch), and — only once `main` has committed to exit — the default
/// disposition is restored and the signal re-raised, so a late signal
/// still ends the process by SIGTERM. Otherwise the thread takes over: it
/// marks termination again (harmless; it is what the order below rests
/// on), logs that the signal arrived (INFO), waits — at most
/// [`CREDENTIAL_WRITE_DRAIN`], on its own clock — for credential writes
/// still being persisted, and then restores SIGTERM's default disposition
/// and re-raises it: the process dies by the signal, exactly as an
/// unhandled one would, so a cancel, the shell and `systemctl stop` all see
/// a signal death and not an exit code. With nothing pending that happens
/// at once. The run itself is never waited for.
///
/// Commands that shut down gracefully on SIGTERM themselves (`autoflow
/// serve`) don't install it.
#[cfg(unix)]
pub fn install_sigterm_handler() {
    use signal_hook::consts::SIGTERM;
    if let Err(e) = register_signal_actions() {
        tracing::warn!(error = %e, "could not install the SIGTERM handler");
        return;
    }
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

/// The actions that run inside the SIGTERM signal handler itself, in this
/// order (signal-hook runs them in registration order):
///
/// 1. the terminating flag is set (`signal_hook::flag::register`, an
///    atomic store) — so [`exit_by_signal_if_terminating`] sees a signal
///    that was delivered before its check, whether or not the handler
///    thread has woken yet;
/// 2. if `main` has committed to exit ([`exit_committed`]), the default
///    disposition is restored and the signal re-raised
///    (`register_conditional_default`): the process dies by SIGTERM here
///    and now.
///
/// With every store and load `SeqCst`, `main`'s "store committed, then
/// load terminating" and the handler's "store terminating, then load
/// committed" cannot both miss: a signal delivered before `main`'s check
/// is seen by that check (and `main` hands off to the signal path); one
/// delivered after it finds the commit and kills the process itself. The
/// handler thread stays responsible for the bounded drain and the
/// re-raise in the ordinary case.
#[cfg(unix)]
fn register_signal_actions() -> std::io::Result<()> {
    use signal_hook::consts::SIGTERM;
    signal_hook::flag::register(
        SIGTERM,
        rupu_providers::credential_writes::terminating_flag(),
    )?;
    signal_hook::flag::register_conditional_default(SIGTERM, exit_committed().clone())?;
    Ok(())
}

/// What SIGTERM does, on the handler thread. Termination is marked before
/// anything that could block (the log line goes to stderr, which may be a
/// stalled pipe), so a runner's next check refuses new work no matter
/// what the log does.
#[cfg(unix)]
fn on_sigterm() -> ! {
    rupu_providers::credential_writes::request_termination();
    tracing::info!(
        "SIGTERM received; terminating by it once pending credential writes are drained"
    );
    drain_credential_writes_blocking(CREDENTIAL_WRITE_DRAIN);
    die_by_sigterm()
}

/// Default disposition back, then the signal again: death by SIGTERM.
/// signal-hook's `emulate_default_handler` restores the disposition,
/// unblocks the signal and raises it — which ends the process — and falls
/// through to `abort()` (SIGABRT) if the disposition could not be
/// restored, the signal could not be raised, or the raised signal did not
/// end the process; for SIGTERM it never returns. The process therefore
/// dies by SIGTERM, else by SIGABRT, and never exits with a code here; the
/// `abort` below is only what makes that contract a type.
#[cfg(unix)]
fn die_by_sigterm() -> ! {
    let _ = signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGTERM);
    std::process::abort()
}

/// `main`'s last step, after its own drain: a process that was signalled
/// dies by the signal, never by the command's exit code. First the exit is
/// committed ([`exit_committed`]) — from here on a SIGTERM kills the
/// process inside the signal handler — and then, if a signal already
/// arrived (the flag is set inside the handler on delivery, so this is not
/// a race with the handler thread), the process hands off to the signal
/// path instead of returning the command's code. The handler thread
/// re-raises only once the pending credential writes are drained, and its
/// drain polls — so a command that returned while a write was pending (the
/// run aborted as terminating) would otherwise let `main`'s own drain,
/// woken the instant the write lands, exit with the command's code first.
/// Returns only when no signal arrived.
pub fn exit_by_signal_if_terminating() {
    exit_committed().store(true, Ordering::SeqCst);
    #[cfg(unix)]
    if rupu_providers::credential_writes::terminating() {
        die_by_sigterm();
    }
}

/// Test seam, debug builds only: with `RUPU_TEST_HOLD_CREDENTIAL_WRITE_MS=
/// <ms>` set, keep one tracked credential write open for that long, so an
/// integration test can observe the exit path with a write pending
/// (`tests/it/sigterm_exit.rs`; `cargo test` builds the binary in debug). A
/// release binary ignores the variable; a value that does not parse is
/// ignored too.
pub fn hold_test_credential_write() {
    #[cfg(debug_assertions)]
    {
        let Some(ms) = std::env::var("RUPU_TEST_HOLD_CREDENTIAL_WRITE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        else {
            return;
        };
        drop(rupu_providers::credential_writes::spawn(
            tokio::time::sleep(Duration::from_millis(ms)),
        ));
    }
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

    // ---- the signal-handler-side actions, in a child process ---------------
    //
    // The closest seam to "the flag is set and the committed exit is killed
    // inside the signal handler, with no help from the handler thread":
    // this test binary re-runs itself with only `register_signal_actions`
    // installed (no thread), driven by `RUPU_EXIT_SEAM_ROLE`.

    /// The child side. A no-op unless this process was spawned as a role.
    #[cfg(unix)]
    #[test]
    fn child_role_entry() {
        let Ok(role) = std::env::var("RUPU_EXIT_SEAM_ROLE") else {
            return;
        };
        register_signal_actions().expect("register the signal actions");
        match role.as_str() {
            // `main`'s last step done: committed, no signal yet. Then the
            // SIGTERM must end the process by itself.
            "committed" => {
                exit_by_signal_if_terminating();
                eprintln!("seam: committed");
                std::thread::sleep(Duration::from_secs(10));
                std::process::exit(0)
            }
            // Not committed: the signal only sets the flag, which this
            // process reports by its exit code (7 = set, 8 = never set).
            "flag-only" => {
                eprintln!("seam: ready");
                let deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < deadline {
                    if rupu_providers::credential_writes::terminating() {
                        std::process::exit(7)
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::process::exit(8)
            }
            other => panic!("unknown role {other:?}"),
        }
    }

    /// Spawn this test binary as `role`, wait for its `seam: <marker>`
    /// line, send it SIGTERM and return how it ended.
    #[cfg(unix)]
    fn run_role(role: &str, marker: &str) -> std::process::ExitStatus {
        use std::io::BufRead;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "exit::tests::child_role_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("RUPU_EXIT_SEAM_ROLE", role)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the child role");
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                let _ = tx.send(line);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) if line.contains(&format!("seam: {marker}")) => break,
                Ok(_) => {}
                Err(e) => {
                    let _ = child.kill();
                    panic!("the child never reported {marker:?}: {e}");
                }
            }
        }
        let killed = std::process::Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!("the child did not end after SIGTERM");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The terminating flag is set inside the signal handler itself: a
    /// process with the actions installed and no handler thread sees
    /// `terminating()` flip on delivery.
    #[cfg(unix)]
    #[test]
    fn the_signal_handler_itself_sets_the_terminating_flag() {
        let status = run_role("flag-only", "ready");
        assert_eq!(
            status.code(),
            Some(7),
            "the flag was set on delivery, with no handler thread: {status:?}"
        );
    }

    /// Once `main` has committed to exit, a SIGTERM ends the process by the
    /// signal right inside the handler — with no handler thread to re-raise
    /// it, and before the committed exit (10s away here) can happen.
    #[cfg(unix)]
    #[test]
    fn a_sigterm_after_the_committed_exit_kills_by_the_signal_in_the_handler() {
        use std::os::unix::process::ExitStatusExt;
        let status = run_role("committed", "committed");
        assert_eq!(
            status.signal(),
            Some(libc_sigterm()),
            "died by SIGTERM inside the handler, not by the exit code: {status:?}"
        );
        assert_eq!(status.code(), None);
    }

    #[cfg(unix)]
    fn libc_sigterm() -> i32 {
        signal_hook::consts::SIGTERM
    }
}
