//! rupu-config — TOML-backed configuration with global+project layering.
//!
//! Two-tier configuration: the global file at `~/.rupu/config.toml` is
//! loaded first and the project file at `<repo>/.rupu/config.toml`
//! deep-merges into it (project wins on conflict; arrays REPLACE
//! globals so users can subtract). See [`layer::layer_files`] for the
//! merge rules.

pub mod config;

// `layer_files` is implemented in Task 8 (TDD); the module exists here so
// that the lib re-export shape is stable from skeleton onward.
pub mod layer;

pub mod provider_config;

pub use config::{BashConfig, Config, RetryConfig};
pub use layer::{layer_files, LayerError};
pub use provider_config::ProviderConfig;
