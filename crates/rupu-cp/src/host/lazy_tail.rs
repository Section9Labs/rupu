//! Spec §5.1: one `tail -n +1 -F` over ssh per distinct cache file, shared by
//! every viewer of that file, killed when the last viewer leaves, never
//! opened for a file that already has a `.complete` sidecar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures_util::StreamExt as _;

use super::ssh::{parse_tail_marker, shell_escape, RemoteExec, RemoteExecError};
use super::transcript_paths::is_complete;

/// Refcount + liveness for one shared remote tail. Every subscriber holds an
/// `Arc`; the feeding task is aborted on drop of the last one, which drops
/// the ssh child through `kill_on_drop`.
pub(crate) struct FeedHandle {
    task: tokio::task::JoinHandle<()>,
    alive: Arc<AtomicBool>,
}

impl FeedHandle {
    /// `false` once the remote stream ended (ssh dropped, remote `tail`
    /// died). The cache file is left as-is; a later subscribe replaces the
    /// feed and replays from byte zero.
    pub(crate) fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for FeedHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) struct LazyTailRegistry {
    exec: Arc<dyn RemoteExec>,
    feeds: Mutex<HashMap<PathBuf, Weak<FeedHandle>>>,
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
    ///   feed is replaced.
    pub(crate) fn subscribe(
        &self,
        remote: &str,
        cache: &Path,
    ) -> Result<Option<Arc<FeedHandle>>, RemoteExecError> {
        if is_complete(cache) {
            return Ok(None);
        }
        let mut feeds = self.feeds.lock().unwrap();
        if let Some(existing) = feeds.get(cache).and_then(Weak::upgrade) {
            if existing.alive() {
                return Ok(Some(existing));
            }
        }
        if let Some(dir) = cache.parent() {
            std::fs::create_dir_all(dir).map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        }
        // Truncate BEFORE spawning: the replay starts from an empty file.
        std::fs::File::create(cache).map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        let cmd = format!("tail -n +1 -F {}", shell_escape(remote));
        let mut stream = self.exec.spawn_lines(&cmd)?;
        let alive = Arc::new(AtomicBool::new(true));
        let alive_for_task = Arc::clone(&alive);
        let cache_owned = cache.to_path_buf();
        let task = tokio::spawn(async move {
            use std::io::Write as _;
            let file = std::fs::OpenOptions::new().append(true).open(&cache_owned);
            let Ok(mut file) = file else {
                alive_for_task.store(false, Ordering::SeqCst);
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
            alive_for_task.store(false, Ordering::SeqCst);
        });
        let handle = Arc::new(FeedHandle { task, alive });
        feeds.insert(cache.to_path_buf(), Arc::downgrade(&handle));
        Ok(Some(handle))
    }

    #[cfg(test)]
    pub(crate) fn live_feeds(&self) -> usize {
        self.feeds
            .lock()
            .unwrap()
            .values()
            .filter(|w| w.strong_count() > 0)
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

        let a = reg.subscribe("/r/run_01B.jsonl", &cache).unwrap().unwrap();
        let b = reg.subscribe("/r/run_01B.jsonl", &cache).unwrap().unwrap();
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

        assert!(reg.subscribe("/r/run_01C.jsonl", &cache).unwrap().is_none());
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

        let first = reg.subscribe("/r/run_01D.jsonl", &cache).unwrap().unwrap();
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
        let second = reg.subscribe("/r/run_01D.jsonl", &cache).unwrap().unwrap();
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
        let _g = reg.subscribe("/r/run_01E.jsonl", &cache).unwrap().unwrap();
        wait_for_content(&cache, "run_start").await;
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n"
        );
    }
}
