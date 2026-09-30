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
    /// Profile + artifact store + limits. For an `action:` step this is the
    /// workflow `defaults.findings_profile` (the dispatcher is built once per
    /// run); a step-level `findings_profile` on an action step is a parse error.
    pub options: rupu_coverage::FindingWriteOptions,
}

pub fn specs() -> Vec<ToolSpec> {
    let mut input_schema = json!({
        "type": "object",
        "required": ["scope"],
        "properties": {
            "scope": {
                "type": "string",
                "enum": ["line", "file", "repo", "host", "endpoint", "resource"],
                "description": "What the finding is about. Code scopes: 'line' (needs file_path + line_range), 'file' (needs file_path), 'repo' (the project as a whole). Target scopes, each needing target_ref: 'host' (a machine or IP), 'endpoint' (a service URL), 'resource' (a cloud resource id such as an OCID/ARN/URN)."
            },
            "summary": { "type": "string", "description": "One sentence stating the weakness. Summary profile only." },
            "severity": {
                "type": "string",
                "enum": ["info", "low", "medium", "high", "critical"],
                "description": "Summary profile only."
            },
            "rationale": {
                "type": "string",
                "description": "Why this is a weakness, and the evidence that establishes it. Summary profile only."
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
            "code_excerpt": { "type": "string", "description": "Relevant excerpt, if any. Summary profile only." },
            "references": {
                "type": "array", "items": { "type": "string" },
                "description": "Supporting links — an issue URL, an advisory. Summary profile only."
            },
            "concern_id": { "type": "string", "description": "Catalog concern id, when one applies." }
        }
    });
    // `json!` cannot embed a function call as a value inside the literal, so
    // the report schema is attached after construction.
    input_schema["properties"]["report"] = rupu_coverage::report::schema::advertised_schema();
    vec![ToolSpec {
        name: "findings.record",
        description: "Record a security finding in this project's findings ledger, so it appears \
                      in the control plane rather than only in an external tracker. Use the \
                      narrowest scope the evidence supports. Under the run's full findings \
                      profile send `report` (a complete finding report) and omit \
                      summary/severity/rationale; under the summary profile send summary, \
                      severity and rationale.",
        input_schema,
        kind: ToolKind::Write,
    }]
}

#[derive(Debug, Deserialize)]
pub struct RecordArgs {
    pub scope: rupu_coverage::FindingScope,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub severity: Option<rupu_coverage::Severity>,
    #[serde(default)]
    pub rationale: Option<String>,
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
    #[serde(default)]
    pub report: Option<rupu_coverage::FindingReport>,
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
    // Under the full profile the excerpt and references belong inside
    // `report`. `report_finding` refuses summary/severity/evidence there, but
    // `code_excerpt`/`references` are folded into `evidence` only when a
    // `rationale` is present — sent alone they would be dropped silently. Catch
    // that residual case; everything else is left to the shared check.
    if ctx.options.profile == rupu_coverage::FindingProfile::Full
        && args.summary.is_none()
        && args.severity.is_none()
        && args.rationale.is_none()
        && (args.code_excerpt.is_some() || !args.references.is_empty())
    {
        return Err(
            "`code_excerpt` and `references` are not accepted under the full findings profile; \
             put them in `report.evidence` / `report.references` instead"
                .to_string(),
        );
    }
    // A missing `rationale` leaves `evidence` unset, which the summary profile
    // reports as a missing `evidence` (renamed by `record_error` to the field
    // name this tool exposes).
    let evidence = args
        .rationale
        .map(|rationale| rupu_coverage::FindingEvidence {
            code_excerpt: args.code_excerpt,
            rationale,
            references: args.references,
        });
    let input = rupu_coverage::ReportFindingInput {
        file_path: args.file_path,
        line_range: args.line_range,
        target_ref: args.target_ref,
        scope: args.scope,
        summary: args.summary,
        severity: args.severity,
        concern_id: args.concern_id,
        evidence,
        report: args.report,
    };
    // Locator, profile and report validation live in `report_finding` so both
    // the agent builtin and this tool enforce the same rule. Two paths
    // agreeing about a contract only stays true when it is one path.
    rupu_coverage::report_finding(&paths, attribution, input, &ctx.options)
        .map(|out| out.id)
        .map_err(record_error)
}

/// Render a `report_finding` failure in this tool's vocabulary.
///
/// The ledger calls the summary-profile detail block `evidence`; this tool
/// exposes it as `rationale` (with `code_excerpt`/`references` alongside), and
/// names itself rather than the agent builtin. The mapping is by error variant
/// on purpose: rewriting the rendered string would also rewrite unrelated text
/// that happens to contain the word, such as an artifact path an author named
/// `evidence`.
fn record_error(e: rupu_coverage::ReportFindingError) -> String {
    use rupu_coverage::ReportFindingError as E;
    match e {
        E::MissingField("evidence") => {
            "`rationale` is required under the summary findings profile".to_string()
        }
        E::DerivedFieldsSupplied => "under the full findings profile `summary`, `severity` and \
                                     `rationale` are derived from `report`; omit them"
            .to_string(),
        E::ReportRequired => "this step records findings under the full profile: `report` is \
                              required (see the findings.record tool schema)"
            .to_string(),
        other => other.to_string(),
    }
}
