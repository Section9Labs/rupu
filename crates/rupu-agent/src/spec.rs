//! Agent file format. `.md` with YAML frontmatter; body is the system
//! prompt.
//!
//! Compatibility: matches Okesu / Claude conventions (frontmatter
//! keys: `name`, `description`, `provider`, `model`, `tools`,
//! `maxTurns`, `permissionMode`). Unknown fields are rejected at parse
//! time so typos like `permision_mode` surface as errors.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur while parsing an agent spec file.
#[derive(Debug, Error)]
pub enum AgentSpecParseError {
    #[error("missing frontmatter delimiter (expected ---)")]
    MissingFrontmatter,
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default, rename = "maxTurns")]
    max_turns: Option<u32>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
}

/// Parsed agent file. The body of the markdown is the system prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub name: String,
    pub description: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    pub max_turns: Option<u32>,
    pub permission_mode: Option<String>,
    pub system_prompt: String,
}

impl AgentSpec {
    /// Parse a string containing the full agent file (frontmatter +
    /// body). The frontmatter must be delimited by `---` lines at the
    /// very start; everything after the second `---` is the body.
    pub fn parse(s: &str) -> Result<Self, AgentSpecParseError> {
        let s = s
            .strip_prefix("---\n")
            .ok_or(AgentSpecParseError::MissingFrontmatter)?;
        let end = s
            .find("\n---\n")
            .or_else(|| s.find("\n---"))
            .ok_or(AgentSpecParseError::MissingFrontmatter)?;
        let yaml = &s[..end];
        let body = s[end..]
            .trim_start_matches('\n')
            .trim_start_matches("---")
            .trim_start_matches('\n');
        let fm: Frontmatter = serde_yaml::from_str(yaml)?;
        Ok(AgentSpec {
            name: fm.name,
            description: fm.description,
            provider: fm.provider,
            model: fm.model,
            tools: fm.tools,
            max_turns: fm.max_turns,
            permission_mode: fm.permission_mode,
            system_prompt: body.to_string(),
        })
    }

    /// Read + parse an agent file from disk.
    pub fn parse_file(path: &std::path::Path) -> Result<Self, AgentSpecParseError> {
        let s = std::fs::read_to_string(path)?;
        Self::parse(&s)
    }
}
