//! Action protocol allowlist validator. Real impl in Task 11.

use rupu_agent::ActionEnvelope;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionValidationResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

pub fn validate_actions(
    _action: &ActionEnvelope,
    _step_allowlist: &[String],
) -> ActionValidationResult {
    todo!("validate_actions lands in Task 11")
}
