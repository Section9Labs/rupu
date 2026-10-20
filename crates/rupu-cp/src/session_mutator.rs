//! Port: archive / restore / delete / stop / compact sessions. rupu-cp
//! defines it; rupu-cli's `cp serve` provides the subprocess adapter that
//! shells `rupu session archive|restore|delete|stop <id>` and starts a
//! detached `rupu session compact <id>`. `None` → the endpoints return 501.

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Archive,
    Restore,
    Delete,
    /// `rupu session stop`: end the session's worker and its running turn.
    Stop,
}

impl SessionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionAction::Archive => "archive",
            SessionAction::Restore => "restore",
            SessionAction::Delete => "delete",
            SessionAction::Stop => "stop",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionMutateError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("invalid session state: {0}")]
    Invalid(String),
    #[error("failed to {action} session: {message}")]
    Failed {
        action: &'static str,
        message: String,
    },
}

#[async_trait]
pub trait SessionMutator: Send + Sync {
    async fn mutate(&self, id: &str, action: SessionAction) -> Result<(), SessionMutateError>;

    /// Start `rupu session compact <id> [--window <window>] --compaction-id
    /// <compaction_id>` detached, its output to `log`, and return the child's
    /// pid once it has started: the compaction itself is a model call and
    /// lands in the session record when it ends. The caller has already
    /// recorded `compaction_id` (`rupu_runtime::services::session_compaction`).
    async fn compact(
        &self,
        id: &str,
        compaction_id: &str,
        window: Option<u32>,
        log: &std::path::Path,
    ) -> Result<u32, SessionMutateError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Stub;
    #[async_trait]
    impl SessionMutator for Stub {
        async fn mutate(&self, _id: &str, action: SessionAction) -> Result<(), SessionMutateError> {
            if action == SessionAction::Restore {
                return Err(SessionMutateError::NotFound("x".into()));
            }
            Ok(())
        }
        async fn compact(
            &self,
            _id: &str,
            _compaction_id: &str,
            _window: Option<u32>,
            _log: &std::path::Path,
        ) -> Result<u32, SessionMutateError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn dispatches_through_trait_object() {
        let m: Arc<dyn SessionMutator> = Arc::new(Stub);
        assert!(m.mutate("s1", SessionAction::Archive).await.is_ok());
        assert!(matches!(
            m.mutate("s1", SessionAction::Restore).await,
            Err(SessionMutateError::NotFound(_))
        ));
    }
}
