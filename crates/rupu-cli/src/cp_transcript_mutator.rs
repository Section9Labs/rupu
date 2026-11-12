//! `cp serve` adapter for rupu-cp's `TranscriptMutator` port. Shells
//! `rupu transcript archive|delete <run_id>` using this same binary.
//!
//! This is the ONLY path rupu-cp may use to mutate a standalone-agent-run
//! transcript: shelling the real CLI command means the `ensure_standalone_transcript`
//! guard in `crates/rupu-cli/src/cmd/transcript.rs` runs for real and refuses
//! to touch a transcript that is actually owned by a session. Reimplementing
//! the file move/delete directly in rupu-cp would silently bypass that guard.

use std::path::PathBuf;

use rupu_cp::transcript_mutator::{TranscriptAction, TranscriptMutateError, TranscriptMutator};

/// Shells `rupu transcript archive|delete <run_id>` children. `exe` is the
/// path to the running `rupu` binary (resolved via `std::env::current_exe()`
/// in `cp serve`).
pub struct SubprocessTranscriptMutator {
    pub exe: PathBuf,
}

/// Build the argv (after the executable) for a transcript mutation.
///
/// For `Delete` we always pass `--force` because the CP UI presents its own
/// confirmation step before invoking the endpoint. `ignore_liveness` maps
/// straight to the CLI's own `--ignore-liveness` flag — the PID-reuse escape
/// hatch — and defaults to `false` (unchanged behavior) unless the caller
/// opted in.
pub(crate) fn build_argv(id: &str, action: TranscriptAction, ignore_liveness: bool) -> Vec<String> {
    let mut argv = vec![
        "transcript".to_string(),
        action.as_str().to_string(),
        id.to_string(),
    ];
    if action == TranscriptAction::Delete {
        argv.push("--force".to_string());
    }
    if ignore_liveness {
        argv.push("--ignore-liveness".to_string());
    }
    argv
}

/// Map a failed child's stderr to the right error variant.
///
/// Real CLI messages (from `crates/rupu-cli/src/cmd/transcript.rs`):
/// - `"transcript {id} is managed by session {id}; use 'rupu session
///   archive|delete' instead"` → [`TranscriptMutateError::Invalid`] (this is
///   the safety-critical case: a session-owned transcript must be refused,
///   never silently clobbered)
/// - `"cannot {action} transcript {id}: it is still running (owning process
///   {pid} is alive)"` → [`TranscriptMutateError::Invalid`] (I4: refuse an
///   in-flight standalone run, not just a session-owned one)
/// - `"transcript not found: {id}"` → [`TranscriptMutateError::NotFound`]
/// - `"transcript already archived: {id}"` → [`TranscriptMutateError::Invalid`]
/// - `"transcript delete requires --force"` → [`TranscriptMutateError::Invalid`]
/// - anything else → [`TranscriptMutateError::Failed`]
pub(crate) fn classify_failure(action: TranscriptAction, stderr: &str) -> TranscriptMutateError {
    let s = stderr.to_ascii_lowercase();
    if s.contains("is managed by session") {
        TranscriptMutateError::Invalid(stderr.trim().to_string())
    } else if s.contains("not found") {
        TranscriptMutateError::NotFound(stderr.trim().to_string())
    } else if s.contains("already archived")
        || s.contains("requires --force")
        || s.contains("still running")
    {
        TranscriptMutateError::Invalid(stderr.trim().to_string())
    } else {
        TranscriptMutateError::Failed {
            action: action.as_str(),
            message: if stderr.trim().is_empty() {
                "transcript command failed".into()
            } else {
                stderr.trim().to_string()
            },
        }
    }
}

#[async_trait::async_trait]
impl TranscriptMutator for SubprocessTranscriptMutator {
    fn supports_exact_delete(&self) -> bool {
        cfg!(unix)
    }
    async fn exact_delete_context(
        &self,
        id: &str,
        scope: &rupu_cp::transcript_mutator::ExactTranscriptScope,
    ) -> Result<rupu_cp::transcript_mutator::ExactTranscriptContext, TranscriptMutateError> {
        let args = vec![
            "transcript".into(),
            "delete-context".into(),
            id.into(),
            "--workspace-id".into(),
            scope.workspace_id.clone(),
            "--path".into(),
            scope.path.clone(),
        ];
        let output = self.exact_process(args).await?;
        serde_json::from_slice(&output).map_err(|_| TranscriptMutateError::Failed {
            action: "inspect",
            message: "invalid bounded exact transcript context".into(),
        })
    }
    async fn delete_exact(
        &self,
        id: &str,
        scope: &rupu_cp::transcript_mutator::ExactTranscriptScope,
        revision: &str,
    ) -> Result<(), TranscriptMutateError> {
        let args = vec![
            "transcript".into(),
            "delete".into(),
            id.into(),
            "--force".into(),
            "--expected-workspace-id".into(),
            scope.workspace_id.clone(),
            "--expected-path".into(),
            scope.path.clone(),
            "--expected-revision".into(),
            revision.into(),
        ];
        self.exact_process(args).await.map(|_| ())
    }

    async fn mutate(
        &self,
        id: &str,
        action: TranscriptAction,
        ignore_liveness: bool,
    ) -> Result<(), TranscriptMutateError> {
        let argv = build_argv(id, action, ignore_liveness);
        let out = tokio::process::Command::new(&self.exe)
            .args(&argv)
            .output()
            .await
            .map_err(|e| TranscriptMutateError::Failed {
                action: action.as_str(),
                message: e.to_string(),
            })?;
        if out.status.success() {
            return Ok(());
        }
        Err(classify_failure(
            action,
            &String::from_utf8_lossy(&out.stderr),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_includes_force_only_for_delete() {
        assert_eq!(
            build_argv("run_abc", TranscriptAction::Archive, false),
            vec!["transcript", "archive", "run_abc"]
        );
        assert_eq!(
            build_argv("run_abc", TranscriptAction::Delete, false),
            vec!["transcript", "delete", "run_abc", "--force"]
        );
    }

    #[test]
    fn argv_appends_ignore_liveness_when_set() {
        assert_eq!(
            build_argv("run_abc", TranscriptAction::Archive, true),
            vec!["transcript", "archive", "run_abc", "--ignore-liveness"]
        );
        assert_eq!(
            build_argv("run_abc", TranscriptAction::Delete, true),
            vec![
                "transcript",
                "delete",
                "run_abc",
                "--force",
                "--ignore-liveness"
            ]
        );
    }

    #[test]
    fn classify_maps_stderr_to_variants() {
        // Invalid/conflict: session-owned transcript — the safety-critical case.
        assert!(matches!(
            classify_failure(
                TranscriptAction::Archive,
                "transcript run_abc is managed by session ses_1; use `rupu session archive|delete` instead"
            ),
            TranscriptMutateError::Invalid(_)
        ));
        // NotFound: missing transcript.
        assert!(matches!(
            classify_failure(TranscriptAction::Archive, "transcript not found: run_abc"),
            TranscriptMutateError::NotFound(_)
        ));
        // Invalid: already archived.
        assert!(matches!(
            classify_failure(
                TranscriptAction::Archive,
                "transcript already archived: run_abc"
            ),
            TranscriptMutateError::Invalid(_)
        ));
        // Invalid: delete without --force (defensive; the adapter always
        // passes it, but classification should still be correct).
        assert!(matches!(
            classify_failure(
                TranscriptAction::Delete,
                "transcript delete requires --force"
            ),
            TranscriptMutateError::Invalid(_)
        ));
        // Failed: unrecognised stderr falls through.
        assert!(matches!(
            classify_failure(TranscriptAction::Delete, "disk error writing metadata"),
            TranscriptMutateError::Failed { .. }
        ));
        // Invalid: I4's in-flight liveness refusal (not just session-owned).
        assert!(matches!(
            classify_failure(
                TranscriptAction::Delete,
                "cannot delete transcript run_abc: it is still running (owning process 4242 is alive)"
            ),
            TranscriptMutateError::Invalid(_)
        ));
    }
}

impl SubprocessTranscriptMutator {
    /// The owned task finishes even if the HTTP waiter is dropped. A timeout or
    /// process failure is unconfirmed, never a signal to retry deletion.
    async fn exact_process(&self, args: Vec<String>) -> Result<Vec<u8>, TranscriptMutateError> {
        use tokio::io::AsyncReadExt;
        static GATE: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        let permit = GATE
            .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                TranscriptMutateError::Invalid("exact transcript operations busy".into())
            })?;
        let exe = self.exe.clone();
        let worker = tokio::spawn(async move {
            let _permit = permit;
            let mut child = tokio::process::Command::new(exe)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| exact_failure("could not start owning transcript command"))?;
            let collect = async {
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| exact_failure("transcript stdout unavailable"))?;
                let stderr = child
                    .stderr
                    .take()
                    .ok_or_else(|| exact_failure("transcript stderr unavailable"))?;
                let mut output = Vec::new();
                let mut error = Vec::new();
                let mut stdout = stdout.take(65_537);
                let mut stderr = stderr.take(16_385);
                tokio::try_join!(
                    stdout.read_to_end(&mut output),
                    stderr.read_to_end(&mut error)
                )
                .map_err(|_| exact_failure("transcript response read failed"))?;
                if output.len() > 65_536 || error.len() > 16_384 {
                    return Err(exact_failure(
                        "transcript response exceeded its limit; result unconfirmed",
                    ));
                }
                let status = child
                    .wait()
                    .await
                    .map_err(|_| exact_failure("transcript command status unavailable"))?;
                Ok((status, output, error))
            };
            let collected = match tokio::time::timeout(std::time::Duration::from_secs(10), collect).await {
                Ok(result) => result,
                Err(_) => Err(exact_failure("exact transcript operation timed out; result unconfirmed; do not retry automatically")),
            };
            let (status, output, error) = match collected {
                Ok(value) => value,
                Err(error) => {
                    // Retain admission until the actual child has exited, including
                    // after the HTTP waiter times out or is cancelled.
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(error);
                }
            };
            if !status.success() {
                let message = String::from_utf8_lossy(&error);
                return Err(if message.contains("exact_transcript_not_found") {
                    TranscriptMutateError::NotFound(message.trim().into())
                } else if message.contains("exact_transcript_unsupported") {
                    TranscriptMutateError::Unsupported(message.trim().into())
                } else if message.contains("exact_transcript_conflict")
                    || message.contains("exact_transcript_unavailable")
                {
                    TranscriptMutateError::Invalid(message.trim().into())
                } else {
                    exact_failure("exact transcript operation failed; result unconfirmed; refresh before any further action")
                });
            }
            Ok(output)
        });
        // Dropping this JoinHandle detaches, rather than cancels, the owner.
        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .map_err(|_| exact_failure("exact transcript operation timed out; result unconfirmed; do not retry automatically"))?
            .map_err(|_| exact_failure("exact transcript task unavailable; result unconfirmed"))?
    }
}
fn exact_failure(message: &str) -> TranscriptMutateError {
    TranscriptMutateError::Failed {
        action: "delete",
        message: message.into(),
    }
}

#[cfg(all(test, unix))]
mod exact_process_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn executable(script: &str) -> (tempfile::TempDir, SubprocessTranscriptMutator) {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("fixture-command");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (t, SubprocessTranscriptMutator { exe: path })
    }
    #[tokio::test]
    async fn exact_owned_command_finishes_after_waiter_cancellation() {
        let (_t,adapter)=executable("printf started > \"$0.started\"\nsleep 0.1\nprintf finished > \"$0.finished\"\nprintf '{}'");
        let started = adapter.exe.with_extension("started");
        let finished = adapter.exe.with_extension("finished");
        let waiter = tokio::spawn(async move { adapter.exact_process(Vec::new()).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !started.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !finished.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(finished).unwrap(), "finished");
    }
    #[tokio::test]
    async fn exact_command_output_is_bounded() {
        let (_t, adapter) = executable("head -c 70000 /dev/zero");
        let error = adapter.exact_process(Vec::new()).await.unwrap_err();
        assert!(error.to_string().contains("exceeded its limit"));
    }
}
