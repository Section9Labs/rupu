//! rupu-tools — six tools the agent runtime can invoke.
//!
//! - [`bash`] — execute a shell command in the workspace cwd.
//! - [`read_file`] — read a file with line-numbered output.
//! - [`write_file`] — create or overwrite a file.
//! - [`edit_file`] — exact-match string replacement.
//! - [`grep`] — search across the workspace (ripgrep-backed).
//! - [`glob`] — file pattern matching.
//!
//! All tools implement the [`Tool`] trait and declare a
//! [`ToolDescriptor`] (name, aliases, [`Effect`], needs). [`catalog`] lists
//! every descriptor rupu defines. Permission is [`PermissionPolicy`]: a pure
//! function of a descriptor's effect and the run's [`PermissionMode`] —
//! tools themselves are not aware of permission state.

pub mod catalog;
pub mod coverage_emit;
pub mod descriptor;
pub mod grant;
pub mod output;
pub mod tool;

mod path_scope;

pub mod permission;
// implemented in Task 19 (line-numbered output + workspace-scope check)
pub mod read_file;
// implemented in Task 20 (create/overwrite + FileEdit derived)
pub mod write_file;
// implemented in Task 21 (exact-match replacement + FileEdit derived)
pub mod edit_file;
// implemented in Task 22 (ripgrep delegate)
pub mod grep;
// structural (tree-sitter) search — delegates to the `ast-grep` binary.
pub mod ast_grep;
// implemented in Task 23 (recursive pattern matching)
pub mod glob;
// implemented in Task 24 (subprocess execution with timeout + env allowlist)
pub mod bash;
// sub-agent dispatch (spec 2026-05-08): single-child synchronous.
pub mod dispatch_agent;
// sub-agent dispatch (spec 2026-05-08): fan-out parallel.
pub mod dispatch_agents_parallel;

pub use ast_grep::AstGrepTool;
pub use bash::BashTool;
pub use catalog::ToolCatalog;
pub use descriptor::{Alias, AliasScope, Effect, Service, ToolDescriptor};
pub use dispatch_agent::DispatchAgentTool;
pub use dispatch_agents_parallel::DispatchAgentsParallelTool;
pub use edit_file::EditFileTool;
pub use glob::GlobTool;
pub use grant::{
    AmbientGrant, GrantEntry, GrantError, GrantInputs, GrantReason, ResolvedGrant, ServiceSet,
    Unavailable, DEFAULT_GRANT,
};
pub use grep::GrepTool;
pub use permission::{
    AllowAlways, Decision, DenyReason, PermissionMode, PermissionPolicy, PromptAnswer,
    PromptRequest, Prompter, UnknownMode,
};
pub use read_file::ReadFileTool;
pub use tool::{
    AgentDispatcher, DerivedEvent, DispatchError, DispatchOutcome, SpawnPermission, Tool,
    ToolContext, ToolError, ToolOutput,
};
pub use write_file::WriteFileTool;
