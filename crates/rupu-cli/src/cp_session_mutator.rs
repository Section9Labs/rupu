//! `cp serve` adapter for rupu-cp's `SessionMutator` port. Shells
//! `rupu session archive|restore|delete <id>` using this same binary.

use std::path::PathBuf;

use rupu_cp::session_mutator::{SessionAction, SessionMutateError, SessionMutator};

/// Shells `rupu session archive|restore|delete <id>` children. `exe` is the
/// path to the running `rupu` binary (resolved via `std::env::current_exe()`
/// in `cp serve`).
pub struct SubprocessSessionMutator {
    pub exe: PathBuf,
}

/// Build the argv (after the executable) for a session mutation.
///
/// For `Delete` we always pass `--force` because the CP UI presents its own
/// confirmation step before invoking the endpoint.
pub(crate) fn build_argv(id: &str, action: SessionAction) -> Vec<String> {
    let mut argv = vec![
        "session".to_string(),
        action.as_str().to_string(),
        id.to_string(),
    ];
    if action == SessionAction::Delete {
        argv.push("--force".to_string());
    }
    argv
}

/// Map a failed child's stderr to the right error variant.
///
/// Real CLI messages (from `crates/rupu-cli/src/cmd/session.rs`):
/// - `"unknown session: {id}"` → [`SessionMutateError::NotFound`]
/// - `"session {id} is already archived"` → [`SessionMutateError::Invalid`]
/// - `"session {id} is already active"` → [`SessionMutateError::Invalid`]
/// - `"session delete requires --force"` → [`SessionMutateError::Invalid`]
/// - `"cannot {action} session {id} while the worker is still running"` →
///   [`SessionMutateError::Invalid`]
/// - anything else → [`SessionMutateError::Failed`]
pub(crate) fn classify_failure(action: SessionAction, stderr: &str) -> SessionMutateError {
    let s = stderr.to_ascii_lowercase();
    if s.contains("unknown session") {
        SessionMutateError::NotFound(stderr.trim().to_string())
    } else if s.contains("already archived")
        || s.contains("already active")
        || s.contains("requires --force")
        || s.contains("while the worker is still running")
    {
        SessionMutateError::Invalid(stderr.trim().to_string())
    } else {
        SessionMutateError::Failed {
            action: action.as_str(),
            message: if stderr.trim().is_empty() {
                "session command failed".into()
            } else {
                stderr.trim().to_string()
            },
        }
    }
}

#[async_trait::async_trait]
impl SessionMutator for SubprocessSessionMutator {
    async fn mutate(&self, id: &str, action: SessionAction) -> Result<(), SessionMutateError> {
        let argv = build_argv(id, action);
        let out = tokio::process::Command::new(&self.exe)
            .args(&argv)
            .output()
            .await
            .map_err(|e| SessionMutateError::Failed {
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
            build_argv("ses_abc", SessionAction::Archive),
            vec!["session", "archive", "ses_abc"]
        );
        assert_eq!(
            build_argv("ses_abc", SessionAction::Restore),
            vec!["session", "restore", "ses_abc"]
        );
        assert_eq!(
            build_argv("ses_abc", SessionAction::Delete),
            vec!["session", "delete", "ses_abc", "--force"]
        );
    }

    #[test]
    fn classify_maps_stderr_to_variants() {
        // NotFound: the real message from `read_session`
        assert!(matches!(
            classify_failure(SessionAction::Archive, "unknown session: ses_abc"),
            SessionMutateError::NotFound(_)
        ));
        // Invalid: already archived
        assert!(matches!(
            classify_failure(
                SessionAction::Archive,
                "session ses_abc is already archived"
            ),
            SessionMutateError::Invalid(_)
        ));
        // Invalid: running guard message
        assert!(matches!(
            classify_failure(
                SessionAction::Archive,
                "cannot archive session ses_abc while the worker is still running"
            ),
            SessionMutateError::Invalid(_)
        ));
        // Invalid: already active (restore path)
        assert!(matches!(
            classify_failure(SessionAction::Restore, "session ses_abc is already active"),
            SessionMutateError::Invalid(_)
        ));
        // Failed: unrecognised stderr falls through
        assert!(matches!(
            classify_failure(SessionAction::Delete, "disk error writing metadata"),
            SessionMutateError::Failed { .. }
        ));
    }
}
