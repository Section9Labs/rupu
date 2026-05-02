//! Workflow + Step structs + YAML parser. Real impl in Task 9.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowParseError {
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("v0 does not support `{key}` in workflow YAML; deferred to Slice B")]
    UnsupportedKey { key: &'static str },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow;
