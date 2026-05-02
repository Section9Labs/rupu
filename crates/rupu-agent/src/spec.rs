//! Agent file format. `.md` with YAML frontmatter; body is the system
//! prompt. Real impl lands in Task 2.

use thiserror::Error;

/// Errors that can occur while parsing an agent spec file.
#[derive(Debug, Error)]
pub enum AgentSpecParseError {
    #[error("missing frontmatter")]
    MissingFrontmatter,
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Parsed representation of an agent file (frontmatter + system prompt body).
pub struct AgentSpec;
