//! Workflow + Step structs + YAML parser.
//!
//! Supports linear orchestrations with conditional step execution
//! (`when:`), per-step / workflow-level error tolerance
//! (`continue_on_error`), typed workflow inputs (`inputs:`), and
//! a `trigger:` declaration (manual / cron / event). The cron
//! runtime and event-webhook receiver are deferred (this PR only
//! parses + validates the declaration; manual is the existing
//! behavior). Parallel steps + panel steps also deferred — see
//! TODO.md.

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
    #[error("trigger.on=`cron` requires a non-empty `cron:` field")]
    TriggerCronMissing,
    #[error("trigger.on=`cron`: `cron:` value `{value}` is not a valid 5-field cron expression: {reason}")]
    TriggerCronInvalid { value: String, reason: String },
    #[error("trigger.on=`event` requires a non-empty `event:` field")]
    TriggerEventMissing,
    #[error("trigger.on=`{kind}` does not accept a `{field}:` field; remove it")]
    TriggerExtraneousField {
        kind: &'static str,
        field: &'static str,
    },
}

/// How a workflow gets kicked off. Manual is the existing behavior
/// (CLI `rupu workflow run <name>`); cron and event declarations
/// parse + validate today but the scheduler / webhook receiver that
/// actually fire them are deferred to follow-up PRs (see TODO.md →
/// "Workflow triggers" multi-PR initiative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    #[default]
    Manual,
    Cron,
    Event,
}

impl TriggerKind {
    fn name(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Cron => "cron",
            Self::Event => "event",
        }
    }
}

/// Top-level `trigger:` block. The cron + event fields are mutually
/// exclusive with each other and with the implicit manual default.
/// Validation happens in [`Workflow::parse`] so users see a clear
/// error before the workflow loads.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    #[serde(default)]
    pub on: TriggerKind,
    /// Standard 5-field cron expression (`min hour dom mon dow`).
    /// Required when `on: cron`; rejected otherwise.
    #[serde(default)]
    pub cron: Option<String>,
    /// Event identifier, e.g. `github.pr.opened`,
    /// `github.issue.created`, `issue.state_changed`. Required when
    /// `on: event`; rejected otherwise. The vocabulary is enforced by
    /// the webhook receiver in a future PR; this PR accepts any
    /// non-empty string so the schema can land before the runtime.
    #[serde(default)]
    pub event: Option<String>,
    /// Optional minijinja-style filter expression evaluated against
    /// the event payload (`{{event.repo.name == 'rupu'}}`). Only
    /// meaningful when `on: event`. Rendering happens at trigger time
    /// in the future event-receiver PR; this PR just preserves the
    /// string verbatim.
    #[serde(default)]
    pub filter: Option<String>,
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
    /// How this workflow gets kicked off. Defaults to manual when
    /// omitted (the legacy behavior — `rupu workflow run <name>` from
    /// the CLI). Cron + event declarations are accepted at parse time
    /// but the runtime that fires them is in a follow-up PR.
    #[serde(default)]
    pub trigger: Trigger,
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
        validate_trigger(&wf.trigger)?;
        Ok(wf)
    }

    pub fn parse_file(path: &std::path::Path) -> Result<Self, WorkflowParseError> {
        let s = std::fs::read_to_string(path)?;
        Self::parse(&s)
    }
}

/// Validate the trigger block's cross-field constraints. The cron
/// runtime + webhook receiver come in follow-up PRs; here we just
/// reject malformed declarations so authors see clear errors at
/// parse time.
fn validate_trigger(trigger: &Trigger) -> Result<(), WorkflowParseError> {
    match trigger.on {
        TriggerKind::Manual => {
            if trigger.cron.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Manual.name(),
                    field: "cron",
                });
            }
            if trigger.event.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Manual.name(),
                    field: "event",
                });
            }
            if trigger.filter.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Manual.name(),
                    field: "filter",
                });
            }
        }
        TriggerKind::Cron => {
            let value = trigger
                .cron
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or(WorkflowParseError::TriggerCronMissing)?;
            validate_cron_expression(value)?;
            if trigger.event.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Cron.name(),
                    field: "event",
                });
            }
            if trigger.filter.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Cron.name(),
                    field: "filter",
                });
            }
        }
        TriggerKind::Event => {
            let _ = trigger
                .event
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or(WorkflowParseError::TriggerEventMissing)?;
            if trigger.cron.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Event.name(),
                    field: "cron",
                });
            }
        }
    }
    Ok(())
}

/// Lightweight 5-field cron validator. We don't need to interpret the
/// expression here (the cron runtime in a follow-up PR will use a
/// proper crate); this just catches obviously-malformed values at
/// parse time so the author isn't surprised at scheduler-startup.
fn validate_cron_expression(expr: &str) -> Result<(), WorkflowParseError> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(WorkflowParseError::TriggerCronInvalid {
            value: expr.to_string(),
            reason: format!(
                "expected 5 fields (min hour dom mon dow); got {}",
                fields.len()
            ),
        });
    }
    // Each field must be `*`, `*/N`, a number, a range `N-M`, a list
    // `N,M,...`, or any combination thereof. Reject patently bogus
    // characters; leave full semantic validation to the scheduler.
    for (idx, field) in fields.iter().enumerate() {
        if field.is_empty()
            || !field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '*' | '/' | '-' | ','))
        {
            return Err(WorkflowParseError::TriggerCronInvalid {
                value: expr.to_string(),
                reason: format!(
                    "field {} (`{}`) contains invalid characters",
                    idx + 1,
                    field
                ),
            });
        }
    }
    Ok(())
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
