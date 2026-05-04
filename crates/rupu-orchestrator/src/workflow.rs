//! Workflow + Step structs + YAML parser.
//!
//! Supports linear orchestrations with conditional step execution
//! (`when:`), per-step / workflow-level error tolerance
//! (`continue_on_error`), and typed workflow inputs (`inputs:`).
//! Parallel steps + panel steps remain deferred (see TODO.md).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowParseError {
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("workflow YAML key `{key}` is not yet supported (deferred — see TODO.md)")]
    UnsupportedKey { key: &'static str },
    #[error("workflow has no steps")]
    Empty,
    #[error("duplicate step id: {0}")]
    DuplicateStep(String),
    #[error("input `{name}`: invalid `default` for type `{ty}`: {reason}")]
    InvalidInputDefault {
        name: String,
        ty: &'static str,
        reason: String,
    },
    #[error("input `{name}`: `default` value is not in the declared `enum` ({allowed:?})")]
    DefaultNotInEnum { name: String, allowed: Vec<String> },
}

/// Declared type for a workflow input. Drives default-value
/// coercion + error messages on `--input` parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    String,
    Int,
    Bool,
}

impl InputType {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Int => "int",
            Self::Bool => "bool",
        }
    }
}

/// A workflow-level input declaration. Authors `inputs:` block in YAML;
/// users provide values at runtime via `rupu workflow run --input k=v`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    #[serde(rename = "type", default = "InputDef::default_type")]
    pub ty: InputType,
    #[serde(default)]
    pub required: bool,
    /// Default value as a YAML scalar. Coerced against `ty` at parse
    /// time so an invalid default shows up before the workflow runs.
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    /// Allowed values; if non-empty, the runtime rejects inputs not
    /// in this list. Stored as strings for simplicity (covers all
    /// three input types via stringification).
    #[serde(rename = "enum", default)]
    pub allowed: Vec<String>,
}

impl InputDef {
    fn default_type() -> InputType {
        InputType::String
    }
}

/// Workflow-level defaults inherited by every step. A step's own
/// override (when present) wins over the default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefaults {
    /// When `Some(true)`, every step inherits `continue_on_error: true`
    /// unless the step explicitly overrides. `Some(false)` and `None`
    /// behave the same way at runtime — the step-level default is also
    /// false — but `Some(false)` is preserved for round-trip clarity.
    #[serde(default)]
    pub continue_on_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub agent: String,
    #[serde(default)]
    pub actions: Vec<String>,
    /// Optional minijinja expression rendered against the step
    /// context (`inputs.*`, `steps.<id>.*`). When the rendered value
    /// is "false", "0", "", "no", or "off" (case-insensitive), the
    /// step is skipped. Any other rendered value runs the step.
    #[serde(default)]
    pub when: Option<String>,
    /// When `Some(true)`, a failure in this step is logged and the
    /// workflow continues to the next step. Overrides
    /// `WorkflowDefaults.continue_on_error`.
    #[serde(default)]
    pub continue_on_error: Option<bool>,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Typed input declarations. The runtime validates `--input k=v`
    /// values against these (required-ness, enum membership, type
    /// coercion) before the first step runs.
    #[serde(default)]
    pub inputs: BTreeMap<String, InputDef>,
    /// Per-workflow defaults shared by every step.
    #[serde(default)]
    pub defaults: WorkflowDefaults,
    pub steps: Vec<Step>,
}

impl Workflow {
    /// Parse a YAML string. Validates step-id uniqueness and input
    /// defaults / enum constraints; returns clear errors on failure.
    pub fn parse(s: &str) -> Result<Self, WorkflowParseError> {
        // Pre-scan for keys that are still deferred (panel steps,
        // explicit parallelism, gates). `when:` and
        // `continue_on_error:` are now supported; do NOT include them
        // in the unsupported list.
        for key in ["parallel", "for_each", "panelists", "gates"] {
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
        for (name, def) in &wf.inputs {
            validate_input_def(name, def)?;
        }
        Ok(wf)
    }

    pub fn parse_file(path: &std::path::Path) -> Result<Self, WorkflowParseError> {
        let s = std::fs::read_to_string(path)?;
        Self::parse(&s)
    }
}

/// Validate that an input declaration is internally consistent: the
/// `default` (if any) coerces to the declared `type`, and the `enum`
/// (if any) contains the default.
fn validate_input_def(name: &str, def: &InputDef) -> Result<(), WorkflowParseError> {
    if let Some(default) = &def.default {
        match def.ty {
            InputType::String => {
                if !default.is_string() {
                    return Err(WorkflowParseError::InvalidInputDefault {
                        name: name.to_string(),
                        ty: def.ty.name(),
                        reason: format!("expected string, got {default:?}"),
                    });
                }
            }
            InputType::Int => {
                if !default.is_i64() && !default.is_u64() {
                    return Err(WorkflowParseError::InvalidInputDefault {
                        name: name.to_string(),
                        ty: def.ty.name(),
                        reason: format!("expected integer, got {default:?}"),
                    });
                }
            }
            InputType::Bool => {
                if !default.is_bool() {
                    return Err(WorkflowParseError::InvalidInputDefault {
                        name: name.to_string(),
                        ty: def.ty.name(),
                        reason: format!("expected bool, got {default:?}"),
                    });
                }
            }
        }
        if !def.allowed.is_empty() {
            let stringified = yaml_scalar_to_string(default);
            if !def.allowed.contains(&stringified) {
                return Err(WorkflowParseError::DefaultNotInEnum {
                    name: name.to_string(),
                    allowed: def.allowed.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Render a YAML scalar to the same string form `--input k=v` would
/// produce, so default + user value can be compared against `enum`
/// uniformly.
pub(crate) fn yaml_scalar_to_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .into(),
    }
}

/// Leak a short literal key name to `&'static str`.
fn leak(key: &str) -> &'static str {
    match key {
        "parallel" => "parallel",
        "for_each" => "for_each",
        "panelists" => "panelists",
        "gates" => "gates",
        _ => "unknown",
    }
}
