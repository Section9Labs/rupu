//! rupu-agent — agent file format + agent loop + permission resolver.
//!
//! This crate is the integration point between `rupu-providers` (LLM
//! clients), `rupu-tools` (the six tools), and `rupu-transcript` (event
//! schema + JSONL writer). The agent loop sends messages to the
//! provider, dispatches tool calls, applies permission gating, and
//! streams events into the transcript.
//!
//! Agent files are markdown with YAML frontmatter (Okesu/Claude
//! convention). See [`spec::AgentSpec`].

// implemented in Task 11
pub mod action;
// implemented in Task 3
pub mod loader;
// Tasks 17+18: MCP tool adapter + runner wiring
pub mod mcp_tool;
// implemented in Task 4
pub mod permission;
// implemented in Task 5/7
pub mod runner;
// implemented in Task 2
pub mod spec;
// implemented in Task 6
pub mod tool_registry;

pub use action::{ActionEnvelope, ActionValidator};
pub use loader::{load_agent, load_agents, AgentLoadError};
pub use permission::{parse_mode, resolve_mode, PermissionDecision, PermissionPrompt};
pub use runner::{run_agent, AgentRunOpts, OnToolCallCallback, RunError, RunResult};
pub use spec::{AgentSpec, AgentSpecParseError};
pub use tool_registry::{default_tool_registry, ToolRegistry};
