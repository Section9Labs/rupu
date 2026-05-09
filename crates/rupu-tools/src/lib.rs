//! rupu-tools — six tools the agent runtime can invoke.
//!
//! - [`bash`] — execute a shell command in the workspace cwd.
//! - [`read_file`] — read a file with line-numbered output.
//! - [`write_file`] — create or overwrite a file.
//! - [`edit_file`] — exact-match string replacement.
//! - [`grep`] — search across the workspace (ripgrep-backed).
//! - [`glob`] — file pattern matching.
//!
//! All tools implement the [`Tool`] trait. Permission gating (the
//! `ask` / `bypass` / `readonly` modes) lives in [`permission`] and
//! is consumed by the agent runtime in Plan 2 — tools themselves
//! are not aware of permission state.

pub mod tool;

mod path_scope;

// implemented in Task 18 (PermissionGate decision API)
pub mod permission;
// implemented in Task 19 (line-numbered output + workspace-scope check)
pub mod read_file;
// implemented in Task 20 (create/overwrite + FileEdit derived)
pub mod write_file;
// implemented in Task 21 (exact-match replacement + FileEdit derived)
pub mod edit_file;
// implemented in Task 22 (ripgrep delegate)
pub mod grep;
// implemented in Task 23 (recursive pattern matching)
pub mod glob;
// implemented in Task 24 (subprocess execution with timeout + env allowlist)
pub mod bash;
// sub-agent dispatch (spec 2026-05-08): single-child synchronous.
pub mod dispatch_agent;

pub use bash::BashTool;
pub use dispatch_agent::DispatchAgentTool;
pub use edit_file::EditFileTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use permission::{PermissionGate, PermissionMode};
pub use read_file::ReadFileTool;
pub use tool::{
    AgentDispatcher, DerivedEvent, DispatchError, DispatchOutcome, Tool, ToolContext, ToolError,
    ToolOutput,
};
pub use write_file::WriteFileTool;
