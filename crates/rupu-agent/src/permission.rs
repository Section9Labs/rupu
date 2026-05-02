//! Permission mode resolution + prompt UX. Real impl lands in Tasks 4-5.

use rupu_tools::PermissionMode;

/// The outcome of a permission decision for a single tool invocation.
pub enum PermissionDecision {
    /// Allow this one invocation.
    Allow,
    /// Allow all future invocations of this tool for the remainder of the run.
    AllowAlwaysForToolThisRun,
    /// Deny this invocation.
    Deny,
    /// Abort the entire run.
    StopRun,
}

/// Interactive prompt presenter for Ask-mode permission decisions.
pub struct PermissionPrompt;

/// Resolve the effective [`PermissionMode`] from the layered configuration
/// sources (CLI flag overrides agent frontmatter overrides project config
/// overrides global config).
pub fn resolve_mode(
    _cli_flag: Option<PermissionMode>,
    _agent_frontmatter: Option<PermissionMode>,
    _project_config: Option<PermissionMode>,
    _global_config: Option<PermissionMode>,
) -> PermissionMode {
    todo!("resolve_mode lands in Task 4")
}

/// Parse a string slice into a [`PermissionMode`], returning `None` for
/// unrecognised values.
pub fn parse_mode(_s: &str) -> Option<PermissionMode> {
    todo!("parse_mode lands in Task 4")
}
