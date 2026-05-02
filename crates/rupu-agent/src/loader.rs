//! Agent loader — discovers project + global agents and resolves
//! shadowing. Real impl lands in Task 3.

use thiserror::Error;

/// Errors that can occur while loading agents from disk.
#[derive(Debug, Error)]
pub enum AgentLoadError {
    #[error("agent not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: crate::spec::AgentSpecParseError,
    },
}

/// Discover and load all agents from global and optional project directories.
/// Project agents shadow global agents with the same name.
pub fn load_agents(
    _global: &std::path::Path,
    _project: Option<&std::path::Path>,
) -> Result<Vec<crate::spec::AgentSpec>, AgentLoadError> {
    todo!("load_agents lands in Task 3")
}

/// Load a single agent by name, searching project directory first then global.
pub fn load_agent(
    _global: &std::path::Path,
    _project: Option<&std::path::Path>,
    _name: &str,
) -> Result<crate::spec::AgentSpec, AgentLoadError> {
    todo!("load_agent lands in Task 3")
}
