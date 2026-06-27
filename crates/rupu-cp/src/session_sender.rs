/// A request to send a message (prompt) to a live session.
#[derive(Debug, Clone)]
pub struct SendMessageRequest {
    pub session_id: String,
    pub prompt: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Spawn(String),
}

/// Port: sends a message to a live session. rupu-cp defines it; rupu-cli's
/// `cp serve` provides the subprocess-spawning adapter. Returns the new run id.
#[async_trait::async_trait]
pub trait SessionSender: Send + Sync {
    async fn send(&self, req: SendMessageRequest) -> Result<String, SendError>;
}
