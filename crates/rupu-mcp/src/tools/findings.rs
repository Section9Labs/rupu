//! `findings.record` — declare a finding from a workflow `action:` step.
//!
//! The agent-side equivalent is the `report_finding` builtin
//! (`rupu-agent/src/coverage_tools.rs`), which an agent reaches directly.
//! An `action:` step is not an agent: it calls one MCP tool and has no
//! builtin registry, so without this it could observe something and have no
//! way to record it.
//!
//! Requires a [`FindingsContext`] on the dispatcher. When the context is
//! absent the tool is still listed but refuses the call, rather than
//! silently writing a finding into some default location — a finding filed
//! against the wrong project is worse than one that failed loudly.

use super::{ToolKind, ToolSpec};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

/// What the dispatcher needs to know in order to attribute and place a
/// finding: which workspace's ledger it belongs in, and which run declared
/// it.
#[derive(Debug, Clone)]
pub struct FindingsContext {
    pub workspace_path: PathBuf,
    /// Scope name the target id is derived from — the workflow or agent
    /// whose findings these are.
    pub scope_name: String,
    pub run_id: String,
    pub model: String,
    pub surface: rupu_coverage::Surface,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "findings.record",
        description: "Record a security finding in this project's findings ledger, so it appears \
                      in the control plane rather than only in an external tracker. Use the \
                      narrowest scope the evidence supports.",
        input_schema: json!({
            "type": "object",
            "required": ["scope", "summary", "severity", "rationale"],
            "properties": {
                "scope": {
                    "type": "string",
                    "enum": ["line", "file", "repo", "host", "endpoint", "resource"],
                    "description": "What the finding is about. Code scopes: 'line' (needs file_path + line_range), 'file' (needs file_path), 'repo' (the project as a whole). Target scopes, each needing target_ref: 'host' (a machine or IP), 'endpoint' (a service URL), 'resource' (a cloud resource id such as an OCID/ARN/URN)."
                },
                "summary": { "type": "string", "description": "One sentence stating the weakness." },
                "severity": {
                    "type": "string",
                    "enum": ["info", "low", "medium", "high", "critical"]
                },
                "rationale": {
                    "type": "string",
                    "description": "Why this is a weakness, and the evidence that establishes it."
                },
                "file_path": { "type": "string", "description": "Workspace-relative path, for the code scopes." },
                "line_range": {
                    "type": "array", "items": { "type": "integer" },
                    "minItems": 2, "maxItems": 2,
                    "description": "[start, end], required for scope 'line'."
                },
                "target_ref": {
                    "type": "string",
                    "description": "The host, endpoint URL, or resource id — required for the target scopes."
                },
                "code_excerpt": { "type": "string", "description": "Relevant excerpt, if any." },
                "references": {
                    "type": "array", "items": { "type": "string" },
                    "description": "Supporting links — an issue URL, an advisory."
                },
                "concern_id": { "type": "string", "description": "Catalog concern id, when one applies." }
            }
        }),
        kind: ToolKind::Write,
    }]
}

#[derive(Debug, Deserialize)]
pub struct RecordArgs {
    pub scope: rupu_coverage::FindingScope,
    pub summary: String,
    pub severity: rupu_coverage::Severity,
    pub rationale: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    #[serde(default)]
    pub target_ref: Option<String>,
    #[serde(default)]
    pub code_excerpt: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub concern_id: Option<String>,
}

/// Write the finding. Returns the new finding id.
pub fn dispatch_record(ctx: &FindingsContext, args: RecordArgs) -> Result<String, String> {
    let target = rupu_coverage::target_id(&ctx.workspace_path, &ctx.scope_name);
    let paths = rupu_coverage::CoveragePaths::new(&ctx.workspace_path, &target);
    let attribution = rupu_coverage::Attribution {
        run_id: ctx.run_id.clone(),
        model: ctx.model.clone(),
        surface: ctx.surface,
    };
    let input = rupu_coverage::ReportFindingInput {
        file_path: args.file_path,
        line_range: args.line_range,
        target_ref: args.target_ref,
        scope: args.scope,
        summary: args.summary,
        severity: args.severity,
        concern_id: args.concern_id,
        evidence: rupu_coverage::FindingEvidence {
            code_excerpt: args.code_excerpt,
            rationale: args.rationale,
            references: args.references,
        },
    };
    // Locator validation lives in `report_finding` so both the agent builtin
    // and this tool enforce the same rule. Two paths agreeing about a
    // contract only stays true when it is one path.
    rupu_coverage::report_finding(&paths, attribution, input)
        .map(|out| out.id)
        .map_err(|e| e.to_string())
}
