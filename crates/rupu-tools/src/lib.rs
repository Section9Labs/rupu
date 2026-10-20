//! rupu-tools — every tool rupu's runs can call, and the one way to decide,
//! grant and call them.
//!
//! - core fs/shell: [`bash`], [`read_file`], [`write_file`], [`edit_file`],
//!   [`grep`], [`glob`], [`ast_grep`];
//! - child runs through the [`ports::Launcher`] port: [`launch`] (`dispatch`,
//!   `join`, `workflows.generate`);
//! - ledgers: [`coverage`], [`findings`], [`assets`];
//! - connectors over the run's SCM registry: [`scm`], [`issues`],
//!   [`github`], [`gitlab`];
//! - a flow's message bus: [`board`], [`msg`]; its live status through the
//!   [`ports::RunStatus`] port: [`goal`], [`budget`]; the agent/workflow
//!   catalog through the [`ports::CatalogPort`] port: [`roster`];
//! - a run's granted skills: [`skills`].
//!
//! All tools implement the [`Tool`] trait and declare a
//! [`ToolDescriptor`] (name, aliases, [`Effect`], needs). [`catalog`] lists
//! every descriptor rupu defines; [`bodies::body`] builds a run's tool from
//! its [`ToolContext`]. Permission is [`PermissionPolicy`]: a pure function of
//! a descriptor's effect and the run's [`PermissionMode`] — tools themselves
//! are not aware of permission state. The agent loop, `action:` workflow
//! steps ([`call`]) and the MCP endpoint (`rupu cp serve` /mcp, [`call`])
//! are three callers of the same tools (spec W4).

pub mod assets;
pub mod board;
pub mod bodies;
pub mod budget;
pub mod call;
pub mod catalog;
pub mod connector;
pub mod coverage;
pub mod coverage_emit;
pub mod descriptor;
pub mod findings;
pub mod github;
pub mod gitlab;
pub mod goal;
pub mod grant;
pub mod issues;
pub mod launch;
pub mod ledger;
pub mod msg;
pub mod output;
pub mod ports;
pub mod roster;
pub mod scm;
pub mod skills;
pub mod tool;

mod path_scope;
mod proc_tree;

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

pub use ast_grep::AstGrepTool;
pub use bash::{BashTool, RUN_ID_ENV};
pub use call::CallOutcome;
pub use catalog::ToolCatalog;
pub use descriptor::{Alias, AliasScope, Effect, Service, ToolDescriptor, ACTION_SERVICES};
pub use edit_file::EditFileTool;
pub use glob::GlobTool;
pub use grant::{
    edit_distance, AmbientGrant, GrantEntry, GrantError, GrantInputs, GrantReason, MessagingSwitch,
    ResolvedGrant, ServiceSet, Unavailable, DEFAULT_GRANT, WORKFLOW_MESSAGING,
};
pub use grep::GrepTool;
pub use permission::{
    AllowAlways, Decision, DenyReason, PermissionMode, PermissionPolicy, PromptAnswer,
    PromptRequest, Prompter, UnknownMode,
};
pub use ports::{
    CatalogPort, ChildHandle, ChildKind, ChildRequest, ChildState, ChildStatus, ChildTarget,
    ChildTokens, GeneratedWorkflow, JoinSet, LaunchError, Launcher, ParentEnd, PortError,
    RunStatus, Until, WorkflowGenerator, MAX_LAUNCH_DEPTH,
};
pub use read_file::ReadFileTool;
pub use tool::{
    BashConfig, CallContext, DerivedEvent, LaunchPool, LaunchScope, ParentLink, RunIdentity,
    SpawnPermission, Surface, Tool, ToolContext, ToolError, ToolOutput, ToolServices,
    WorkspaceScope,
};
pub use write_file::WriteFileTool;
