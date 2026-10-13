//! The launch tools (spec W7): `dispatch` starts a child run of any
//! [`ChildKind`], `join` collects children, `workflows.generate` authors a
//! workflow for a later `dispatch {kind: workflow, file}`. Each is a thin
//! caller of the run's [`Launcher`] / [`WorkflowGenerator`]: allowlists,
//! depth, the permission ceiling, codenames and the children ledger are the
//! launcher's.
//!
//! The pre-W7 names are aliases that answer in their old shapes (D16), told
//! apart by [`crate::CallContext::called_as`]: `dispatch_agent` and
//! `dispatch_agents_parallel` (synchronous sub-agents), and the agentiflow
//! `dispatch {agent, prompt}`, `run_workflow`, `generate_workflow` and
//! `join {handle}`.
//!
//! [`ChildKind`]: crate::ports::ChildKind
//! [`Launcher`]: crate::ports::Launcher
//! [`WorkflowGenerator`]: crate::ports::WorkflowGenerator

pub mod dispatch;
pub mod generate;
pub mod join;

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::tool::ToolError;

pub use dispatch::DispatchTool;
pub use generate::WorkflowsGenerateTool;
pub use join::JoinTool;

/// How long `join` (and `dispatch {wait: true}`) waits when `timeout_secs`
/// is omitted.
pub const DEFAULT_JOIN_TIMEOUT_SECS: u64 = 300;

/// Hard ceiling on `timeout_secs`, so one call cannot park a turn forever;
/// join again to keep waiting.
pub const MAX_JOIN_TIMEOUT_SECS: u64 = 3600;

/// How long a legacy synchronous `dispatch_agent` waits: until the child
/// ends, as it always did.
pub(crate) const SYNC_WAIT: Duration = Duration::from_secs(365 * 24 * 3600);

/// `timeout_secs`: absent / `null` is the default, a non-negative integer is
/// seconds (capped at [`MAX_JOIN_TIMEOUT_SECS`]), anything else is an error
/// rather than silently ignored.
pub(crate) fn parse_timeout(input: &Value) -> Result<Duration, ToolError> {
    match input.get("timeout_secs") {
        None | Some(Value::Null) => Ok(Duration::from_secs(DEFAULT_JOIN_TIMEOUT_SECS)),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Duration::from_secs(n.min(MAX_JOIN_TIMEOUT_SECS))),
            None => Err(ToolError::InvalidInput(
                "timeout_secs must be a non-negative integer when given".into(),
            )),
        },
    }
}

/// The tool's `inputs` object. `null` / absent is empty; anything but an
/// object is an error.
pub(crate) fn parse_inputs(input: &Value) -> Result<BTreeMap<String, Value>, ToolError> {
    match input.get("inputs") {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(o)) => Ok(o.clone().into_iter().collect()),
        Some(_) => Err(ToolError::InvalidInput(
            "`inputs` must be an object of name: value pairs".into(),
        )),
    }
}

/// An agent's prompt with its structured inputs folded in as a JSON block
/// (what `dispatch_agent` always did).
pub fn fold_inputs(prompt: &str, inputs: &BTreeMap<String, Value>) -> String {
    if inputs.is_empty() {
        return prompt.to_string();
    }
    let v = Value::Object(inputs.clone().into_iter().collect());
    let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    format!("{prompt}\n\nInputs (JSON):\n{pretty}")
}

/// A workflow's inputs as `--input k=v` strings: scalars stringify (a number
/// or bool as the model wrote it), `null` means "not given"; an array or
/// object has no `--input` form and is refused rather than guessed at.
pub fn workflow_input_strings(
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (k, v) in inputs {
        match v {
            Value::Null => {}
            Value::String(s) => {
                out.insert(k.clone(), s.clone());
            }
            Value::Number(_) | Value::Bool(_) => {
                out.insert(k.clone(), v.to_string());
            }
            _ => {
                return Err(format!(
                    "input `{k}` must be a string, number or boolean, not an array or object"
                ))
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn join_timeout_defaults_and_caps() {
        assert_eq!(
            parse_timeout(&json!({})).unwrap(),
            Duration::from_secs(DEFAULT_JOIN_TIMEOUT_SECS)
        );
        assert_eq!(
            parse_timeout(&json!({ "timeout_secs": 0 })).unwrap(),
            Duration::ZERO
        );
        assert_eq!(
            parse_timeout(&json!({ "timeout_secs": 999_999 })).unwrap(),
            Duration::from_secs(MAX_JOIN_TIMEOUT_SECS)
        );
        assert!(parse_timeout(&json!({ "timeout_secs": "soon" })).is_err());
        assert!(parse_timeout(&json!({ "timeout_secs": -1 })).is_err());
    }

    #[test]
    fn inputs_fold_into_the_prompt_as_a_json_block() {
        let inputs: BTreeMap<String, Value> = [("subject".to_string(), json!("x"))].into();
        assert_eq!(
            fold_inputs("review", &inputs),
            "review\n\nInputs (JSON):\n{\n  \"subject\": \"x\"\n}"
        );
        assert_eq!(fold_inputs("review", &BTreeMap::new()), "review");
    }

    #[test]
    fn workflow_inputs_stringify_scalars_and_refuse_structures() {
        let inputs: BTreeMap<String, Value> = [
            ("a".to_string(), json!("s")),
            ("b".to_string(), json!(3)),
            ("c".to_string(), json!(true)),
            ("d".to_string(), Value::Null),
        ]
        .into();
        let out = workflow_input_strings(&inputs).unwrap();
        assert_eq!(
            out.into_iter().collect::<Vec<_>>(),
            [
                ("a".to_string(), "s".to_string()),
                ("b".to_string(), "3".to_string()),
                ("c".to_string(), "true".to_string()),
            ]
        );
        let bad: BTreeMap<String, Value> = [("x".to_string(), json!([1]))].into();
        assert!(workflow_input_strings(&bad).is_err());
    }
}
