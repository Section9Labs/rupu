//! Global+project config layering.
//!
//! Real implementation lands in Task 8 of Plan 1; this stub exists so
//! the public surface of `rupu-config` is stable from the skeleton stage.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LayerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Layer global and project config files. Returns the merged `Config`.
///
/// Implemented in Task 8.
pub fn layer_files(
    _global: Option<&std::path::Path>,
    _project: Option<&std::path::Path>,
) -> Result<crate::Config, LayerError> {
    unimplemented!("layer_files lands in Task 8")
}
