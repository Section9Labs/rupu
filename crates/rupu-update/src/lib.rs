#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod model;
pub use model::{parse_releases, Asset, Channel, Release, ReleaseSource};

pub mod select;
pub use select::{asset_for, select_latest};

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("unknown release channel: {0} (expected \"stable\" or \"beta\")")]
    BadChannel(String),
    #[error("failed to parse release data: {0}")]
    Parse(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("no {channel} build published for {platform}")]
    NoAssetForPlatform { channel: String, platform: String },
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("refusing to update a development build ({0})")]
    DevBuild(String),
    #[error("install failed: {0}")]
    Install(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
