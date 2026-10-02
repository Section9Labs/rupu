use thiserror::Error;

#[derive(Debug, Error)]
pub enum FleetError {
    #[error("fleet io error during {action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("fleet serialization error: {0}")]
    Ser(#[from] serde_json::Error),
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("work unit {key} already claimed by {holder}")]
    Claimed { key: String, holder: String },
    #[error("inbox for {participant} is full (cap {cap})")]
    InboxFull { participant: String, cap: usize },
}
