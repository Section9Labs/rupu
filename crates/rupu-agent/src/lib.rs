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

// Continue an interrupted agent run from its transcript (recover-on-interrupt).
pub mod continuation;
// Pre-turn collector pipeline: TurnCollector port + data-never-authority
// injection wrapper (agentiflows plan 1, Part B).
pub mod collector;
// Task 18: coverage tool wrappers (injected when concerns: is present)
pub mod coverage_tools;
// Open-file limit management + agent-run admission pacing.
pub mod fd_budget;
// implemented in Task 3
pub mod loader;
// Tasks 17+18: MCP tool adapter + runner wiring
pub mod mcp_tool;
// Classifies every reply and provider error into an outcome (response-outcomes plan 2).
pub mod outcome;
// implemented in Task 4
pub mod permission;
// Task 3 (transcript fidelity plan 1): reconstructs the exact provider
// conversation from a v2 transcript — the inverse of the runner's emission
// contract.
pub mod replay;
// The recovery ladder: policy table, per-run state and the HopBuilder port (response-outcomes plan 2).
pub mod recovery;
// implemented in Task 5/7
pub mod runner;
// implemented in Task 2
pub mod spec;
// implemented in Task 6
pub mod tool_registry;

pub use collector::{
    Cadence, CollectorPipeline, CommandCollector, Injection, InjectionKind, TurnCollector,
    TurnContext,
};
pub use fd_budget::load_agent_admitted;
pub use loader::{load_agent, load_agents, AgentLoadError};
pub use permission::{parse_mode, resolve_mode, PermissionDecision, PermissionPrompt};
pub use recovery::{Hop, HopBuilder, RecoveryOpts};
pub use runner::{
    compact_messages, run_agent, run_agent_full, run_agent_with_limits, AgentRunOpts,
    BypassDecider, CompactionOutcome, MockProvider, OnToolCallCallback, OnUsageCallback, RunError,
    RunExit, RunResult, ScriptedTurn, UsageKind, UsageTurn,
};
pub use rupu_providers::types::StopReason;
pub use spec::{AgentSpec, AgentSpecParseError};
pub use tool_registry::{default_tool_registry, ToolRegistry};
