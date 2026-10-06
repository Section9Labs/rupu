//! Spec §5.1: one `tail -n +1 -F` over ssh per distinct cache file, shared by
//! every viewer of that file, killed when the last viewer leaves, never
//! opened for a file that already has a `.complete` sidecar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures_util::StreamExt as _;
use tokio::sync::{watch, Mutex};

use super::connector::blocking_host;
use super::ssh::{parse_tail_marker, shell_escape, RemoteExec, RemoteExecError};
use super::transcript_paths::is_complete;

/// How long `subscribe` will wait for a just-replaced feed's task to
/// confirm it has actually stopped writing before truncating the cache out
/// from under it. A cooperative `abort()` only takes effect at the aborted
/// task's next await point, so this is a real (if generous) wait, not a
/// formality — see [`FeedDone`]. A timeout just proceeds; it trades a
/// vanishingly rare stuck-task edge case for never hanging a viewer.
const REPLACE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// The most tailed lines one write hop takes: every line already waiting
/// when the feed wakes, up to this many, goes to disk in a single hop to the
/// blocking pool — a byte-zero replay is thousands of lines at once.
const FEED_WRITE_BATCH: usize = 512;

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
        // Before the abort, like `retire`: a write hop the task already
        // handed to the blocking pool, but that has not started, then writes
        // nothing (see `FeedFile::append`).
        self.alive.store(false, Ordering::SeqCst);
        self.task.abort();
    }
}

/// Shared, behind an `Arc`, by the feeding task for its entire body and by
/// each write hop the task hands to the blocking pool, for that hop's whole
/// run. `JoinHandle::abort()` drops the task's future at its next `.await`,
/// which can be the await on a write hop that is already on the blocking
/// pool — and that hop is not cancelled: it runs to completion. Its clone of
/// this guard is what holds `finished` back until it has. `drop` runs only
/// once the last holder is gone, so it is the one moment that is guaranteed
/// to run *after* any write the feed started, on every exit path (normal
/// stream end, panic, or abort alike).
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
    /// A tokio `Mutex`, because `subscribe` holds it across a hop to the
    /// blocking pool: its re-check of the cache's `.complete` mark and the
    /// feed it then installs must sit in one critical section (see there).
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
    ///
    /// Its own disk work (the `.complete` checks, the cache directory) and
    /// the feed's writes run on tokio's blocking pool.
    pub(crate) async fn subscribe(
        &self,
        remote: &str,
        cache: &Path,
    ) -> Result<Option<Arc<FeedHandle>>, RemoteExecError> {
        let path = cache.to_path_buf();
        if off_runtime(move || Ok(is_complete(&path))).await? {
            return Ok(None);
        }

        // Fast path: an existing, live feed is shared without waiting.
        let wait_rx = {
            let feeds = self.feeds.lock().await;
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
        // why this is a real race, not a formality. Not under the lock: the
        // wait can last `REPLACE_WAIT_TIMEOUT`, and every other cache's
        // subscribe and retire would queue behind it.
        if let Some(mut rx) = wait_rx {
            let _ = tokio::time::timeout(REPLACE_WAIT_TIMEOUT, rx.wait_for(|done| *done)).await;
        }

        let mut feeds = self.feeds.lock().await;
        // Dead entries are never otherwise removed, so the map would grow for
        // the process's lifetime. Prune on the slow path only — the fast path
        // above must stay lock-and-return.
        feeds.retain(|_, e| e.handle.strong_count() > 0);
        // Re-check completeness UNDER the lock. The check at the top of this
        // function ran before the (awaited) wait for the old feed to stop, and
        // a terminal pull can write the `.complete` sidecar in that window. A
        // complete cache is authoritative: never tail it. The lock is held
        // across this hop, so no `retire` can slip between the check and the
        // feed installed below. The same hop creates the cache's directory.
        let path = cache.to_path_buf();
        if off_runtime(move || prepare_cache(&path)).await? {
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

        let cmd = format!("tail -n +1 -F {}", shell_escape(remote));
        let stream = self.exec.spawn_lines(&cmd)?;
        let alive = Arc::new(AtomicBool::new(true));
        let (finished_tx, finished_rx) = watch::channel(false);
        let done = FeedDone {
            alive: Arc::clone(&alive),
            finished: finished_tx,
        };
        let mut out = FeedFile {
            cache: cache.to_path_buf(),
            file: None,
        };
        let alive_task = Arc::clone(&alive);
        let task = tokio::spawn(async move {
            // Held for the whole body, and by each write hop for its own
            // run: the last `Drop` is what tells a replacing `subscribe`
            // this feed will never write again.
            let done = Arc::new(done);
            // One writer, in order: each hop is awaited before the next
            // batch is read, and an aborted task starts no further hop.
            let mut batches = stream.ready_chunks(FEED_WRITE_BATCH);
            while let Some(batch) = batches.next().await {
                let mut lines = Vec::with_capacity(batch.len());
                let mut ended = false;
                for item in batch {
                    match item {
                        Ok(line)
                            if parse_tail_marker(&line).is_some() || line.trim().is_empty() => {}
                        Ok(line) => lines.push(line),
                        Err(_) => {
                            ended = true;
                            break;
                        }
                    }
                }
                if !lines.is_empty() {
                    let (done, alive) = (Arc::clone(&done), Arc::clone(&alive_task));
                    let hop = blocking_host(move || {
                        let _done = done;
                        let wrote = out.append(&lines, &alive);
                        Ok((out, wrote))
                    });
                    match hop.await {
                        Ok((back, true)) => out = back,
                        _ => return,
                    }
                }
                if ended {
                    return;
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
        let entry = self.feeds.lock().await.remove(cache);
        let Some(entry) = entry else { return };
        // Before the abort lands, so `has_live_feed` and `subscribe`'s
        // liveness checks stop reporting this feed immediately.
        entry.alive.store(false, Ordering::SeqCst);
        entry.abort.abort();
        // A write hop the task already handed to the blocking pool is not
        // cancelled by the abort; it finishes first. Wait for its `FeedDone`
        // guard, exactly as the replace path does, so the caller really is
        // the only writer when this returns.
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
    pub(crate) async fn has_live_feed(&self, cache: &Path) -> bool {
        let feeds = self.feeds.lock().await;
        feeds
            .get(cache)
            .and_then(|e| e.handle.upgrade())
            .is_some_and(|h| h.alive())
    }

    #[cfg(test)]
    pub(crate) async fn live_feeds(&self) -> usize {
        self.feeds
            .lock()
            .await
            .values()
            .filter(|e| e.handle.strong_count() > 0)
            .count()
    }
}

/// Run `subscribe`'s disk work `f` on tokio's blocking pool
/// ([`blocking_host`]). Either failure — `f`'s own, or a panic in it — means
/// the feed could not be started: [`RemoteExecError::Spawn`].
async fn off_runtime<T: Send + 'static>(
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, RemoteExecError> {
    let spawn_err = |e: &dyn std::fmt::Display| RemoteExecError::Spawn(e.to_string());
    blocking_host(move || Ok(f()))
        .await
        .map_err(|e| spawn_err(&e))?
        .map_err(|e| spawn_err(&e))
}

/// `subscribe`'s locked re-check: `true` when `cache` is complete, else
/// make sure its directory exists for the feed about to fill it.
fn prepare_cache(cache: &Path) -> std::io::Result<bool> {
    if is_complete(cache) {
        return Ok(true);
    }
    if let Some(dir) = cache.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(false)
}

/// The cache file a feed fills, carried by its task from one write hop to
/// the next.
struct FeedFile {
    cache: PathBuf,
    /// Opened lazily on the first real line — see `subscribe`'s doc comment:
    /// until the remote has proven it can deliver, the already-collected
    /// partial content stays on disk untouched.
    file: Option<std::fs::File>,
}

impl FeedFile {
    /// Append `lines` to the cache, in order: one write hop's blocking body.
    /// `false` means the feed stops: it was told to before this hop ran
    /// (`alive` cleared by `retire`, or by the last viewer leaving), the
    /// cache turned complete before its first line, or the disk failed.
    fn append(&mut self, lines: &[String], alive: &AtomicBool) -> bool {
        use std::io::Write as _;
        if !alive.load(Ordering::SeqCst) {
            return false;
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => {
                // The cache became complete (terminal pull finished and
                // wrote the sidecar) while we waited for the first remote
                // line. The pull retired us or raced our spawn — never
                // truncate a complete file.
                if is_complete(&self.cache) {
                    return false;
                }
                // Truncate only now, on the first real line, so a feed that
                // never yields (host unreachable) leaves previously collected
                // content intact; the replay that follows starts from byte zero.
                if std::fs::File::create(&self.cache).is_err() {
                    return false;
                }
                let Ok(opened) = std::fs::OpenOptions::new().append(true).open(&self.cache) else {
                    return false;
                };
                self.file.insert(opened)
            }
        };
        let mut batch = String::new();
        for line in lines {
            batch.push_str(line);
            batch.push('\n');
        }
        file.write_all(batch.as_bytes())
            .and_then(|_| file.flush())
            .is_ok()
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
        assert_eq!(reg.live_feeds().await, 1);

        reg.retire(&cache).await;
        assert!(!guard.alive(), "the still-held handle reports dead");
        assert!(!reg.has_live_feed(&cache).await);
        assert_eq!(reg.live_feeds().await, 0, "the entry is gone");

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

        assert!(
            !reg.has_live_feed(&cache).await,
            "no feed has been opened yet"
        );
        let guard = reg
            .subscribe("/r/run_01LIVE.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        wait_for_content(&cache, "run_start").await;
        assert!(
            reg.has_live_feed(&cache).await,
            "a held guard is a live feed"
        );
        drop(guard);
        assert!(
            !reg.has_live_feed(&cache).await,
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
        assert!(!reg.has_live_feed(&done).await);
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
        assert_eq!(reg.live_feeds().await, 1);
        wait_for_content(&cache, "run_start").await;

        drop(a);
        assert_eq!(reg.live_feeds().await, 1, "one holder left");
        drop(b);
        assert_eq!(
            reg.live_feeds().await,
            0,
            "last holder gone → feed released"
        );
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

    /// The feed's cache writes (the first line's truncate + append open, then
    /// every append) run on the blocking pool: parked on a FIFO cache, the
    /// runtime keeps ticking. The stream ends after its one line, so the
    /// feed's `FeedDone` fires once that line is written.
    #[tokio::test(flavor = "current_thread")]
    async fn the_feed_writes_the_cache_off_the_runtime() {
        use crate::host::runtime_liveness::{assert_runtime_live_while_parked_on, make_fifo};
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01FIFO.jsonl");
        if !make_fifo(&cache) {
            eprintln!("mkfifo unavailable; skipping");
            return;
        }
        let ex = exec(&[r#"{"type":"run_start"}"#], false);
        let reg = LazyTailRegistry::new(ex.clone());

        let _guard = reg
            .subscribe("/r/run_01FIFO.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        let mut finished = reg.feeds.lock().await[&cache].finished.clone();
        let feed_done = async move {
            let _ = finished.wait_for(|done| *done).await;
        };
        assert_runtime_live_while_parked_on(feed_done, &cache, true).await;
    }

    /// `retire` waits for a write the feed already handed to the blocking
    /// pool: the abort does not cancel that hop, and the hop's clone of
    /// `FeedDone` holds `finished` back until its write is done. Here the
    /// hop is parked mid-`write_all` on a full FIFO when `retire` is called.
    /// Multi-threaded, so the drain below still runs if a regression ever
    /// puts that write back on a runtime thread.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retire_waits_for_a_write_hop_already_on_the_blocking_pool() {
        use crate::host::runtime_liveness::make_fifo;
        use rustix::fs::{Mode, OFlags};
        use rustix::io::Errno;
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01INFLIGHT.jsonl");
        if !make_fifo(&cache) {
            eprintln!("mkfifo unavailable; skipping");
            return;
        }
        // Held open, so the feed's opens return at once; read only when
        // asked, so a line larger than the pipe's buffer parks its write.
        let reader =
            rustix::fs::open(&cache, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty()).unwrap();
        let line = format!(r#"{{"type":"run_start","pad":"{}"}}"#, "x".repeat(1 << 20));
        let ex = exec(&[line.as_str()], true);
        let reg = LazyTailRegistry::new(ex.clone());
        let guard = reg
            .subscribe("/r/run_01INFLIGHT.jsonl", &cache)
            .await
            .unwrap()
            .unwrap();
        let finished = reg.feeds.lock().await[&cache].finished.clone();

        // The hop has started writing once the pipe holds a byte. Until its
        // open, a read is EOF (no writer); after it, EAGAIN (nothing yet).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match rustix::io::read(&reader, &mut [0u8; 1]) {
                Ok(1) => break,
                Ok(_) | Err(Errno::AGAIN) => {}
                Err(e) => panic!("reading the FIFO: {e}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the feed never started writing"
            );
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        let retire = reg.retire(&cache);
        tokio::pin!(retire);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), &mut retire)
                .await
                .is_err(),
            "retire returned while the feed's write hop was still writing"
        );
        assert!(!*finished.borrow());

        // Drain the pipe: the write completes, the hop ends (closing the
        // file, so the drain sees EOF) and drops its `FeedDone`.
        let drain = std::thread::spawn(move || {
            let (mut buf, mut total) = (vec![0u8; 64 * 1024], 1);
            loop {
                match rustix::io::read(&reader, &mut buf[..]) {
                    Ok(0) => return total,
                    Ok(n) => total += n,
                    Err(Errno::AGAIN) => std::thread::sleep(std::time::Duration::from_millis(1)),
                    Err(e) => panic!("draining the FIFO: {e}"),
                }
            }
        });
        retire.await;
        assert!(
            *finished.borrow(),
            "retire returned before the feed's FeedDone fired"
        );
        assert!(!guard.alive());
        assert_eq!(
            drain.join().unwrap(),
            line.len() + 1,
            "the in-flight write finished whole"
        );
    }

    /// `subscribe`'s first `.complete` check (a `stat`, which no FIFO can
    /// park) waits for the blocking pool.
    #[test]
    fn subscribe_checks_completeness_off_the_runtime() {
        use crate::host::runtime_liveness::{
            assert_waits_for_the_blocking_pool, one_blocking_thread_runtime, HeldBlockingThread,
        };
        one_blocking_thread_runtime().block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let cache = tmp.path().join("run_01CHECK.jsonl");
            std::fs::write(&cache, "{\"type\":\"run_start\"}\n").unwrap();
            std::fs::write(crate::host::transcript_paths::complete_marker(&cache), b"").unwrap();
            let ex = exec(&[], true);
            let reg = LazyTailRegistry::new(ex.clone());

            let held = HeldBlockingThread::hold();
            let sub = reg.subscribe("/r/run_01CHECK.jsonl", &cache);
            let got = assert_waits_for_the_blocking_pool(sub, held).await;

            assert!(got.unwrap().is_none(), "a complete cache is not tailed");
            assert!(ex.spawns.lock().unwrap().is_empty());
        });
    }

    /// `subscribe`'s locked re-check and the cache directory it makes (a
    /// `stat` and a `mkdir`) wait for the blocking pool, under the lock.
    #[test]
    fn subscribe_rechecks_and_makes_the_cache_dir_off_the_runtime() {
        use crate::host::runtime_liveness::{
            assert_waits_for_the_blocking_pool, one_blocking_thread_runtime, HeldBlockingThread,
        };
        one_blocking_thread_runtime().block_on(async {
            let tmp = tempfile::tempdir().unwrap();
            let cache = tmp.path().join("mirror/h/transcripts/run_01DIR.jsonl");
            let ex = exec(&[], true);
            let reg = LazyTailRegistry::new(ex.clone());

            // Taking the lock first parks `subscribe` on it once its first
            // hop is done (the blocking thread is still free for that one);
            // the thread is held before the lock is let go, so the hop after
            // the lock queues behind it.
            let lock = reg.feeds.lock().await;
            let sub = reg.subscribe("/r/run_01DIR.jsonl", &cache);
            tokio::pin!(sub);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(50), &mut sub)
                    .await
                    .is_err(),
                "subscribe waits for the registry lock"
            );
            let held = HeldBlockingThread::hold();
            drop(lock);
            let got = assert_waits_for_the_blocking_pool(sub, held).await;

            assert!(got.unwrap().is_some(), "a feed was started");
            assert!(cache.parent().unwrap().is_dir());
            assert_eq!(ex.spawns.lock().unwrap().len(), 1);
        });
    }

    /// A write hop that has not started when its feed is told to stop —
    /// retired, or its last viewer gone — writes nothing: the stop at the
    /// next await point that an abort gave the inline writes. The hop waits
    /// here behind the runtime's only blocking thread, held until the feed
    /// has been stopped.
    #[test]
    fn a_write_hop_that_starts_after_its_feed_stopped_writes_nothing() {
        use crate::host::runtime_liveness::{one_blocking_thread_runtime, HeldBlockingThread};
        for retired in [true, false] {
            one_blocking_thread_runtime().block_on(async {
                let tmp = tempfile::tempdir().unwrap();
                let cache = tmp.path().join("run_01QUEUED.jsonl");
                std::fs::write(&cache, "COLLECTED EARLIER\n").unwrap();
                let ex = exec(&[r#"{"type":"run_start"}"#], true);
                let reg = LazyTailRegistry::new(ex.clone());
                let guard = reg
                    .subscribe("/r/run_01QUEUED.jsonl", &cache)
                    .await
                    .unwrap()
                    .unwrap();
                let mut finished = reg.feeds.lock().await[&cache].finished.clone();

                // Held before the feed task first runs, so its write hop
                // queues behind it.
                let held = HeldBlockingThread::hold();
                // Single-threaded: the feed task runs (and queues its hop)
                // before this sleep's timer is serviced.
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                assert!(!*finished.borrow(), "the feed's hop is still queued");

                let retire = reg.retire(&cache);
                tokio::pin!(retire);
                if retired {
                    // One poll: clears `alive` and aborts, then waits.
                    let _ = tokio::time::timeout(std::time::Duration::ZERO, &mut retire).await;
                } else {
                    drop(guard);
                }
                held.release().await;
                if retired {
                    retire.await;
                }
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    finished.wait_for(|done| *done),
                )
                .await
                .expect("the stopped feed's FeedDone fired")
                .unwrap();
                assert_eq!(
                    std::fs::read_to_string(&cache).unwrap(),
                    "COLLECTED EARLIER\n",
                    "retired: {retired} — the queued hop neither truncated nor wrote"
                );
            });
        }
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
