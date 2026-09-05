//! Spec §5.1: one `tail -n +1 -F` over ssh per distinct cache file, shared by
//! every viewer of that file, killed when the last viewer leaves, never
//! opened for a file that already has a `.complete` sidecar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::StreamExt as _;
use tokio::sync::watch;

use super::ssh::{parse_tail_marker, shell_escape, RemoteExec, RemoteExecError};
use super::transcript_paths::is_complete;

/// How long `subscribe` will wait for a just-replaced feed's task to
/// confirm it has actually stopped writing before truncating the cache out
/// from under it. A cooperative `abort()` only takes effect at the aborted
/// task's next await point, so this is a real (if generous) wait, not a
/// formality — see [`FeedDone`]. A timeout just proceeds; it trades a
/// vanishingly rare stuck-task edge case for never hanging a viewer.
const REPLACE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Refcount + liveness for one shared remote tail. Every subscriber holds an
/// `Arc`; the feeding task is aborted on drop of the last one, which drops
/// the ssh child through `kill_on_drop`.
pub(crate) struct FeedHandle {
    task: tokio::task::JoinHandle<()>,
    alive: Arc<AtomicBool>,
}

impl FeedHandle {
    /// `false` once the remote stream ended (ssh dropped, remote `tail`
    /// died, or the task was aborted). The cache file is left as-is; a
    /// later subscribe replaces the feed and replays from byte zero.
    pub(crate) fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for FeedHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Held by the feeding task for its entire body. `JoinHandle::abort()` is
/// cooperative: a task that is actively running (not suspended at an
/// `.await`) finishes its current poll — including any in-flight
/// synchronous `writeln!`/`flush()` — before the cancellation actually
/// drops its future. Because Rust only runs a value's `Drop` once nothing
/// is still executing inside it, this guard's `drop` is the one moment
/// that is guaranteed to run *after* any write the task was mid-way
/// through, on every exit path (normal stream end, panic, or abort alike).
/// `subscribe` waits on `finished` before truncating the same cache file
/// for a replacement feed, so a stale task's tail end can never land after
/// the new feed has already started writing (spec §5.1: no duplicate
/// lines across a replaced feed).
struct FeedDone {
    alive: Arc<AtomicBool>,
    finished: watch::Sender<bool>,
}

impl Drop for FeedDone {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.finished.send(true);
    }
}

/// One registry entry: a weak handle so the registry itself never keeps a
/// feed alive, plus three things kept independently of the `Weak` — once the
/// last `Arc<FeedHandle>` is dropped the `Weak` can no longer be upgraded, so
/// these are the only way the registry can still act on that feed:
///
/// * `finished` — how `subscribe` learns the task has actually stopped;
/// * `abort` / `alive` — how [`LazyTailRegistry::retire`] stops a feed the
///   registry does not hold a strong handle to. `FeedHandle::drop` aborts on
///   the LAST subscriber's drop, which is the wrong moment for a terminal
///   pull: viewers are still attached, and their feed must stop writing
///   before the authoritative body is put in place.
struct FeedEntry {
    handle: Weak<FeedHandle>,
    finished: watch::Receiver<bool>,
    abort: tokio::task::AbortHandle,
    alive: Arc<AtomicBool>,
}

pub(crate) struct LazyTailRegistry {
    exec: Arc<dyn RemoteExec>,
    feeds: Mutex<HashMap<PathBuf, FeedEntry>>,
}

impl LazyTailRegistry {
    pub(crate) fn new(exec: Arc<dyn RemoteExec>) -> Self {
        Self {
            exec,
            feeds: Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe to `remote` being tailed into `cache`.
    ///
    /// * `Ok(None)` — `cache` is already complete; nothing to tail.
    /// * `Ok(Some(handle))` — hold it for as long as the viewer is attached.
    ///   The first subscriber spawns `tail -n +1 -F` and the feeding task
    ///   truncates the cache when its FIRST line arrives — a byte-zero
    ///   replay, which is what makes the truncate safe. Truncating here
    ///   instead would destroy whatever was already collected whenever the
    ///   spawn fails or the host never answers, handing the viewer an empty
    ///   page flagged partial; deferring it costs nothing, because the line
    ///   that authorises the truncate is the first line of the same replay.
    ///   Later subscribers share the running feed; a dead feed is replaced —
    ///   after confirming, via [`FeedDone`], that the old feed's task has
    ///   actually stopped writing.
    pub(crate) async fn subscribe(
        &self,
        remote: &str,
        cache: &Path,
    ) -> Result<Option<Arc<FeedHandle>>, RemoteExecError> {
        if is_complete(cache) {
            return Ok(None);
        }

        // Fast path: an existing, live feed is shared without waiting.
        let wait_rx = {
            let feeds = self.feeds.lock().unwrap();
            match feeds.get(cache) {
                Some(entry) => {
                    if let Some(existing) = entry.handle.upgrade() {
                        if existing.alive() {
                            return Ok(Some(existing));
                        }
                    }
                    Some(entry.finished.clone())
                }
                None => None,
            }
        };

        // No usable feed. A previous one may still be tearing down — wait
        // (bounded) for its `FeedDone` guard to fire before truncating the
        // same cache file underneath it; see `FeedDone`'s doc comment for
        // why this is a real race, not a formality. Never await while
        // holding the std `Mutex` (clippy::await_holding_lock) — the lock
        // above is already released by the time we get here.
        if let Some(mut rx) = wait_rx {
            let _ = tokio::time::timeout(REPLACE_WAIT_TIMEOUT, rx.wait_for(|done| *done)).await;
        }

        let mut feeds = self.feeds.lock().unwrap();
        // Dead entries are never otherwise removed, so the map would grow for
        // the process's lifetime. Prune on the slow path only — the fast path
        // above must stay lock-and-return.
        feeds.retain(|_, e| e.handle.strong_count() > 0);
        // Re-check completeness UNDER the lock. The check at the top of this
        // function ran before the (awaited) wait for the old feed to stop, and
        // a terminal pull can write the `.complete` sidecar in that window. A
        // complete cache is authoritative: never tail it.
        if is_complete(cache) {
            return Ok(None);
        }
        // Someone else may have installed a fresh, live feed while we
        // waited — join it rather than spawning a redundant second tail.
        if let Some(entry) = feeds.get(cache) {
            if let Some(existing) = entry.handle.upgrade() {
                if existing.alive() {
                    return Ok(Some(existing));
                }
            }
        }

        if let Some(dir) = cache.parent() {
            std::fs::create_dir_all(dir).map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        }
        let cmd = format!("tail -n +1 -F {}", shell_escape(remote));
        let mut stream = self.exec.spawn_lines(&cmd)?;
        let alive = Arc::new(AtomicBool::new(true));
        let (finished_tx, finished_rx) = watch::channel(false);
        let done = FeedDone {
            alive: Arc::clone(&alive),
            finished: finished_tx,
        };
        let cache_owned = cache.to_path_buf();
        let alive_task = Arc::clone(&alive);
        let task = tokio::spawn(async move {
            // Held for the whole body: its `Drop` is what tells a
            // replacing `subscribe` this task will never write again.
            let _done = done;
            use std::io::Write as _;
            // Opened lazily on the first real line — see `subscribe`'s doc
            // comment: until the remote has proven it can deliver, the
            // already-collected partial content stays on disk untouched.
            let mut file: Option<std::fs::File> = None;
            while let Some(Ok(line)) = stream.next().await {
                if parse_tail_marker(&line).is_some() || line.trim().is_empty() {
                    continue;
                }
                if file.is_none() {
                    // The cache became complete (terminal pull finished and
                    // wrote the sidecar) while we waited for the first remote
                    // line. The pull retired us or raced our spawn — never
                    // truncate a complete file.
                    if is_complete(&cache_owned) {
                        alive_task.store(false, Ordering::SeqCst);
                        return;
                    }
                    // Truncate only now, on the first real line, so a feed that
                    // never yields (host unreachable) leaves previously collected
                    // content intact; the replay that follows starts from byte zero.
                    if std::fs::File::create(&cache_owned).is_err() {
                        return;
                    }
                    let Ok(opened) = std::fs::OpenOptions::new().append(true).open(&cache_owned)
                    else {
                        return;
                    };
                    file = Some(opened);
                }
                let Some(f) = file.as_mut() else { return };
                if writeln!(f, "{line}").and_then(|_| f.flush()).is_err() {
                    break;
                }
            }
        });
        let abort = task.abort_handle();
        let handle = Arc::new(FeedHandle {
            task,
            alive: Arc::clone(&alive),
        });
        feeds.insert(
            cache.to_path_buf(),
            FeedEntry {
                handle: Arc::downgrade(&handle),
                finished: finished_rx,
                abort,
                alive,
            },
        );
        Ok(Some(handle))
    }

    /// Stop any feed tailing into `cache` and forget it, so the caller can
    /// rewrite the file as its sole writer.
    ///
    /// The terminal pull is authoritative and its result is marked
    /// `.complete`, after which the read path serves the cache verbatim and
    /// `subscribe` refuses to tail it — so a wrong byte written at that moment
    /// is never repaired. Letting the feed and the pull share the file is what
    /// makes wrong bytes possible: the feed's in-flight lines land after the
    /// authoritative body, or its first-line truncate lands on top of it, or a
    /// truncate inside the pull's `write_all` leaves a NUL hole. Retiring the
    /// feed first removes the second writer entirely; the pull then does its
    /// ordinary atomic tmp+rename.
    ///
    /// Existing subscribers keep their `Arc<FeedHandle>` (it just reports
    /// `alive() == false`) and their `TranscriptTail` keeps reading the cache
    /// BY PATH at a byte offset. After the pull's rename that path is the
    /// authoritative file, a strict superset of the replay bytes already
    /// delivered, so the stream continues cleanly rather than breaking.
    ///
    /// Idempotent: retiring a path with no feed does nothing.
    pub(crate) async fn retire(&self, cache: &Path) {
        let entry = {
            let mut feeds = self.feeds.lock().unwrap();
            feeds.remove(cache)
        };
        let Some(entry) = entry else { return };
        // Before the abort lands, so `has_live_feed` and `subscribe`'s
        // liveness checks stop reporting this feed immediately.
        entry.alive.store(false, Ordering::SeqCst);
        entry.abort.abort();
        // `abort()` is cooperative — a task mid-`writeln!` finishes that write
        // first. Wait for its `FeedDone` guard, exactly as the replace path
        // does, so the caller really is the only writer when this returns.
        let mut rx = entry.finished;
        let _ = tokio::time::timeout(REPLACE_WAIT_TIMEOUT, rx.wait_for(|done| *done)).await;
    }

    /// Is a feed currently tailing into `cache` AND still alive?
    ///
    /// A live feed holds an append handle on `cache`'s inode. Any writer that
    /// replaces the file by `rename` would swap the dentry out from under it,
    /// leaving the feed appending to an unlinked inode while readers open the
    /// path — the SSE stream goes permanently quiet, and `alive()` stays
    /// `true`, so later subscribers join the same zombie. Callers that are
    /// about to write `cache` ask this first: `pull_transcript` steps aside
    /// entirely (the feed is already filling the file), and the terminal pull
    /// retires the feed (`retire`) before its atomic rename.
    pub(crate) fn has_live_feed(&self, cache: &Path) -> bool {
        let feeds = self.feeds.lock().unwrap();
        feeds
            .get(cache)
            .and_then(|e| e.handle.upgrade())
            .is_some_and(|h| h.alive())
    }

    #[cfg(test)]
    pub(crate) fn live_feeds(&self) -> usize {
        self.feeds
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.handle.strong_count() > 0)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ssh::{LineStream, RemoteOutput};

    /// Scripted `spawn_lines`: replays `lines`, then either hangs (a real
    /// `tail -F`) or ends (a dropped ssh session). Counts spawns.
    struct ScriptedExec {
        lines: Vec<String>,
        hang: bool,
        spawns: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl RemoteExec for ScriptedExec {
        async fn run(&self, _c: &str) -> Result<RemoteOutput, RemoteExecError> {
            unimplemented!("lazy tail never calls run")
        }
        fn spawn_lines(&self, c: &str) -> Result<LineStream, RemoteExecError> {
            self.spawns.lock().unwrap().push(c.to_string());
            let items: Vec<std::io::Result<String>> = self.lines.iter().cloned().map(Ok).collect();
            let head = futures_util::stream::iter(items);
            if self.hang {
                Ok(Box::pin(head.chain(futures_util::stream::pending())))
            } else {
                Ok(Box::pin(head))
            }
        }
        async fn run_bytes(
            &self,
            _c: &str,
            _s: Option<Vec<u8>>,
        ) -> Result<Vec<u8>, RemoteExecError> {
            unimplemented!()
        }
    }

    fn exec(lines: &[&str], hang: bool) -> Arc<ScriptedExec> {
        Arc::new(ScriptedExec {
            lines: lines.iter().map(|s| s.to_string()).collect(),
            hang,
            spawns: Default::default(),
        })
    }

    async fn wait_for_content(path: &Path, needle: &str) {
        for _ in 0..100 {
            if std::fs::read_to_string(path)
                .map(|s| s.contains(needle))
                .unwrap_or(false)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("{needle:?} never appeared in {}", path.display());
    }

    #[tokio::test]
    async fn first_subscriber_truncates_then_replays_from_the_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("mirror/h/transcripts/run_01A.jsonl");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, "STALE LINE FROM A PREVIOUS TAIL\n").unwrap();
        let ex = exec(
            &[r#"{"type":"run_start"}"#, r#"{"type":"turn_start"}"#],
            true,
        );
        let reg = LazyTailRegistry::new(ex.clone());

        let guard = reg
            .subscribe("/remote/.rupu/transcripts/run_01A.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "turn_start").await;
        let got = std::fs::read_to_string(&cache).unwrap();
        assert!(
            !got.contains("STALE"),
            "cache must be truncated before replay: {got:?}"
        );
        assert_eq!(got, "{\"type\":\"run_start\"}\n{\"type\":\"turn_start\"}\n");
        let spawns = ex.spawns.lock().unwrap();
        assert_eq!(spawns.len(), 1);
        assert_eq!(
            spawns[0],
            "tail -n +1 -F '/remote/.rupu/transcripts/run_01A.jsonl'"
        );
        drop(guard);
    }

    /// Issue 4: `subscribe` used to truncate the cache before the tail had
    /// proven it could start. A spawn that never yields (host unreachable,
    /// remote `tail` that never produces a line) then left the viewer with an
    /// EMPTY page flagged partial, having destroyed content a previous pull
    /// had already collected. The truncate now waits for the first line.
    #[tokio::test]
    async fn stale_content_survives_a_feed_that_never_yields_a_line() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01STALE.jsonl");
        std::fs::write(&cache, "PARTIAL CONTENT FROM AN EARLIER PULL\n").unwrap();
        // Yields nothing, then pends forever: the remote never answers.
        let ex = exec(&[], true);
        let reg = LazyTailRegistry::new(ex.clone());

        let _guard = reg
            .subscribe("/r/run_01STALE.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        // Give the task ample opportunity to (wrongly) truncate.
        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "PARTIAL CONTENT FROM AN EARLIER PULL\n",
            "nothing arrived from the remote, so the already-collected \
             content must still be on disk"
        );
    }

    /// `retire` is the sole-writer handoff the terminal pull needs: the feed
    /// must be stopped, and confirmed stopped, before the authoritative body
    /// replaces the file — `FeedHandle::drop` only fires on the LAST
    /// subscriber's drop, which is the wrong moment (viewers are still
    /// attached).
    #[tokio::test]
    async fn retire_aborts_the_feed_and_removes_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01RETIRE.jsonl");
        let ex = exec(&[r#"{"type":"run_start"}"#], true);
        let reg = LazyTailRegistry::new(ex.clone());

        // Retiring a path with no feed is a no-op, not a panic.
        reg.retire(&cache).await;

        // The subscriber keeps holding its guard across the retire — that is
        // the whole point: viewers stay attached and keep reading the path.
        let guard = reg
            .subscribe("/r/run_01RETIRE.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "run_start").await;
        assert!(guard.alive());
        assert_eq!(reg.live_feeds(), 1);

        reg.retire(&cache).await;
        assert!(!guard.alive(), "the still-held handle reports dead");
        assert!(!reg.has_live_feed(&cache));
        assert_eq!(reg.live_feeds(), 0, "the entry is gone");

        // …and the registry is clean enough to tail the file again.
        let second = reg
            .subscribe("/r/run_01RETIRE.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        assert!(!Arc::ptr_eq(&guard, &second));
        assert_eq!(ex.spawns.lock().unwrap().len(), 2);
    }

    /// C1: `has_live_feed` is what a would-be writer of the cache asks before
    /// it renames a new file over the inode a feed is appending to.
    #[tokio::test]
    async fn has_live_feed_tracks_the_holder_and_ignores_a_complete_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01LIVE.jsonl");
        let ex = exec(&[r#"{"type":"run_start"}"#], true);
        let reg = LazyTailRegistry::new(ex.clone());

        assert!(!reg.has_live_feed(&cache), "no feed has been opened yet");
        let guard = reg
            .subscribe("/r/run_01LIVE.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "run_start").await;
        assert!(reg.has_live_feed(&cache), "a held guard is a live feed");
        drop(guard);
        assert!(
            !reg.has_live_feed(&cache),
            "the last guard is gone → no live feed"
        );

        // A complete cache is never tailed, so it never has a feed either.
        let done = tmp.path().join("run_01DONE.jsonl");
        std::fs::write(&done, "{\"type\":\"run_start\"}\n").unwrap();
        std::fs::write(crate::host::transcript_paths::complete_marker(&done), b"").unwrap();
        assert!(reg
            .subscribe("/r/run_01DONE.jsonl", &done)
            .await
            .unwrap()
            .is_none());
        assert!(!reg.has_live_feed(&done));
    }

    #[tokio::test]
    async fn two_subscribers_share_one_remote_tail_and_the_last_drop_kills_it() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01B.jsonl");
        let ex = exec(&[r#"{"type":"run_start"}"#], true);
        let reg = LazyTailRegistry::new(ex.clone());

        let a = reg
            .subscribe("/r/run_01B.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        let b = reg
            .subscribe("/r/run_01B.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(ex.spawns.lock().unwrap().len(), 1);
        assert_eq!(reg.live_feeds(), 1);
        wait_for_content(&cache, "run_start").await;

        drop(a);
        assert_eq!(reg.live_feeds(), 1, "one holder left");
        drop(b);
        assert_eq!(reg.live_feeds(), 0, "last holder gone → feed released");
        // The partial cache stays on disk.
        assert!(cache.exists());
    }

    #[tokio::test]
    async fn a_complete_cache_is_never_tailed() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01C.jsonl");
        std::fs::write(&cache, "{\"type\":\"run_start\"}\n").unwrap();
        std::fs::write(crate::host::transcript_paths::complete_marker(&cache), b"").unwrap();
        let ex = exec(&[], true);
        let reg = LazyTailRegistry::new(ex.clone());

        assert!(reg
            .subscribe("/r/run_01C.jsonl", &cache)
            .await
            .unwrap()
            .is_none());
        assert!(ex.spawns.lock().unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n",
            "not truncated"
        );
    }

    #[tokio::test]
    async fn a_dead_feed_is_replaced_on_the_next_subscribe() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01D.jsonl");
        // `hang: false` → the "ssh session" ends right after replay.
        let ex = exec(&[r#"{"type":"run_start"}"#], false);
        let reg = LazyTailRegistry::new(ex.clone());

        let first = reg
            .subscribe("/r/run_01D.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "run_start").await;
        for _ in 0..100 {
            if !first.alive() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!first.alive(), "feed must report dead once the stream ends");

        // A second viewer, while the first still holds its (dead) handle,
        // gets a fresh tail — which truncates and replays from byte zero.
        let second = reg
            .subscribe("/r/run_01D.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(ex.spawns.lock().unwrap().len(), 2);
        wait_for_content(&cache, "run_start").await;
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n",
            "replay from an empty file: no duplicate lines"
        );
    }

    #[tokio::test]
    async fn tail_headers_are_not_written_into_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01E.jsonl");
        let ex = exec(
            &["==> /r/run_01E.jsonl <==", r#"{"type":"run_start"}"#, ""],
            true,
        );
        let reg = LazyTailRegistry::new(ex.clone());
        let _g = reg
            .subscribe("/r/run_01E.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "run_start").await;
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n"
        );
    }

    /// Pins the fix for the real race: `FeedHandle::drop`'s `task.abort()`
    /// is only a *request* — a task caught mid-write finishes that write
    /// before the cancellation can actually drop its future. A naive
    /// `subscribe` that replaces a feed the instant `Weak::upgrade` fails
    /// could truncate the cache and start a fresh append while the old
    /// task's tail-end write is still in flight, landing after the new
    /// feed's own replay and duplicating/corrupting the cache (spec §5.1:
    /// "a viewer that reconnects never gets duplicate lines"). `subscribe`
    /// now waits for the old task's `FeedDone` guard to confirm it has
    /// truly stopped before touching the file.
    #[tokio::test]
    async fn replacing_the_just_dropped_last_guard_waits_for_it_to_finish_first() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01RACE.jsonl");
        let ex = exec(&[r#"{"type":"run_start"}"#], true);
        let reg = LazyTailRegistry::new(ex.clone());

        let first = reg
            .subscribe("/r/run_01RACE.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "run_start").await;
        // `first.alive` is a private field, visible here because `tests` is
        // a child module of `lazy_tail` — clone the flag out before
        // dropping the only strong `Arc<FeedHandle>`, so it can still be
        // inspected once `first` itself is gone.
        let first_alive = Arc::clone(&first.alive);
        drop(first);

        let second = reg
            .subscribe("/r/run_01RACE.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "run_start").await;

        assert_eq!(
            ex.spawns.lock().unwrap().len(),
            2,
            "a fresh tail was spawned"
        );
        assert!(
            !first_alive.load(Ordering::SeqCst),
            "the replaced feed's task must be confirmed stopped by the time \
             the second subscribe returns"
        );
        assert!(second.alive());
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n",
            "replay from an empty file: exactly one copy of the line, no \
             duplicate from the replaced feed's tail end"
        );
    }
}
