//! Port: archive / delete STANDALONE agent-run transcripts. rupu-cp defines
//! it; rupu-cli's `cp serve` provides the subprocess adapter that shells
//! `rupu transcript archive|delete <run_id>`. `None` → the endpoints return
//! 501.
//!
//! No `Restore` variant: `rupu transcript restore` does not exist (unlike
//! sessions/runs, which are restorable). Do not add one here without first
//! shipping the CLI verb.
//!
//! The subprocess adapter is load-bearing for safety, not just convenience:
//! the CLI's `ensure_standalone_transcript` guard refuses to archive/delete a
//! transcript whose metadata carries a `session_id` (it belongs to a session,
//! not a standalone run) — see `crates/rupu-cli/src/cmd/transcript.rs`. rupu-cp
//! must never reimplement the file move/delete directly, or that guard is
//! bypassed and a session-owned transcript could be silently clobbered.

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptAction {
    Archive,
    Delete,
}

impl TranscriptAction {
    pub fn as_str(self) -> &'static str {
        match self {
            TranscriptAction::Archive => "archive",
            TranscriptAction::Delete => "delete",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TranscriptMutateError {
    #[error("transcript not found: {0}")]
    NotFound(String),
    #[error("invalid transcript state: {0}")]
    Invalid(String),
    #[error("exact transcript deletion unsupported: {0}")]
    Unsupported(String),
    #[error("failed to {action} transcript: {message}")]
    Failed {
        action: &'static str,
        message: String,
    },
}

#[async_trait]
pub trait TranscriptMutator: Send + Sync {
    fn supports_exact_delete(&self) -> bool {
        false
    }
    async fn exact_delete_context(
        &self,
        _id: &str,
        _scope: &ExactTranscriptScope,
    ) -> Result<ExactTranscriptContext, TranscriptMutateError> {
        Err(TranscriptMutateError::Unsupported(
            "adapter does not implement exact source deletion".into(),
        ))
    }
    async fn delete_exact(
        &self,
        _id: &str,
        _scope: &ExactTranscriptScope,
        _revision: &str,
    ) -> Result<(), TranscriptMutateError> {
        Err(TranscriptMutateError::Unsupported(
            "adapter does not implement exact source deletion".into(),
        ))
    }

    /// `ignore_liveness` — skip the CLI's still-running check
    /// (`ensure_standalone_not_running` in
    /// `crates/rupu-cli/src/cmd/transcript.rs`). Defaults to `false`
    /// everywhere; only set `true` to recover a transcript whose recorded
    /// pid was reused by an unrelated process. Never bypasses the separate
    /// session-ownership guard (`ensure_standalone_transcript`), which has
    /// no override.
    async fn mutate(
        &self,
        id: &str,
        action: TranscriptAction,
        ignore_liveness: bool,
    ) -> Result<(), TranscriptMutateError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Stub;
    #[async_trait]
    impl TranscriptMutator for Stub {
        async fn mutate(
            &self,
            _id: &str,
            action: TranscriptAction,
            _ignore_liveness: bool,
        ) -> Result<(), TranscriptMutateError> {
            if action == TranscriptAction::Delete {
                return Err(TranscriptMutateError::NotFound("x".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatches_through_trait_object() {
        let m: Arc<dyn TranscriptMutator> = Arc::new(Stub);
        assert!(m
            .mutate("t1", TranscriptAction::Archive, false)
            .await
            .is_ok());
        assert!(matches!(
            m.mutate("t1", TranscriptAction::Delete, false).await,
            Err(TranscriptMutateError::NotFound(_))
        ));
    }
}

/// Explicit local source selected by the operator, independent of CLI cwd.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactTranscriptScope {
    pub workspace_id: String,
    pub path: String,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactTranscriptEffect {
    pub kind: String,
    pub path: String,
    /// Decimal byte count; never rounded through a JSON floating-point value.
    pub bytes: String,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactTranscriptLimits {
    pub metadata_bytes: usize,
    pub prologue_bytes: usize,
    pub effect_entries: usize,
    pub context_bytes: usize,
}
impl Default for ExactTranscriptLimits {
    fn default() -> Self {
        Self {
            metadata_bytes: 262_144,
            prologue_bytes: 262_144,
            effect_entries: 32,
            context_bytes: 65_536,
        }
    }
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactTranscriptContext {
    pub schema: String,
    pub id: String,
    pub workspace_id: String,
    pub host_id: String,
    pub path: String,
    /// ready | unavailable. Only ready carries a revision and effects.
    pub state: String,
    pub reason: Option<String>,
    pub source_kind: Option<String>,
    pub archived: Option<bool>,
    pub revision: Option<String>,
    pub observed_at: String,
    pub effects: Vec<ExactTranscriptEffect>,
    pub coordination: String,
    pub limits: ExactTranscriptLimits,
}
