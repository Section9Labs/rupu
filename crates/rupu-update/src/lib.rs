#![deny(clippy::all)]
// disallowed_methods is grouped under clippy::all in this clippy version;
// the workspace-level `disallowed_methods = "warn"` (Cargo.toml) is meant
// to keep this crate buildable under clippy until Plan 2 migrates it onto
// rupu-netflow, but a crate-level `deny(clippy::all)` would otherwise
// silently escalate it back to a hard error. Downgrade it back to warn
// here explicitly; Plan 2's final task removes this override along with
// the last raw reqwest call in this crate.
#![warn(clippy::disallowed_methods)]
#![forbid(unsafe_code)]

pub mod model;
pub use model::{parse_releases, Asset, Channel, Release, ReleaseSource};

pub mod select;
pub use select::{asset_for, select_latest};

pub mod decide;
pub use decide::{current_platform, decide, is_dev_exe, platform_name, Decision};

pub mod verify;
pub use verify::{BinaryCheck, CodesignCheck, NoopBinaryCheck};

pub mod install;

pub mod notice;

pub mod github;
pub use github::{download_bytes, releases_api_url, GithubReleaseSource};

pub mod flow;
pub use flow::{check, install, ApplyStrategy, CheckOutcome, DirectApply, UpdateContext};

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
