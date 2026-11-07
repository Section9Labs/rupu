//! Bundled agents are resolved before either disk layer. The session harness
//! is a reserved runtime identity, not a project template that can be shadowed.

use crate::AgentSpec;

pub const SESSION_AGENT_NAME: &str = "rupu-session";
const SESSION_AGENT: &str = include_str!("builtin/rupu-session.md");

/// One provider-independent root definition, shipped in every runtime binary.
/// Provider/model selection belongs to the session launch, never this file.
pub fn session_agent() -> AgentSpec {
    AgentSpec::parse(SESSION_AGENT).expect("bundled rupu-session agent must parse")
}

/// Return a reserved definition without opening project or global agent files.
pub fn find(name: &str) -> Option<AgentSpec> {
    (name == SESSION_AGENT_NAME).then(session_agent)
}
