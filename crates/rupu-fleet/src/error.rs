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
    /// The message log reached `[messaging].log_max_mb`: nothing more can be
    /// sent in this space.
    #[error("the message log is full ({max_bytes} bytes, `[messaging].log_max_mb`): nothing more can be sent in this run")]
    LogFull { max_bytes: u64 },
    #[error(transparent)]
    Address(#[from] crate::address::AddressError),
    /// A write the space refuses: an operator message to the operator, or
    /// a run writing as `operator`.
    #[error("{0}")]
    Refused(String),
    /// The message is in the log, but delivering it failed partway: some
    /// recipients may have it. Sending it again would deliver it twice.
    #[error("message {id} is logged but delivering it failed: {source}; some recipients may already have it, so don't send it again")]
    PartlyDelivered {
        id: String,
        #[source]
        source: Box<FleetError>,
    },
    #[error(
        "'{channel}' is a broadcast channel, not a direct inbox; use broadcast_send/read_broadcast"
    )]
    NotAnInbox { channel: String },
}

/// What a [`crate::Bus`] write fails with.
pub type BusError = FleetError;
