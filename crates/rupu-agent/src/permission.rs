//! Permission mode resolution + interactive prompt UX.
//!
//! Resolution precedence (spec §"Permission model"):
//!   CLI flag > agent frontmatter > project config > global config > default (Ask)

use rupu_tools::PermissionMode;

/// Pick the effective mode. The interactive prompt UX (in this same
/// module, [`PermissionPrompt`]) consumes the result.
pub fn resolve_mode(
    cli_flag: Option<PermissionMode>,
    agent_frontmatter: Option<PermissionMode>,
    project_config: Option<PermissionMode>,
    global_config: Option<PermissionMode>,
) -> PermissionMode {
    cli_flag
        .or(agent_frontmatter)
        .or(project_config)
        .or(global_config)
        .unwrap_or(PermissionMode::Ask)
}

/// Parse the textual mode from agent frontmatter / config files.
/// Returns `None` for an unknown string (caller decides whether that's
/// a hard error or a "skip this layer").
pub fn parse_mode(s: &str) -> Option<PermissionMode> {
    match s {
        "ask" => Some(PermissionMode::Ask),
        "bypass" => Some(PermissionMode::Bypass),
        "readonly" => Some(PermissionMode::Readonly),
        _ => None,
    }
}

/// Operator decision for an `Ask`-mode tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow this single tool call.
    Allow,
    /// Allow all calls of this tool kind for the rest of this run.
    AllowAlwaysForToolThisRun,
    /// Deny this single tool call (agent sees `permission_denied`).
    Deny,
    /// Stop the run entirely.
    StopRun,
}

/// Carries the interactive `Ask`-mode prompt UX. Stub here; the
/// stdin-driven impl lands in Task 5.
pub struct PermissionPrompt;
