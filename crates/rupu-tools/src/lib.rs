//! rupu-tools — every tool rupu's runs can call, and the one way to decide,
//! grant and call them.
//!
//! - core fs/shell: [`bash`], [`read_file`], [`write_file`], [`edit_file`],
//!   [`grep`], [`glob`], [`ast_grep`];
//! - sub-agent dispatch: [`dispatch_agent`], [`dispatch_agents_parallel`];
//! - ledgers: [`coverage`], [`findings`], [`assets`];
//! - connectors over the run's SCM registry: [`scm`], [`issues`],
//!   [`github`], [`gitlab`].
//!
//! All tools implement the [`Tool`] trait and declare a
//! [`ToolDescriptor`] (name, aliases, [`Effect`], needs). [`catalog`] lists
//! every descriptor rupu defines; [`bodies::body`] builds a run's tool from
//! its [`ToolContext`]. Permission is [`PermissionPolicy`]: a pure function of
//! a descriptor's effect and the run's [`PermissionMode`] — tools themselves
//! are not aware of permission state. The agent loop, `action:` workflow
//! steps ([`call`]) and `rupu mcp serve` ([`call`]) are three callers of the
//! same tools (spec W4).

pub mod assets;
pub mod bodies;
pub mod call;
pub mod catalog;
pub mod connector;
pub mod coverage;
pub mod coverage_emit;
pub mod descriptor;
pub mod findings;
pub mod github;
pub mod gitlab;
pub mod grant;
pub mod issues;
pub mod ledger;
pub mod output;
pub mod scm;
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
pub use call::CallOutcome;
pub use catalog::ToolCatalog;
pub use descriptor::{Alias, AliasScope, Effect, Service, ToolDescriptor, ACTION_SERVICES};
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
    AgentDispatcher, BashConfig, CallContext, DerivedEvent, DispatchError, DispatchOutcome,
    ParentLink, RunIdentity, SpawnPermission, Surface, Tool, ToolContext, ToolError, ToolOutput,
    ToolServices, WorkspaceScope,
};
pub use write_file::WriteFileTool;
