//! What the binary does on its way out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// How long the binary waits, before it goes away, for credential writes
/// still in flight — on the normal exit path (`main`, before the runtime
/// shuts down) and on SIGTERM (the handler thread, before the re-raise).
pub const CREDENTIAL_WRITE_DRAIN: Duration = Duration::from_secs(10);

/// How long the SIGTERM handler thread waits for each of its own stderr
/// lines — `SIGTERM received`, the drain's `waiting for N …` and `gave up`
/// — to land before it moves on ([`say_detached`]): long enough that each
/// is there when stderr is readable (the `gave up` line, written right
/// before the re-raise, is how the operator learns a rotation may have
/// been lost), bounded so a stderr nobody reads delays the re-raise by at
/// most this per line: three lines, 750ms, on top of the drain.
pub const SIGTERM_STDERR_GRACE: Duration = Duration::from_millis(250);

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
/// it — and its two messages go out through [`say_detached`], each waited
/// for at most [`SIGTERM_STDERR_GRACE`], so a stderr nobody reads cannot
/// stretch it by more than 500ms either; with a readable stderr both
/// lines land, the `gave up` one included, which is written right before
/// the re-raise and is the only word the operator gets that a rotation
/// may have been lost. Returns `true` when nothing is left in flight.
pub fn drain_credential_writes_blocking(timeout: Duration) -> bool {
    let pending = rupu_providers::credential_writes::pending();
    if pending == 0 {
        return true;
    }
    say_detached(
        move || eprintln!("rupu: waiting for {pending} credential write(s) to finish…"),
        Some(SIGTERM_STDERR_GRACE),
    );
    let drained = rupu_providers::credential_writes::wait_blocking(timeout);
    if !drained {
        let secs = timeout.as_secs();
        say_detached(
            move || eprintln!("rupu: gave up waiting for credential writes after {secs}s"),
            Some(SIGTERM_STDERR_GRACE),
        );
    }
    drained
}

/// Run `say` (a stderr write, or a log line that may go to stderr) on a
/// detached thread, so a stderr nobody reads — a full pipe — blocks that
/// thread and never the caller: the handler thread's drain and re-raise
/// never depend on stderr being writable. The caller waits at most `grace`
/// for the write to land (`None`: not at all) and never joins the thread.
/// If no thread can be spawned, the message is simply not written.
fn say_detached(say: impl FnOnce() + Send + 'static, grace: Option<Duration>) {
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("rupu-sigterm-say".into())
        .spawn(move || {
            say();
            let _ = done_tx.send(());
        });
    if spawned.is_err() {
        return;
    }
    if let Some(grace) = grace {
        let _ = done_rx.recv_timeout(grace);
    }
}

/// Set by `main` once it has finished its own drain and is about to exit
/// with the command's code ([`exit_by_signal_if_terminating`]) — and by
/// [`install_sigterm_handler`] when the handler thread could not be set
/// up. From then on a SIGTERM ends the process by the signal right inside
/// the signal handler ([`register_signal_actions_at`]'s re-raise action):
/// there is no window left in which a signal delivered after `main`'s
/// last check could let the process exit by its code instead. That holds
/// as long as the re-raise action is registered; without it — and
/// without the handler thread — a signal only marks the process
/// terminating, the one state in which it is not acted on at once (see
/// [`register_signal_actions_at`]).
fn exit_committed() -> &'static Arc<AtomicBool> {
    static COMMITTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    COMMITTED.get_or_init(Default::default)
}

/// Handle SIGTERM on a dedicated OS thread — not a tokio task, so a wedged
/// runtime can neither delay nor swallow it. Two things happen inside the
/// signal handler itself, async-signal-safely, the instant the signal is
/// delivered ([`register_signal_actions_at`]): the process is marked
/// terminating (every runner then refuses to start an LLM call or a tool
/// dispatch), and — only once the exit is committed — the default
/// disposition is restored and the signal re-raised, so a late signal
/// still ends the process by SIGTERM. Otherwise the thread takes over: it
/// marks termination again (harmless; it is what the order below rests
/// on), logs that the signal arrived (INFO, written off-thread — see
/// [`say_detached`]), waits — at most [`CREDENTIAL_WRITE_DRAIN`], on its
/// own clock — for credential writes still being persisted, and then
/// restores SIGTERM's default disposition and re-raises it: the process
/// dies by the signal, exactly as an unhandled one would, so a cancel, the
/// shell and `systemctl stop` all see a signal death and not an exit code.
/// With nothing pending that happens at once. The run itself is never
/// waited for.
///
/// The thread checks the terminating flag before it waits for the signal:
/// a SIGTERM delivered after the actions were registered but before the
/// thread's signal socket pair existed set the flag and reached nothing
/// else, and is taken as received.
///
/// If the thread cannot be set up (its signal socket pair or the thread
/// itself — EMFILE/ENFILE/EAGAIN), the in-handler actions stay and the
/// exit is committed at once, so a SIGTERM ends the process by the signal
/// immediately, with no drain — never swallowed (signal-hook never runs
/// the default disposition a signal had before its handler); one delivered
/// between the actions' registration and that commit only set the flag,
/// so the commit is followed by a check of it that ends the process by the
/// signal too. The failure is printed on stderr, since it happens before
/// logging is set up. If the flag action cannot be registered, nothing is
/// installed and SIGTERM keeps its default disposition; if only the
/// re-raise action cannot be, the flag stays and the thread is set up as
/// usual — see [`register_signal_actions_at`] for what that loses.
///
/// Commands that shut down gracefully on SIGTERM themselves (`autoflow
/// serve`, `agentiflow serve`) don't install it.
#[cfg(unix)]
pub fn install_sigterm_handler() {
    install_sigterm_handler_at(None, CREDENTIAL_WRITE_DRAIN)
}

/// No-op off unix: there is no SIGTERM.
#[cfg(not(unix))]
pub fn install_sigterm_handler() {}

/// Where [`install_sigterm_handler_at`] is made to fail — the tests of the
/// failure branches (`child_role_entry`) inject these; production passes
/// `None`.
#[cfg(unix)]
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallFailure {
    /// The re-raise action (`register_conditional_default`) cannot be
    /// registered — unreachable for SIGTERM in practice.
    ConditionalDefault,
    /// `Signals::new` fails (EMFILE/ENFILE at its socket pair).
    Signals,
    /// The handler thread cannot be spawned (EAGAIN).
    Spawn,
}

/// [`install_sigterm_handler`] with its failure injection and the drain
/// bound (`drain`: [`CREDENTIAL_WRITE_DRAIN`] in production; the tests
/// shorten it to see the drain give up).
#[cfg(unix)]
fn install_sigterm_handler_at(fail: Option<InstallFailure>, drain: Duration) {
    use signal_hook::consts::SIGTERM;
    let kills_on_commit = match register_signal_actions_at(fail) {
        Ok(kills_on_commit) => kills_on_commit,
        Err(e) => {
            // Nothing is installed: SIGTERM keeps its default disposition.
            eprintln!(
                "rupu: could not install the SIGTERM handler ({e}); a SIGTERM ends the process \
                 at once, with no wait for credential writes"
            );
            return;
        }
    };
    let signals = match fail {
        Some(InstallFailure::Signals) => {
            Err(std::io::Error::other("injected failure: Signals::new"))
        }
        _ => signal_hook::iterator::Signals::new([SIGTERM]),
    };
    let mut signals = match signals {
        Ok(signals) => signals,
        Err(e) => return without_handler_thread(&e, kills_on_commit),
    };
    let spawned = match fail {
        Some(InstallFailure::Spawn) => Err(std::io::Error::other("injected failure: thread spawn")),
        _ => std::thread::Builder::new()
            .name("rupu-sigterm".into())
            .spawn(move || {
                // A SIGTERM delivered after the actions were registered
                // but before `signals` existed set the flag and reached
                // nothing else: taken as received here, or it would never
                // be seen.
                if rupu_providers::credential_writes::terminating()
                    || signals.forever().next().is_some()
                {
                    on_sigterm(drain);
                }
            })
            .map(drop),
    };
    if let Err(e) = spawned {
        without_handler_thread(&e, kills_on_commit);
    }
}

/// The in-handler actions are installed but no thread will drain and
/// re-raise. Left like that, a SIGTERM would only set the flag and the
/// process would ignore `systemctl stop` until SIGKILL. Committing the exit
/// makes the conditional-default action end the process by SIGTERM at once
/// instead — what an uninstalled handler would do, minus the drain. A
/// signal that was delivered between the actions' registration and the
/// commit only set the flag, and nothing else will ever look at it: the
/// commit is followed by a check of the flag that ends the process by the
/// signal (the [`exit_by_signal_if_terminating`] pattern).
///
/// `kills_on_commit` is whether the re-raise action is registered
/// ([`register_signal_actions_at`]). Without it there is nothing left to
/// end the process on a later SIGTERM: the signal only marks the process
/// terminating, a run stops at its next LLM call or tool dispatch and the
/// process then dies by the signal on its way out ([`exit_by_signal_if_terminating`]),
/// while a command that runs no agent (`cp serve`) keeps running until
/// SIGKILL. Said so on stderr.
#[cfg(unix)]
fn without_handler_thread(e: &std::io::Error, kills_on_commit: bool) {
    exit_committed().store(true, Ordering::SeqCst);
    if rupu_providers::credential_writes::terminating() {
        die_by_sigterm();
    }
    if kills_on_commit {
        eprintln!(
            "rupu: could not install the SIGTERM handler ({e}); a SIGTERM ends the process at \
             once, with no wait for credential writes"
        );
    } else {
        eprintln!(
            "rupu: could not install the SIGTERM handler ({e}), and its re-raise could not be \
             registered either; a SIGTERM only marks this process terminating: a run stops at \
             its next LLM call or tool dispatch and the process then ends by the signal on its \
             way out, but a command that runs no agent (`cp serve`) keeps running until SIGKILL"
        );
    }
}

/// The actions that run inside the SIGTERM signal handler itself, in this
/// order (signal-hook runs them in registration order):
///
/// 1. the terminating flag is set (`signal_hook::flag::register`, an
///    atomic store) — so [`exit_by_signal_if_terminating`] sees a signal
///    that was delivered before its check, whether or not the handler
///    thread has woken yet;
/// 2. if the exit is committed ([`exit_committed`]), the default
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
///
/// Returns whether the second action is registered. `Err` only when the
/// first cannot be: nothing is installed then, and SIGTERM keeps its
/// default disposition. If only the second cannot be (unreachable for
/// SIGTERM: signal-hook refuses only the signals it forbids), the first
/// stays — signal-hook's `unregister` removes an action but leaves its
/// handler installed, running nothing, so removing it would leave SIGTERM
/// ignored, not at its default — and the handler thread still drains and
/// re-raises as usual. What is lost is the in-handler kill after the
/// committed exit: a SIGTERM delivered after `main`'s last check then
/// races the exit, and the process may end by its own code. Said so on
/// stderr.
#[cfg(unix)]
fn register_signal_actions_at(fail: Option<InstallFailure>) -> std::io::Result<bool> {
    use signal_hook::consts::SIGTERM;
    let _flag_action = signal_hook::flag::register(
        SIGTERM,
        rupu_providers::credential_writes::terminating_flag(),
    )?;
    let re_raise = match fail {
        Some(InstallFailure::ConditionalDefault) => Err(std::io::Error::other(
            "injected failure: register_conditional_default",
        )),
        _ => signal_hook::flag::register_conditional_default(SIGTERM, exit_committed().clone())
            .map(drop),
    };
    match re_raise {
        Ok(()) => Ok(true),
        Err(e) => {
            eprintln!(
                "rupu: could not register the SIGTERM re-raise ({e}); a SIGTERM delivered as \
                 this command finishes may let it exit by its own code instead"
            );
            Ok(false)
        }
    }
}

/// What SIGTERM does, on the handler thread. Termination is marked before
/// anything else; the log line and the drain's lines are each written
/// off-thread and waited for at most [`SIGTERM_STDERR_GRACE`], so a
/// blocked stderr can delay neither the mark nor the drain nor the
/// re-raise beyond that per line. `drain` bounds the wait for pending
/// credential writes.
#[cfg(unix)]
fn on_sigterm(drain: Duration) -> ! {
    rupu_providers::credential_writes::request_termination();
    say_detached(
        || {
            tracing::info!(
                "SIGTERM received; terminating by it once pending credential writes are drained"
            )
        },
        Some(SIGTERM_STDERR_GRACE),
    );
    drain_credential_writes_blocking(drain);
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

    /// A message whose writer blocks (here: forever) costs the caller at
    /// most the grace, and nothing without one.
    #[test]
    fn say_detached_never_blocks_the_caller_beyond_the_grace() {
        let started = Instant::now();
        say_detached(
            || std::thread::sleep(Duration::from_secs(30)),
            Some(Duration::from_millis(100)),
        );
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(100) && waited < Duration::from_secs(2),
            "bounded by the grace: {waited:?}"
        );
        let started = Instant::now();
        say_detached(|| std::thread::sleep(Duration::from_secs(30)), None);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "no grace: no wait ({:?})",
            started.elapsed()
        );
    }

    // ---- the signal-handler-side actions, in a child process ---------------
    //
    // The closest seam to "the flag is set and the committed exit is killed
    // inside the signal handler, with no help from the handler thread", and
    // to the install's failure branches: this test binary re-runs itself,
    // driven by `RUPU_EXIT_SEAM_ROLE`, with exactly the parts under test
    // installed.

    /// The child side. A no-op unless this process was spawned as a role.
    #[cfg(unix)]
    #[test]
    fn child_role_entry() {
        let Ok(role) = std::env::var("RUPU_EXIT_SEAM_ROLE") else {
            return;
        };
        match role.as_str() {
            // Only the in-handler actions, no thread; `main`'s last step
            // done: committed, no signal yet. Then the SIGTERM must end the
            // process by itself.
            "committed" => {
                register_signal_actions_at(None).expect("register the signal actions");
                exit_by_signal_if_terminating();
                eprintln!("seam: committed");
                std::thread::sleep(Duration::from_secs(10));
                std::process::exit(0)
            }
            // Only the in-handler actions, not committed: the signal only
            // sets the flag, which this process reports by its exit code
            // (7 = set, 8 = never set).
            "flag-only" => {
                register_signal_actions_at(None).expect("register the signal actions");
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
            // The install with its signal socket pair, or its thread,
            // failing: a SIGTERM must still end the process by the signal.
            "signals-fail" | "spawn-fail" => {
                install_sigterm_handler_at(Some(injected_failure(&role)), CREDENTIAL_WRITE_DRAIN);
                eprintln!("seam: installed");
                std::thread::sleep(Duration::from_secs(10));
                std::process::exit(0)
            }
            // The re-raise action failing to register: the flag action
            // stays and the handler thread is installed as usual, so a
            // SIGTERM must still end the process by the signal (through
            // the thread).
            "conditional-fail" => {
                install_sigterm_handler_at(
                    Some(InstallFailure::ConditionalDefault),
                    CREDENTIAL_WRITE_DRAIN,
                );
                eprintln!("seam: installed");
                std::thread::sleep(Duration::from_secs(10));
                std::process::exit(0)
            }
            // A SIGTERM delivered after the actions were registered but
            // before the handler thread's signal socket pair existed: it
            // set the flag and reached nothing else. Stood in for by the
            // flag being set before the install; the thread must take it as
            // received and end the process by the signal with no signal
            // ever sent by the parent.
            "signalled-before-thread" => {
                rupu_providers::credential_writes::request_termination();
                install_sigterm_handler_at(None, CREDENTIAL_WRITE_DRAIN);
                std::thread::sleep(Duration::from_secs(10));
                std::process::exit(0)
            }
            // The same signal, with the thread's set-up failing: it was
            // delivered before the exit got committed, so nothing would
            // ever act on it unless the commit is followed by a check.
            "signalled-before-commit:signals-fail" | "signalled-before-commit:spawn-fail" => {
                rupu_providers::credential_writes::request_termination();
                let failure = injected_failure(role.rsplit(':').next().unwrap());
                install_sigterm_handler_at(Some(failure), CREDENTIAL_WRITE_DRAIN);
                std::thread::sleep(Duration::from_secs(10));
                std::process::exit(0)
            }
            // The real handler, a credential write pending for 1.5s, and a
            // stderr nobody reads that is already full (a thread fills it
            // and blocks, holding stderr's lock): the handler thread must
            // still re-raise once the write lands. Progress is reported on
            // stdout, the only readable side.
            "stderr-blocked" => {
                use std::io::Write;
                info_subscriber_on_stderr();
                println!("seam: ready");
                let _ = std::io::stdout().flush();
                std::thread::spawn(|| {
                    let filler = vec![b'x'; 4 << 20];
                    let mut err = std::io::stderr().lock();
                    let _ = err.write_all(&filler);
                });
                let rt = tokio::runtime::Runtime::new().expect("runtime");
                rt.block_on(async {
                    drop(rupu_providers::credential_writes::spawn(
                        tokio::time::sleep(Duration::from_millis(1500)),
                    ));
                    install_sigterm_handler();
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
                std::process::exit(0)
            }
            // The real handler with a 1s drain bound and a credential write
            // that outlasts it (3s), stderr readable: the handler's lines —
            // `SIGTERM received`, `waiting for 1 …`, `gave up … after 1s` —
            // must all reach stderr before the re-raise.
            "drain-gave-up" => {
                use std::io::Write;
                info_subscriber_on_stderr();
                let rt = tokio::runtime::Runtime::new().expect("runtime");
                rt.block_on(async {
                    drop(rupu_providers::credential_writes::spawn(
                        tokio::time::sleep(Duration::from_secs(3)),
                    ));
                    install_sigterm_handler_at(None, Duration::from_secs(1));
                    println!("seam: ready");
                    let _ = std::io::stdout().flush();
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
                std::process::exit(0)
            }
            other => panic!("unknown role {other:?}"),
        }
    }

    /// The injected failure a `*-fail` role name stands for.
    #[cfg(unix)]
    fn injected_failure(role: &str) -> InstallFailure {
        match role {
            "signals-fail" => InstallFailure::Signals,
            "spawn-fail" => InstallFailure::Spawn,
            other => panic!("no injected failure for {other:?}"),
        }
    }

    /// As the real binary under `RUPU_LOG=info`: the handler's INFO line is
    /// a stderr write.
    #[cfg(unix)]
    fn info_subscriber_on_stderr() {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::INFO)
            .init();
    }

    /// One of the child's pipes.
    #[cfg(unix)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pipe {
        Stderr,
        Stdout,
    }

    /// How the parent drives a child role ([`run_role`]).
    #[cfg(unix)]
    struct RoleRun<'a> {
        /// The `seam: <marker>` line to wait for, and the pipe it comes on;
        /// `None`: don't wait for one.
        marker: Option<(&'a str, Pipe)>,
        /// The pipe read to EOF and returned. A pipe named neither here nor
        /// by `marker` is never read.
        collect: Option<Pipe>,
        /// Waited after the marker, before the signal.
        settle: Duration,
        /// Send SIGTERM; `false`: just wait for the child to end by itself.
        sigterm: bool,
    }

    #[cfg(unix)]
    impl RoleRun<'_> {
        /// Wait for `seam: <marker>` on `on`, then SIGTERM; read nothing else.
        fn sigterm_after(marker: &str, on: Pipe) -> RoleRun<'_> {
            RoleRun {
                marker: Some((marker, on)),
                collect: None,
                settle: Duration::ZERO,
                sigterm: true,
            }
        }

        /// Send no signal, read nothing: the child ends by itself.
        fn hands_off() -> RoleRun<'static> {
            RoleRun {
                marker: None,
                collect: None,
                settle: Duration::ZERO,
                sigterm: false,
            }
        }
    }

    /// How a child role ended.
    #[cfg(unix)]
    struct RoleOutcome {
        status: std::process::ExitStatus,
        /// From the signal (or, without one, from the spawn) to the end.
        took: Duration,
        /// Every line of the `collect` pipe, to EOF.
        collected: Vec<String>,
    }

    /// Spawn this test binary as `role` and drive it as `run` says: wait
    /// for its marker, wait `settle`, send it SIGTERM (or not), wait for it
    /// to end, and read its `collect` pipe to EOF.
    #[cfg(unix)]
    fn run_role(role: &str, run: RoleRun<'_>) -> RoleOutcome {
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
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the child role");
        let started = Instant::now();
        let marker_on = run.marker.map(|(_, on)| on);
        let (tx, rx) = std::sync::mpsc::channel::<(Pipe, String)>();
        let mut readers = Vec::new();
        for on in [Pipe::Stdout, Pipe::Stderr] {
            if marker_on != Some(on) && run.collect != Some(on) {
                continue;
            }
            let reader: Box<dyn std::io::Read + Send> = match on {
                Pipe::Stderr => Box::new(child.stderr.take().unwrap()),
                Pipe::Stdout => Box::new(child.stdout.take().unwrap()),
            };
            let tx = tx.clone();
            readers.push(std::thread::spawn(move || {
                for line in std::io::BufReader::new(reader)
                    .lines()
                    .map_while(Result::ok)
                {
                    let _ = tx.send((on, line));
                }
            }));
        }
        drop(tx);
        let mut collected = Vec::new();
        if let Some((marker, on)) = run.marker {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok((from, line)) => {
                        let hit = from == on && line.contains(&format!("seam: {marker}"));
                        if run.collect == Some(from) {
                            collected.push(line);
                        }
                        if hit {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = child.kill();
                        panic!("the child never reported {marker:?}: {e}");
                    }
                }
            }
        }
        std::thread::sleep(run.settle);
        let signalled_at = if run.sigterm {
            let killed = std::process::Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status()
                .unwrap();
            assert!(killed.success());
            Instant::now()
        } else {
            started
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!(
                    "the child did not end{}",
                    if run.sigterm { " after SIGTERM" } else { "" }
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let took = signalled_at.elapsed();
        // The readers end at EOF, once the child is gone.
        for (from, line) in rx {
            if run.collect == Some(from) {
                collected.push(line);
            }
        }
        for reader in readers {
            let _ = reader.join();
        }
        RoleOutcome {
            status,
            took,
            collected,
        }
    }

    /// The terminating flag is set inside the signal handler itself: a
    /// process with the actions installed and no handler thread sees
    /// `terminating()` flip on delivery.
    #[cfg(unix)]
    #[test]
    fn the_signal_handler_itself_sets_the_terminating_flag() {
        let outcome = run_role("flag-only", RoleRun::sigterm_after("ready", Pipe::Stderr));
        assert_eq!(
            outcome.status.code(),
            Some(7),
            "the flag was set on delivery, with no handler thread: {:?}",
            outcome.status
        );
    }

    /// Once `main` has committed to exit, a SIGTERM ends the process by the
    /// signal right inside the handler — with no handler thread to re-raise
    /// it, and before the committed exit (10s away here) can happen.
    #[cfg(unix)]
    #[test]
    fn a_sigterm_after_the_committed_exit_kills_by_the_signal_in_the_handler() {
        use std::os::unix::process::ExitStatusExt;
        let outcome = run_role(
            "committed",
            RoleRun::sigterm_after("committed", Pipe::Stderr),
        );
        assert_eq!(
            outcome.status.signal(),
            Some(libc_sigterm()),
            "died by SIGTERM inside the handler, not by the exit code: {:?}",
            outcome.status
        );
        assert_eq!(outcome.status.code(), None);
    }

    /// A handler whose thread could not be set up (the signal socket pair,
    /// or the thread itself) never swallows SIGTERM: the process still dies
    /// by the signal, at once.
    #[cfg(unix)]
    #[test]
    fn a_failed_handler_install_still_dies_by_sigterm() {
        use std::os::unix::process::ExitStatusExt;
        for role in ["signals-fail", "spawn-fail"] {
            let outcome = run_role(role, RoleRun::sigterm_after("installed", Pipe::Stderr));
            assert_eq!(
                outcome.status.signal(),
                Some(libc_sigterm()),
                "{role}: died by SIGTERM, not swallowed and not by the exit code: {:?}",
                outcome.status
            );
            assert!(
                outcome.took < Duration::from_secs(5),
                "{role}: at once, with no drain ({:?})",
                outcome.took
            );
        }
    }

    /// A SIGTERM delivered after the in-handler actions were registered but
    /// before the handler thread could set itself up — the flag set, no
    /// thread to see it — is still acted on: the thread, once up, takes
    /// the flag as the signal received and ends the process by it (nothing
    /// is sent by the parent).
    #[cfg(unix)]
    #[test]
    fn a_sigterm_delivered_before_the_handler_thread_existed_is_taken_as_received() {
        use std::os::unix::process::ExitStatusExt;
        let outcome = run_role("signalled-before-thread", RoleRun::hands_off());
        assert_eq!(
            outcome.status.signal(),
            Some(libc_sigterm()),
            "ended by the SIGTERM the flag stood for: {:?}",
            outcome.status
        );
        assert!(
            outcome.took < Duration::from_secs(5),
            "at once, nothing pending ({:?})",
            outcome.took
        );
    }

    /// The same early SIGTERM when the thread's set-up then fails: it was
    /// delivered before the exit got committed, so the in-handler kill
    /// never saw a committed exit — the commit must be followed by a check
    /// of the flag that ends the process by the signal.
    #[cfg(unix)]
    #[test]
    fn a_sigterm_delivered_before_the_failed_install_committed_still_ends_the_process() {
        use std::os::unix::process::ExitStatusExt;
        for role in [
            "signalled-before-commit:signals-fail",
            "signalled-before-commit:spawn-fail",
        ] {
            let outcome = run_role(role, RoleRun::hands_off());
            assert_eq!(
                outcome.status.signal(),
                Some(libc_sigterm()),
                "{role}: ended by the SIGTERM the flag stood for, not swallowed: {:?}",
                outcome.status
            );
            assert!(
                outcome.took < Duration::from_secs(5),
                "{role}: at once ({:?})",
                outcome.took
            );
        }
    }

    /// When only the re-raise action cannot be registered, the flag action
    /// stays and the handler thread is installed: a SIGTERM still ends the
    /// process by the signal (through the thread's drain and re-raise),
    /// where removing the flag action would have left signal-hook's handler
    /// installed with nothing to run — SIGTERM ignored.
    #[cfg(unix)]
    #[test]
    fn a_failed_re_raise_registration_keeps_the_flag_and_the_thread() {
        use std::os::unix::process::ExitStatusExt;
        let outcome = run_role(
            "conditional-fail",
            RoleRun::sigterm_after("installed", Pipe::Stderr),
        );
        assert_eq!(
            outcome.status.signal(),
            Some(libc_sigterm()),
            "died by SIGTERM through the handler thread: {:?}",
            outcome.status
        );
        assert!(
            outcome.took < Duration::from_secs(5),
            "nothing pending: at once ({:?})",
            outcome.took
        );
    }

    /// A stderr nobody reads, already full, with a credential write pending:
    /// the handler thread still waits for the write and re-raises — its
    /// own output never blocks the drain or the re-raise.
    #[cfg(unix)]
    #[test]
    fn a_blocked_stderr_does_not_stall_the_re_raise() {
        use std::os::unix::process::ExitStatusExt;
        // 500ms for the filler thread to fill the pipe and block on it.
        let outcome = run_role(
            "stderr-blocked",
            RoleRun {
                marker: Some(("ready", Pipe::Stdout)),
                collect: None,
                settle: Duration::from_millis(500),
                sigterm: true,
            },
        );
        assert_eq!(
            outcome.status.signal(),
            Some(libc_sigterm()),
            "died by SIGTERM once the write landed: {:?}",
            outcome.status
        );
        assert!(
            outcome.took >= Duration::from_millis(900) && outcome.took < Duration::from_secs(12),
            "held by the pending write, then re-raised within the bound ({:?})",
            outcome.took
        );
    }

    /// With a readable stderr, every line the handler thread writes on its
    /// way to the re-raise is there — the `gave up` line included, which
    /// is written right before the re-raise and is the only word the
    /// operator gets that a rotation may have been lost.
    #[cfg(unix)]
    #[test]
    fn the_handler_s_lines_reach_a_readable_stderr_before_the_re_raise() {
        use std::os::unix::process::ExitStatusExt;
        let outcome = run_role(
            "drain-gave-up",
            RoleRun {
                marker: Some(("ready", Pipe::Stdout)),
                collect: Some(Pipe::Stderr),
                settle: Duration::ZERO,
                sigterm: true,
            },
        );
        assert_eq!(
            outcome.status.signal(),
            Some(libc_sigterm()),
            "died by SIGTERM after the drain gave up: {:?}",
            outcome.status
        );
        assert!(
            outcome.took >= Duration::from_secs(1) && outcome.took < Duration::from_secs(5),
            "the 1s drain bound, then the re-raise ({:?})",
            outcome.took
        );
        let stderr = outcome.collected.join("\n");
        for line in [
            "SIGTERM received",
            "rupu: waiting for 1 credential write(s) to finish",
            "rupu: gave up waiting for credential writes after 1s",
        ] {
            assert!(
                stderr.contains(line),
                "{line:?} missing from stderr:\n{stderr}"
            );
        }
    }

    #[cfg(unix)]
    fn libc_sigterm() -> i32 {
        signal_hook::consts::SIGTERM
    }
}
