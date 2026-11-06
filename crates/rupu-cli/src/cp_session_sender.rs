//! `SubprocessSessionSender` — the `cp serve` adapter for rupu-cp's
//! [`SessionSender`] port. It shells `rupu session send --detach [--skill
//! <s>]… -- <id> ["<prompt>"]`, which resolves the skills, enqueues the turn +
//! ensures the session worker, prints `run: <run_id>` to stdout, and exits
//! promptly (the turn runs async in a separate worker). We parse that
//! `run: …` line and return the run id so the web UI can navigate to the new
//! run immediately.

use rupu_cp::session_sender::{SendError, SendMessageRequest, SessionSender};
use rupu_runtime::argv::{RunArgv, SessionSend};
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

/// The `rupu session send --detach` for `req`.
fn session_send_argv(req: &SendMessageRequest) -> RunArgv {
    RunArgv::SessionSend(SessionSend {
        session_id: req.session_id.clone(),
        prompt: req.prompt.clone(),
        detach: true,
        skills: req.skills.clone(),
    })
}

/// A failed `rupu` child's message for an API error body: its stderr,
/// trimmed, without the `[error] ` label `output::diag::fail` gives it, or
/// `fallback` when it printed nothing.
pub(crate) fn cli_failure(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    let text = text.strip_prefix("[error] ").unwrap_or(text).trim_start();
    if text.is_empty() {
        fallback.to_string()
    } else {
        text.to_string()
    }
}

#[async_trait::async_trait]
impl SessionSender for SubprocessSessionSender {
    async fn send(&self, req: SendMessageRequest) -> Result<String, SendError> {
        if req.prompt.as_deref().is_none_or(|p| p.trim().is_empty()) && req.skills.is_empty() {
            return Err(SendError::Invalid(
                "a send needs a prompt or a skill".into(),
            ));
        }

        let out = tokio::process::Command::new(&self.exe)
            .args(session_send_argv(&req).to_args())
            .output()
            .await
            .map_err(|e| SendError::Spawn(e.to_string()))?;

        if !out.status.success() {
            let msg = cli_failure(&out.stderr, "session send failed");
            // Exit 1 is the command refusing the request (an unknown or
            // ambiguous skill, a stopped session): the caller's to fix.
            return Err(if out.status.code() == Some(1) {
                SendError::Invalid(msg)
            } else {
                SendError::Spawn(msg)
            });
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
pub(crate) mod tests {
    use super::*;
    use std::path::Path;

    /// A stand-in `rupu` in `dir` that prints `stderr` and exits 1, the way
    /// the real binary fails a command (`output::diag::fail`, no colour
    /// when stderr is not a terminal).
    /// Through `exec_ready_script`, so the spawn under test can't fail with
    /// ETXTBSY (another test thread forking while the script's write
    /// descriptor is open — seen on the musl CI as "Text file busy").
    pub(crate) fn failing_rupu(dir: &Path, stderr: &str) -> PathBuf {
        std::fs::write(dir.join("stderr.txt"), stderr).unwrap();
        crate::test_support::exec_ready_script(
            &dir.join("rupu"),
            "cat \"$(dirname \"$0\")/stderr.txt\" >&2\nexit 1\n",
        )
    }

    fn send_req(prompt: Option<&str>, skills: &[&str]) -> SendMessageRequest {
        SendMessageRequest {
            session_id: "ses_1".into(),
            prompt: prompt.map(Into::into),
            skills: skills.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn argv(req: &SendMessageRequest) -> Vec<String> {
        session_send_argv(req)
            .to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_skills_ride_the_argv_before_the_separator() {
        assert_eq!(
            argv(&send_req(Some("hi"), &["pdf"])),
            ["session", "send", "--detach", "--skill", "pdf", "--", "ses_1", "hi"]
        );
    }

    #[test]
    fn a_skill_only_send_renders_no_prompt() {
        assert_eq!(
            argv(&send_req(None, &["pdf"])),
            ["session", "send", "--detach", "--skill", "pdf", "--", "ses_1"]
        );
    }

    /// `session send --skill` resolves the operator's entries itself and
    /// exits 1 on a typo: that is the request's fault, so it comes back as
    /// `Invalid` (a 400) carrying the CLI's own message, without its
    /// `[error] ` label.
    #[tokio::test]
    async fn a_refused_skill_is_invalid_with_the_clis_message() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let skill = global.join("skills").join("pdf");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: pdf\ndescription: The pdf skill.\n---\nRead PDFs.\n",
        )
        .unwrap();
        let refusal = rupu_runtime::assembly::resolve_operator_skills(
            &rupu_skills::LayerDirs::for_layers(&global, None),
            &[],
            &["pdff".to_string()],
        )
        .await
        .expect_err("a typo is refused");
        let message = format!("[error] {refusal}");
        assert!(
            message.contains("unknown skill \"pdff\" (did you mean pdf?)"),
            "{message}"
        );

        let sender = SubprocessSessionSender {
            exe: failing_rupu(tmp.path(), &format!("{message}  \n")),
        };
        match sender.send(send_req(None, &["pdff"])).await {
            Err(SendError::Invalid(m)) => assert_eq!(m, refusal.to_string()),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

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
