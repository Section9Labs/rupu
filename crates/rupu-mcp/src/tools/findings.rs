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
    /// Profile + artifact store + limits. The profile here is the run
    /// default (workflow `defaults.findings_profile`, else `full`); the
    /// dispatcher is built once per run, so an `action:` step's own
    /// `findings_profile` — and the engagement set it records under
    /// (`options.engagement`) — reach `findings.record` per call through
    /// `ToolDispatcher::call_with_findings`.
    pub options: rupu_coverage::FindingWriteOptions,
    /// The run's **crew** codename (e.g. `jade-reef`), stamped on
    /// `Attribution.codename`. Crew only, never a per-agent instance: one
    /// `FindingsContext` is built per workflow run, not per step, so it
    /// cannot know which agent instance made the call. `None` for callers
    /// with no codename.
    pub codename: Option<String>,
    /// Provider paired with `model`, stamped on `Attribution.provider`.
    /// There is no `agent`: an `action:` step is not an agent.
    pub provider: Option<String>,
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
            "concern_id": { "type": "string", "description": "Catalog concern id, when one applies." },
            "asset": {
                "type": "object",
                "required": ["kind", "locator"],
                "description": "The asset this finding is about. Only accepted when the step records under an engagement profile (`engagement_profiles` on the step or the workflow defaults): the kind routes the finding to the profile that owns it, and the asset is registered in the project's asset graph. Omit it for a plain code finding.",
                "properties": {
                    "kind": {
                        "type": "string",
                        "description": "Profile-namespaced asset kind, e.g. `binary:function`. It must be one the active engagement profiles declare."
                    },
                    "locator": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "object" },
                        "description": "The coordinates that identify the asset: a list of single-key objects, e.g. [{\"sha256\": \"<hex>\"}, {\"address\": 4198400}, {\"symbol\": \"main\"}]. Coordinates: path, line_range {start,end}, symbol, commit, sha256, offset, address, host, port {number,proto}, url, http_route {method,path}, param, resource_id {scheme,id}. Use the coordinates the kind declares."
                    },
                    "parent": {
                        "type": "string",
                        "description": "Id of this asset's parent asset, if it has one."
                    },
                    "label": {
                        "type": "string",
                        "description": "A human-readable name for the asset. Omit it to derive one from the kind's label template."
                    }
                }
            }
        }
    });
    // `json!` cannot embed a function call as a value inside the literal, so
    // the report schema is attached after construction.
    input_schema["properties"]["report"] = rupu_coverage::report::schema::advertised_schema();
    vec![ToolSpec {
        name: "findings.record",
        description: "Record a security finding in this project's findings ledger, so it appears \
                      in the control plane rather than only in an external tracker. Use the \
                      narrowest scope the evidence supports. Under the full findings profile \
                      (the step's `findings_profile`, else the workflow default, else full) \
                      send `report` (a complete finding report) and omit \
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
    /// The asset this finding is about (kind + typed locator). Meaningful
    /// only under an engagement profile, which routes the finding by the
    /// asset's kind; see [`dispatch_record`].
    #[serde(default)]
    pub asset: Option<rupu_coverage::AssetInput>,
}

/// Write the finding. Returns the new finding id.
pub fn dispatch_record(ctx: &FindingsContext, args: RecordArgs) -> Result<String, String> {
    let target = rupu_coverage::target_id(&ctx.workspace_path, &ctx.scope_name);
    let paths = rupu_coverage::CoveragePaths::new(&ctx.workspace_path, &target);
    let attribution = rupu_coverage::Attribution {
        run_id: ctx.run_id.clone(),
        model: ctx.model.clone(),
        surface: ctx.surface,
        codename: ctx.codename.clone(),
        agent: None,
        provider: ctx.provider.clone(),
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
    // `asset` only means something under an engagement: with none,
    // `report_finding` takes the native `code` path and IGNORES the asset (the
    // agent builtin gets away with that because it advertises `asset` only
    // when an engagement is active). This tool's schema is static and always
    // lists it, so an ignored asset would read as a recorded one. Refuse it
    // instead.
    if args.asset.is_some() && ctx.options.engagement.is_none() {
        return Err(
            "`asset` is only accepted when this step records under an engagement profile \
             (set `engagement_profiles` on the step or in the workflow `defaults`); without \
             one the finding is recorded under the native code rules and an asset would be \
             dropped"
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
        asset: args.asset,
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
