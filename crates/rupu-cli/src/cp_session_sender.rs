//! `SubprocessSessionSender` — the `cp serve` adapter for rupu-cp's
//! [`SessionSender`] port. It shells `rupu session send <id> "<prompt>"
//! --detach`, which enqueues the turn + ensures the session worker, prints
//! `run: <run_id>` to stdout, and exits promptly (the turn runs async in a
//! separate worker). We parse that `run: …` line and return the run id so the
//! web UI can navigate to the new run immediately.

use rupu_cp::session_sender::{SendError, SendMessageRequest, SessionSender};
use std::path::PathBuf;

/// Spawns `rupu session send …` children. `exe` is the path to the running
/// `rupu` binary (resolved via `std::env::current_exe()` in `cp serve`).
pub struct SubprocessSessionSender {
    pub exe: PathBuf,
}

/// Scan `session send --detach` stdout for the `run: <id>` line and return the
/// run id. Matches the CLI's printout (`println!("run: {run_id}")`).
fn parse_run_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("run:") {
            let id = rest.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[async_trait::async_trait]
impl SessionSender for SubprocessSessionSender {
    async fn send(&self, req: SendMessageRequest) -> Result<String, SendError> {
        if req.prompt.trim().is_empty() {
            return Err(SendError::Invalid("prompt is empty".into()));
        }

        let out = tokio::process::Command::new(&self.exe)
            .args(["session", "send", &req.session_id, &req.prompt, "--detach"])
            .output()
            .await
            .map_err(|e| SendError::Spawn(e.to_string()))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                "session send failed".to_string()
            } else {
                stderr
            };
            return Err(SendError::Spawn(msg));
        }

        let stdout = String::from_utf8_lossy(&out.stdout);
        match parse_run_id(&stdout) {
            Some(id) => Ok(id),
            None => Err(SendError::Spawn(
                "could not determine run id from session send output".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_run_id_finds_run_line() {
        let stdout = "session: sess_01XYZ\nrun: run_01ABCDEF\nattach: rupu session attach sess_01XYZ\n";
        assert_eq!(parse_run_id(stdout), Some("run_01ABCDEF".to_string()));
    }

    #[test]
    fn parse_run_id_none_when_absent() {
        let stdout = "session: sess_01XYZ\nattach: rupu session attach sess_01XYZ\n";
        assert_eq!(parse_run_id(stdout), None);
    }
}
