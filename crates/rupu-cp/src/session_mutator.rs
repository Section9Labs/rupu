//! Port: archive / restore / delete sessions. rupu-cp defines it; rupu-cli's
//! `cp serve` provides the subprocess adapter that shells `rupu session
//! archive|restore|delete <id>`. `None` → the endpoints return 501.

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Archive,
    Restore,
    Delete,
}

impl SessionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionAction::Archive => "archive",
            SessionAction::Restore => "restore",
            SessionAction::Delete => "delete",
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
