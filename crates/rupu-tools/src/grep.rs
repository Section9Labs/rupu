//! `grep` tool — ripgrep-backed search across the workspace. Real
//! impl lands in Task 22 of Plan 1.

/// Workspace-scoped grep. Delegates to the `rg` binary. Implements
/// the [`crate::Tool`] trait in Task 22.
pub struct GrepTool;
