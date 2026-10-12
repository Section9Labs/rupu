//! `SKILL.md`: YAML frontmatter between `---` lines, then the instructions.
//! Unknown keys are accepted: harnesses add their own, and rejecting them
//! would break format compatibility.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The agentskills.io limits. Published skills exceed them, so going over
/// is a warning; the index cuts a description at `MAX_DESCRIPTION_LEN`.
pub const MAX_DESCRIPTION_LEN: usize = 1024;
pub const MAX_COMPATIBILITY_LEN: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub allowed_tools: Vec<String>,
    pub disable_model_invocation: bool,
    /// Rules the skill breaks without being invalid, such as an over-limit
    /// `description`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrontmatterError {
    #[error("SKILL.md must start with a `---` frontmatter line")]
    MissingOpen,
    #[error("SKILL.md frontmatter has no closing `---` line")]
    MissingClose,
    #[error("frontmatter YAML: {0}")]
    Yaml(String),
    #[error("frontmatter has no `name`")]
    MissingName,
    #[error("frontmatter has no `description` (or it is empty)")]
    MissingDescription,
    #[error("`compatibility` is empty; give it text or leave it out")]
    EmptyCompatibility,
    #[error("`metadata.{0}` must be a string, number or boolean")]
    Metadata(String),
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, serde_yaml::Value>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<AllowedTools>,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AllowedTools {
    One(String),
    Many(Vec<String>),
}

pub fn parse_skill_md(text: &str) -> Result<(Frontmatter, String), FrontmatterError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return Err(FrontmatterError::MissingOpen),
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
    }
    if !closed {
        return Err(FrontmatterError::MissingClose);
    }
    let body: String = lines.collect();
    let body = body.trim_start_matches(['\r', '\n']).trim_end().to_owned();

    if yaml.trim().is_empty() {
        return Err(FrontmatterError::MissingName);
    }
    let raw: Raw =
        serde_yaml::from_str(&yaml).map_err(|e| FrontmatterError::Yaml(e.to_string()))?;

    let name = raw
        .name
        .filter(|n| !n.is_empty())
        .ok_or(FrontmatterError::MissingName)?;
    let description = raw
        .description
        .map(|d| d.trim().to_owned())
        .filter(|d| !d.is_empty())
        .ok_or(FrontmatterError::MissingDescription)?;
    let compatibility = raw.compatibility.map(|c| c.trim().to_owned());
    if compatibility.as_deref() == Some("") {
        return Err(FrontmatterError::EmptyCompatibility);
    }
    let mut warnings = Vec::new();
    let mut over_limit = |field: &str, value: &str, limit: usize| {
        let len = value.chars().count();
        if len > limit {
            warnings.push(format!(
                "`{field}` is {len} characters; the agentskills.io limit is {limit}"
            ));
        }
    };
    over_limit("description", &description, MAX_DESCRIPTION_LEN);
    if let Some(c) = &compatibility {
        over_limit("compatibility", c, MAX_COMPATIBILITY_LEN);
    }
    let mut metadata = BTreeMap::new();
    for (k, v) in raw.metadata {
        let s = match v {
            serde_yaml::Value::String(s) => s,
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::Bool(b) => b.to_string(),
            serde_yaml::Value::Null => String::new(),
            _ => return Err(FrontmatterError::Metadata(k)),
        };
        metadata.insert(k, s);
    }
    let allowed_tools = match raw.allowed_tools {
        None => Vec::new(),
        Some(AllowedTools::One(s)) => split_tool_list(&s),
        Some(AllowedTools::Many(v)) => v
            .into_iter()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect(),
    };
    Ok((
        Frontmatter {
            name,
            description,
            license: raw.license,
            compatibility,
            metadata,
            allowed_tools,
            disable_model_invocation: raw.disable_model_invocation,
            warnings,
        },
        body,
    ))
}

/// Split on whitespace and commas outside parentheses, and after the `)`
/// that closes them, so `Bash(git status:*) Read, Grep` and
/// `Bash(git:*)Read` are entries, not fragments.
fn split_tool_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
                if depth == 0 {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c if depth == 0 && (c.is_whitespace() || c == ',') => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
