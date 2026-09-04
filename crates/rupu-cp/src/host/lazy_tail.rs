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
/// feed alive, plus a `finished` receiver kept independently of the
/// `Weak` — once the last `Arc<FeedHandle>` is dropped the `Weak` can no
/// longer be upgraded, so this is the only way `subscribe` can still learn
/// when that feed's task has actually stopped.
struct FeedEntry {
    handle: Weak<FeedHandle>,
    finished: watch::Receiver<bool>,
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
    ///   The first subscriber truncates the cache and spawns
    ///   `tail -n +1 -F` (a byte-zero replay, which is what makes the
    ///   truncate safe); later subscribers share the running feed; a dead
    ///   feed is replaced — after confirming, via [`FeedDone`], that the
    ///   old feed's task has actually stopped writing.
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
        // Truncate BEFORE spawning: the replay starts from an empty file.
        // Safe now — any previous feed's task is confirmed fully stopped.
        std::fs::File::create(cache).map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        let cmd = format!("tail -n +1 -F {}", shell_escape(remote));
        let mut stream = self.exec.spawn_lines(&cmd)?;
        let alive = Arc::new(AtomicBool::new(true));
        let (finished_tx, finished_rx) = watch::channel(false);
        let done = FeedDone {
            alive: Arc::clone(&alive),
            finished: finished_tx,
        };
        let cache_owned = cache.to_path_buf();
        let task = tokio::spawn(async move {
            // Held for the whole body: its `Drop` is what tells a
            // replacing `subscribe` this task will never write again.
            let _done = done;
            use std::io::Write as _;
            let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&cache_owned) else {
                return;
            };
            while let Some(Ok(line)) = stream.next().await {
                if parse_tail_marker(&line).is_some() || line.trim().is_empty() {
                    continue;
                }
                if writeln!(file, "{line}").and_then(|_| file.flush()).is_err() {
                    break;
                }
            }
        });
        let handle = Arc::new(FeedHandle { task, alive });
        feeds.insert(
            cache.to_path_buf(),
            FeedEntry {
                handle: Arc::downgrade(&handle),
                finished: finished_rx,
            },
        );
        Ok(Some(handle))
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
