//! Workflow + Step structs + YAML parser.
//!
//! v0 accepts only linear workflows: a `steps:` list executed in
//! order. Future-reserved keys (`parallel:`, `when:`, `gates:`) are
//! detected at parse time and produce
//! [`WorkflowParseError::UnsupportedKey`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowParseError {
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("v0 does not support `{key}` in workflow YAML; deferred to Slice B")]
    UnsupportedKey { key: &'static str },
    #[error("workflow has no steps")]
    Empty,
    #[error("duplicate step id: {0}")]
    DuplicateStep(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub agent: String,
    #[serde(default)]
    pub actions: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub steps: Vec<Step>,
}

impl Workflow {
    /// Parse a YAML string. v0 rejects any of the future-reserved
    /// step-level keys (`parallel`, `when`, `gates`).
    pub fn parse(s: &str) -> Result<Self, WorkflowParseError> {
        // Pre-scan the raw YAML for future-reserved keys to give a
        // friendly error message before serde gets to it.
        for key in ["parallel", "when", "gates"] {
            // Simple line-prefix match — sufficient for v0 since `parallel:` etc
            // would always appear at the start of an indented line.
            for line in s.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with(&format!("{key}:")) {
                    return Err(WorkflowParseError::UnsupportedKey { key: leak(key) });
                }
            }
        }

        let wf: Workflow = serde_yaml::from_str(s)?;
        if wf.steps.is_empty() {
            return Err(WorkflowParseError::Empty);
        }
        let mut seen = BTreeSet::new();
        for step in &wf.steps {
            if !seen.insert(step.id.clone()) {
                return Err(WorkflowParseError::DuplicateStep(step.id.clone()));
            }
        }
        Ok(wf)
    }

    pub fn parse_file(path: &std::path::Path) -> Result<Self, WorkflowParseError> {
        let s = std::fs::read_to_string(path)?;
        Self::parse(&s)
    }
}

/// Leak a short literal key name to `&'static str`. Avoids allocating
/// boxed strings just so the error type can carry the token.
fn leak(key: &str) -> &'static str {
    match key {
        "parallel" => "parallel",
        "when" => "when",
        "gates" => "gates",
        _ => "unknown",
    }
}
