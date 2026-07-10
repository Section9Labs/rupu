//! Agent file format. `.md` with YAML frontmatter; body is the system
//! prompt.
//!
//! Compatibility: matches Okesu / Claude conventions (frontmatter
//! keys: `name`, `description`, `provider`, `model`, `tools`,
//! `maxTurns`, `permissionMode`). Unknown fields are rejected at parse
//! time so typos like `permision_mode` surface as errors.

use rupu_coverage::ConcernsBlock;
use rupu_providers::model_tier::{ContextWindow, ThinkingLevel};
use rupu_providers::types::{ContextManagement, OutputFormat, Speed};
use rupu_providers::AuthMode;
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
    auth: Option<AuthMode>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default, rename = "maxTurns")]
    max_turns: Option<u32>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
    /// Anthropic-specific opt-out for the canonical OAuth system-prompt
    /// prefix on this agent. `None` (default) leaves the prefix on for
    /// OAuth requests; `Some(false)` disables it. No effect when the
    /// resolved provider/auth is not Anthropic OAuth.
    #[serde(default, rename = "anthropicOauthPrefix")]
    anthropic_oauth_prefix: Option<bool>,
    /// Reasoning / thinking effort level. Accepts the canonical
    /// `auto|minimal|low|medium|high|max` plus aliases `adaptive`
    /// (= auto) and `xhigh` (= max). Each provider maps to its native
    /// shape — Anthropic emits `thinking.type: adaptive` for `auto`
    /// and `thinking.budget_tokens: <n>` for the rest; OpenAI / Copilot
    /// emit `reasoning.effort: <name>`; Gemini emits
    /// `generationConfig.thinkingConfig.thinkingBudget: <budget>`.
    #[serde(default)]
    effort: Option<ThinkingLevel>,
    /// Desired context-window tier. `default` or omitted picks the
    /// model's native window; `1m` (alias `1M`, `one_million`) opts
    /// into the 1M-token window. Anthropic Sonnet/Opus 4 honor this on
    /// the api-key path by adding the `context-1m-2025-08-07` beta;
    /// the OAuth path always includes that beta via the static CSV.
    /// Other providers ignore this for now.
    #[serde(default, rename = "contextWindow")]
    context_window: Option<ContextWindow>,
    /// Output-format hint. `text` (default) leaves the model free
    /// to choose; `json` constrains the response to parse as JSON.
    /// Anthropic emits as `output_config.format`; OpenAI emits as
    /// `response_format.type: "json_object"`. Other providers
    /// currently ignore this.
    #[serde(default, rename = "outputFormat")]
    output_format: Option<OutputFormat>,
    /// JSON Schema for Anthropic structured outputs. When present,
    /// `outputFormat: json` agents get a guaranteed schema-conforming
    /// response via Anthropic's `output_config.format = {type:
    /// "json_schema", schema: <this value>}`. Declared inline as a
    /// YAML mapping so an agent stays a single self-contained `.md`
    /// file; `serde_yaml` deserializes it straight into a JSON
    /// `serde_json::Value`. `None` (default) preserves today's
    /// prompt-driven-only `outputFormat: json` behavior — Anthropic
    /// mandates a real schema for `format`, so no schema means no
    /// `output_config.format` is emitted at all.
    #[serde(default, rename = "outputSchema")]
    output_schema: Option<serde_json::Value>,
    /// Anthropic-only soft cap on output tokens. The model
    /// self-paces toward this budget — distinct from `maxTurns`,
    /// which is a hard ceiling. Emitted as
    /// `output_config.task_budget`. Ignored by other providers.
    #[serde(default, rename = "anthropicTaskBudget")]
    anthropic_task_budget: Option<u32>,
    /// Anthropic-only auto context-management strategy. When set,
    /// the server transparently drops earlier `tool_use` /
    /// `tool_result` blocks if the conversation would otherwise
    /// overflow. Emitted as
    /// `context_management: { type: "tool_clearing" }`. Ignored by
    /// other providers.
    #[serde(default, rename = "anthropicContextManagement")]
    anthropic_context_management: Option<ContextManagement>,
    /// Anthropic-only fast-mode toggle. Account-gated; sending
    /// `fast` from an account without the feature returns 400.
    /// Emitted as the top-level `speed: "fast"` body field.
    /// Ignored by other providers.
    #[serde(default, rename = "anthropicSpeed")]
    anthropic_speed: Option<Speed>,
    /// Allowlist of agent names this agent is permitted to dispatch
    /// via the `dispatch_agent` / `dispatch_agents_parallel` tools.
    /// `None` = the agent doesn't dispatch any children (default for
    /// most agents). The dispatch tools fail at invocation if the
    /// requested agent isn't in this list. See
    /// `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.
    #[serde(default, rename = "dispatchableAgents")]
    dispatchable_agents: Option<Vec<String>>,
    /// Coverage concerns block. When present, the runner flattens the
    /// catalog, writes a snapshot to `.rupu/coverage/<target>/catalog.yaml`,
    /// injects the 4 coverage tools, and prepends the catalog to the
    /// system prompt.
    #[serde(default)]
    concerns: Option<ConcernsBlock>,
    /// Per-request output-token budget (`max_tokens` in the LLM request).
    /// `None` falls back to `runner::DEFAULT_MAX_TOKENS` (8192). Raise it for
    /// agents that emit large output (e.g. writing reports) — note extended
    /// thinking (`effort`) draws from this same budget.
    #[serde(default, rename = "maxTokens")]
    max_tokens: Option<u32>,
    /// Model context-window size in tokens. When set, enables proactive
    /// LLM context compaction: the runner summarises older turns before
    /// the next turn when the previous turn's input exceeded
    /// `compactAtPercent` of this value. Absent → compaction disabled.
    #[serde(default, rename = "contextWindowTokens")]
    context_window_tokens: Option<u32>,
    /// Percentage of `contextWindowTokens` at which to trigger compaction.
    /// Defaults to 80 when `contextWindowTokens` is set and this is
    /// omitted. Clamped to `[10, 95]`.
    #[serde(default, rename = "compactAtPercent")]
    compact_at_percent: Option<u8>,
}

/// Parsed agent file. The body of the markdown is the system prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub name: String,
    pub description: Option<String>,
    pub provider: Option<String>,
    pub auth: Option<AuthMode>,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    pub max_turns: Option<u32>,
    pub permission_mode: Option<String>,
    pub anthropic_oauth_prefix: Option<bool>,
    pub effort: Option<ThinkingLevel>,
    pub context_window: Option<ContextWindow>,
    pub output_format: Option<OutputFormat>,
    /// JSON Schema for Anthropic structured outputs. See the
    /// `outputSchema` frontmatter doc comment on `Frontmatter`.
    pub output_schema: Option<serde_json::Value>,
    pub anthropic_task_budget: Option<u32>,
    pub anthropic_context_management: Option<ContextManagement>,
    pub anthropic_speed: Option<Speed>,
    /// Per-agent allowlist of children this agent can dispatch via
    /// `dispatch_agent` / `dispatch_agents_parallel`.
    pub dispatchable_agents: Option<Vec<String>>,
    /// Coverage concerns block parsed from `concerns:` frontmatter.
    pub concerns: Option<ConcernsBlock>,
    /// Per-request output-token budget. `None` falls back to
    /// `runner::DEFAULT_MAX_TOKENS` (8192).
    pub max_tokens: Option<u32>,
    /// Model context-window size in tokens. When set, enables LLM context compaction.
    pub context_window_tokens: Option<u32>,
    /// Compact-at percentage threshold. See `compactAtPercent` frontmatter.
    pub compact_at_percent: Option<u8>,
    pub system_prompt: String,
    /// The full original file text (frontmatter + body) verbatim. Lets the CP
    /// render the definition source with syntax highlighting; agents are
    /// matched by parsed `name` (not file stem), so the source path is not
    /// cleanly recoverable downstream — keeping the raw text here is the clean
    /// home for it.
    pub raw: String,
}

impl AgentSpec {
    /// Parse a string containing the full agent file (frontmatter +
    /// body). The frontmatter must be delimited by `---` lines at the
    /// very start; everything after the second `---` is the body.
    pub fn parse(s: &str) -> Result<Self, AgentSpecParseError> {
        let raw = s.to_string();
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
            auth: fm.auth,
            model: fm.model,
            tools: fm.tools,
            max_turns: fm.max_turns,
            permission_mode: fm.permission_mode,
            anthropic_oauth_prefix: fm.anthropic_oauth_prefix,
            effort: fm.effort,
            context_window: fm.context_window,
            output_format: fm.output_format,
            output_schema: fm.output_schema,
            anthropic_task_budget: fm.anthropic_task_budget,
            anthropic_context_management: fm.anthropic_context_management,
            anthropic_speed: fm.anthropic_speed,
            dispatchable_agents: fm.dispatchable_agents,
            concerns: fm.concerns,
            max_tokens: fm.max_tokens,
            context_window_tokens: fm.context_window_tokens,
            compact_at_percent: fm.compact_at_percent,
            system_prompt: body.to_string(),
            raw,
        })
    }

    /// Read + parse an agent file from disk.
    pub fn parse_file(path: &std::path::Path) -> Result<Self, AgentSpecParseError> {
        let s = std::fs::read_to_string(path)?;
        Self::parse(&s)
    }
}

#[cfg(test)]
mod compaction_config_tests {
    use super::AgentSpec;

    #[test]
    fn parses_context_compaction_fields() {
        let src = "---
name: test
contextWindowTokens: 1000000
compactAtPercent: 75
---
You are a test agent.
";
        let spec = AgentSpec::parse(src).expect("parse ok");
        assert_eq!(spec.context_window_tokens, Some(1_000_000));
        assert_eq!(spec.compact_at_percent, Some(75));
    }

    #[test]
    fn compaction_fields_absent_yields_none() {
        let src = "---
name: test
---
You are a test agent.
";
        let spec = AgentSpec::parse(src).expect("parse ok");
        assert_eq!(spec.context_window_tokens, None);
        assert_eq!(spec.compact_at_percent, None);
    }

    #[test]
    fn parse_preserves_raw_file_text() {
        let src = "---
name: test
model: opus
---
You are a test agent.
";
        let spec = AgentSpec::parse(src).expect("parse ok");
        // `raw` holds the full original file text verbatim (frontmatter + body),
        // so the CP can show the definition source with syntax highlighting.
        assert_eq!(spec.raw, src);
    }
}
