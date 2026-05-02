//! Action-protocol allowlist validator.
//!
//! Each workflow step declares an `actions:` allowlist. When the
//! agent emits actions during the step, the runner asks
//! [`validate_actions`] whether each is allowed. Disallowed actions
//! are logged in the transcript (`action_emitted` with `applied:
//! false`) but do not abort the run.

use rupu_agent::ActionEnvelope;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionValidationResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Check whether `action.kind` appears in `step_allowlist`.
pub fn validate_actions(
    action: &ActionEnvelope,
    step_allowlist: &[String],
) -> ActionValidationResult {
    if step_allowlist.iter().any(|k| k == &action.kind) {
        ActionValidationResult {
            allowed: true,
            reason: None,
        }
    } else {
        ActionValidationResult {
            allowed: false,
            reason: Some("not in step allowlist".into()),
        }
    }
}
