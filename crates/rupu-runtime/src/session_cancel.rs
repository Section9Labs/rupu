//! Run-addressed cooperative cancellation. The requester never rewrites session.json
//! or signals a process; only the owning turn consumes its unique request file.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
struct Owner {
    pid: u32,
    start: Option<u64>,
}
fn path(dir: &Path, run: &str, suffix: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        !run.is_empty()
            && run.len() <= 256
            && run
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
        "invalid run id"
    );
    Ok(dir.join(format!("cancel-{run}.{suffix}")))
}
/// A turn advertises support only while its cooperative cancellation monitor lives.
/// Dropping this guard aborts that monitor and removes only this turn's files.
pub struct TurnCancellation {
    pub token: CancellationToken,
    monitor: tokio::task::JoinHandle<()>,
    owner: PathBuf,
    request: PathBuf,
}
impl TurnCancellation {
    pub fn start(dir: &Path, run: &str, pid: u32, start: Option<u64>) -> anyhow::Result<Self> {
        let owner = path(dir, run, "owner.json")?;
        let request = path(dir, run, "request")?;
        let token = CancellationToken::new();
        fs::write(&owner, serde_json::to_vec(&Owner { pid, start })?)?;
        let signal = token.clone();
        let pending = request.clone();
        let monitor = tokio::spawn(async move {
            loop {
                if pending.exists() {
                    signal.cancel();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        Ok(Self {
            token,
            monitor,
            owner,
            request,
        })
    }
}
impl Drop for TurnCancellation {
    fn drop(&mut self) {
        self.monitor.abort();
        let _ = fs::remove_file(&self.owner);
        let _ = fs::remove_file(&self.request);
    }
}
pub fn available(
    dir: &Path,
    run: &str,
    active_pid: Option<u32>,
    alive: impl Fn(u32, Option<u64>) -> bool,
) -> bool {
    let owner = path(dir, run, "owner.json")
        .ok()
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Owner>(&b).ok());
    owner.is_some_and(|o| Some(o.pid) == active_pid && alive(o.pid, o.start))
}
/// Recheck identity immediately before creating the run-specific signal. Even if
/// the turn completes concurrently, this signal cannot cancel a subsequent run.
pub fn request(
    dir: &Path,
    expected_run: &str,
    alive: impl Fn(u32, Option<u64>) -> bool,
) -> anyhow::Result<()> {
    let record: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("session.json"))?)?;
    anyhow::ensure!(
        record["active_run_id"].as_str() == Some(expected_run),
        "active run changed; reload before cancelling"
    );
    let pid = record["active_pid"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok());
    anyhow::ensure!(
        available(dir, expected_run, pid, alive),
        "active worker does not support cooperative cancellation; restart it after this turn"
    );
    fs::write(path(dir, expected_run, "request")?, b"cancel")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_is_run_scoped_and_does_not_rewrite_session() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let record = br#"{"active_run_id":"run_one","active_pid":42,"worker_pid":42,"message_history":["retained"]}"#;
        fs::write(dir.join("session.json"), record).unwrap();
        let first = TurnCancellation::start(dir, "run_one", 42, Some(5)).unwrap();
        assert!(!available(dir, "run_one", Some(42), |_, _| false));
        assert!(request(dir, "run_stale", |_, _| true).is_err());
        assert!(request(dir, "../escape", |_, _| true).is_err());
        request(dir, "run_one", |p, s| p == 42 && s == Some(5)).unwrap();
        tokio::time::timeout(Duration::from_secs(1), first.token.cancelled())
            .await
            .unwrap();
        let next = TurnCancellation::start(dir, "run_two", 42, Some(5)).unwrap();
        assert!(!next.token.is_cancelled());
        assert_eq!(fs::read(dir.join("session.json")).unwrap(), record);
        drop(first);
        assert!(!available(dir, "run_one", Some(42), |_, _| true));
        assert!(available(dir, "run_two", Some(42), |_, _| true));
    }
}
