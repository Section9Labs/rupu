//! Test support for "no blocking file I/O on the async runtime": drive an
//! operation whose file I/O is held up, and assert the runtime stays live —
//! or that the operation waits for the blocking pool — meanwhile.
//!
//! * [`assert_runtime_live_while_parked_on`] parks the I/O on a FIFO, whose
//!   `open` waits for its peer. It reaches any operation that OPENS the file.
//! * [`assert_waits_for_the_blocking_pool`] holds the only thread of a
//!   [`one_blocking_thread_runtime`]'s blocking pool, so a hop has to wait
//!   for it. It reaches what no FIFO can park — a bare `stat` or `mkdir`, or
//!   a reader that refuses a non-regular file before opening it.

/// Put a FIFO at `path` (replacing any file there). `false` when
/// `mkfifo` is unavailable; the caller then skips.
pub(crate) fn make_fifo(path: &std::path::Path) -> bool {
    let _ = std::fs::remove_file(path);
    std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .is_ok_and(|s| s.success())
}

/// Serve the end of `fifo` that its parked opener waits for, from when
/// `release` is set (or a 5 s watchdog passes) until `done` is set.
/// Never blocks:
///
/// * the opener WRITES: hold a non-blocking read end. The writer's open
///   returns, it writes and closes; nobody waits for an EOF.
/// * the opener READS: open a non-blocking write end and close it again,
///   every few ms (ENXIO until the reader has opened). Each close is an
///   EOF for the reader. One is not enough: macOS loses it when the close
///   lands while the reader is still waking from its open (about 0.2–0.8%
///   of single handoffs measured), and the next handoff gets through.
fn serve_fifo_peer(
    fifo: std::path::PathBuf,
    opener_writes: bool,
    release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use rustix::fs::{Mode, OFlags};
    use std::sync::atomic::Ordering::SeqCst;
    let tick = std::time::Duration::from_millis(5);
    let watchdog = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !release.load(SeqCst) && std::time::Instant::now() < watchdog {
        std::thread::sleep(tick);
    }
    let _read_end = opener_writes
        .then(|| rustix::fs::open(&fifo, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty()));
    while !done.load(SeqCst) {
        if !opener_writes {
            let _ = rustix::fs::open(&fifo, OFlags::WRONLY | OFlags::NONBLOCK, Mode::empty());
        }
        std::thread::sleep(tick);
    }
}

/// Drive `op`, whose file I/O will park on the FIFO at `fifo` until its
/// peer opens, and assert that the runtime stays live meanwhile: on a
/// current-thread runtime a timer must fire while `op` is still pending.
/// Blocking I/O on the runtime thread freezes the timer, so `op` would
/// instead complete in its first poll once the watchdog serves the peer
/// (after 5 s) — a failure, not a hang.
pub(crate) async fn assert_runtime_live_while_parked_on<T>(
    op: impl std::future::Future<Output = T>,
    fifo: &std::path::Path,
    opener_writes: bool,
) -> T {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::Arc;
    /// Stops the peer however the caller leaves, the panic below included.
    struct Stop(Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, SeqCst);
        }
    }
    let (release, done) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let peer = {
        let (fifo, release, done) = (fifo.to_path_buf(), Arc::clone(&release), Arc::clone(&done));
        std::thread::spawn(move || serve_fifo_peer(fifo, opener_writes, release, done))
    };
    let stop = Stop(done);
    tokio::pin!(op);
    tokio::select! {
        biased;
        _ = &mut op => panic!(
            "finished before its FIFO had a peer: the I/O ran on the runtime \
             thread and froze it until the watchdog served the peer"
        ),
        _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
    }
    release.store(true, SeqCst);
    let out = op.await;
    drop(stop);
    peer.join().unwrap();
    out
}

/// A current-thread runtime whose blocking pool has ONE thread, for
/// [`HeldBlockingThread`].
pub(crate) fn one_blocking_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
}

/// The only thread of a [`one_blocking_thread_runtime`]'s blocking pool,
/// held from [`Self::hold`] until [`Self::release`] (or drop): every hop
/// submitted meanwhile queues behind it.
pub(crate) struct HeldBlockingThread {
    release: std::sync::mpsc::Sender<()>,
    busy: tokio::task::JoinHandle<()>,
}

impl HeldBlockingThread {
    pub(crate) fn hold() -> Self {
        let (release, parked) = std::sync::mpsc::channel::<()>();
        let busy = tokio::task::spawn_blocking(move || {
            // Also returns when the sender drops, so a failed test never
            // leaves the runtime's shutdown waiting on this thread.
            let _ = parked.recv();
        });
        Self { release, busy }
    }

    pub(crate) async fn release(self) {
        let _ = self.release.send(());
        self.busy.await.unwrap();
    }
}

/// Drive `op` while `held` holds the runtime's only blocking thread, and
/// assert that it does not finish until that thread is free: its I/O waits
/// for a hop. I/O on the runtime thread instead lets `op` finish in its first
/// poll — a failure, not a hang. Meaningful for I/O that comes before any
/// other hop `op` makes: a later hop would hold `op` back on its own.
pub(crate) async fn assert_waits_for_the_blocking_pool<T>(
    op: impl std::future::Future<Output = T>,
    held: HeldBlockingThread,
) -> T {
    tokio::pin!(op);
    tokio::select! {
        biased;
        _ = &mut op => panic!(
            "finished while the runtime's only blocking thread was held: its \
             I/O ran on the runtime thread, not in a hop to the blocking pool"
        ),
        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
    }
    held.release().await;
    op.await
}
