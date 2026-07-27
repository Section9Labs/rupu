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
/// confirmation step before invoking the endpoint.
pub(crate) fn build_argv(id: &str, action: TranscriptAction) -> Vec<String> {
    let mut argv = vec![
        "transcript".to_string(),
        action.as_str().to_string(),
        id.to_string(),
    ];
    if action == TranscriptAction::Delete {
        argv.push("--force".to_string());
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
    async fn mutate(
        &self,
        id: &str,
        action: TranscriptAction,
    ) -> Result<(), TranscriptMutateError> {
        let argv = build_argv(id, action);
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
            build_argv("run_abc", TranscriptAction::Archive),
            vec!["transcript", "archive", "run_abc"]
        );
        assert_eq!(
            build_argv("run_abc", TranscriptAction::Delete),
            vec!["transcript", "delete", "run_abc", "--force"]
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
            classify_failure(TranscriptAction::Delete, "transcript delete requires --force"),
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
