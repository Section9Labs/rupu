//! `findings.report`: record a finding in the run's findings ledger. The one
//! implementation behind the agent loop, `action: findings.record` steps and
//! `rupu mcp serve` (spec W4 §3.2).

use crate::coverage_emit::attribution_from;
use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_coverage::{
    report_finding, FindingEvidence, FindingProfile, FindingWriteOptions, ReportFindingError,
    ReportFindingInput,
};
use serde_json::Value;
use std::time::Instant;

// was `report_finding`; `findings.record` was the MCP / `action:` name.
pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "findings.report",
    aliases: &[Alias::any("report_finding"), Alias::any("findings.record")],
    effect: Effect::Record,
    needs: &[Service::Findings],
    uses: &[],
    description: "Record a security or quality finding in this project's ledger. Returns the \
     generated finding id (use it in coverage.mark calls and in another finding's \
     cross_references). Under the full profile send a complete `report`; a rejected \
     call lists every problem to fix.",
    input_schema: wide_schema,
};

/// The flat keys an `action: findings.record` step sends in place of an
/// `evidence` object (the step `with:` vocabulary, docs/workflow-format.md).
const FLAT_EVIDENCE_KEYS: [&str; 3] = ["rationale", "code_excerpt", "references"];

/// The lightweight record: the summary profile's schema, verbatim.
pub fn summary_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["scope", "summary", "severity", "evidence"],
        "properties": {
            "tags": rupu_coverage::tags_schema_property(),
            "file_path": {
                "type": "string",
                "description": "Workspace-relative path of the affected file, if applicable."
            },
            "line_range": {
                "type": "array",
                "items": { "type": "integer" },
                "minItems": 2,
                "maxItems": 2,
                "description": "Line range [start, end] within the file, if applicable."
            },
            "target_ref": {
                "type": "string",
                "description": "What this finding is about when it is not a file: a host or IP (host), a URL (endpoint), or a cloud resource id such as an OCID/ARN/URN (resource). Required for those three scopes."
            },
            "scope": {
                "type": "string",
                "enum": ["line", "file", "repo", "host", "endpoint", "resource"],
                "description": "What the finding is about. Code scopes: 'line' (needs file_path + line_range), 'file' (needs file_path), 'repo' (the project as a whole). Target scopes, each needing target_ref: 'host' (a machine or IP), 'endpoint' (a specific service URL), 'resource' (a cloud resource by its own id). Pick the narrowest scope the evidence actually supports - claiming a whole host for a defect on one endpoint overstates it."
            },
            "summary": {
                "type": "string",
                "description": "One-sentence description of the finding."
            },
            "severity": {
                "type": "string",
                "enum": ["info", "low", "medium", "high", "critical"],
                "description": "Severity of the finding."
            },
            "concern_id": {
                "type": "string",
                "description": "Concern ID this finding relates to, if known."
            },
            "evidence": {
                "type": "object",
                "required": ["rationale"],
                "properties": {
                    "code_excerpt": { "type": "string" },
                    "rationale": { "type": "string" },
                    "references": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            },
            "asset": rupu_coverage::asset_schema_property()
        }
    })
}

/// The full profile: locators + a complete `report`. `summary`, `severity`
/// and `evidence` are derived from the report, so they are not offered.
pub fn full_schema() -> Value {
    let mut s = summary_schema();
    let props = s["properties"].as_object_mut().expect("object schema");
    props.remove("summary");
    props.remove("severity");
    props.remove("evidence");
    props.insert(
        "report".to_string(),
        rupu_coverage::report::schema::advertised_schema(),
    );
    s["required"] = serde_json::json!(["scope", "report"]);
    s
}

/// Every key the tool accepts, under either profile and in either evidence
/// form: the summary schema's properties, the full schema's `report`, and the
/// flat `rationale` / `code_excerpt` / `references` an `action:` step sends.
/// What `/api/tools` lists and an `action:` step's `with:` is checked
/// against; a run's model sees its own profile's schema
/// ([`FindingsReportTool::input_schema`]). Built from the two profile
/// schemas, so it can't drift from them.
pub fn wide_schema() -> Value {
    let mut s = summary_schema();
    let props = s["properties"].as_object_mut().expect("object schema");
    props.insert(
        "report".to_string(),
        rupu_coverage::report::schema::advertised_schema(),
    );
    props.insert(
        "rationale".to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Summary profile, flat form of `evidence.rationale`: why this is a weakness, and the evidence that establishes it."
        }),
    );
    props.insert(
        "code_excerpt".to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Summary profile, flat form of `evidence.code_excerpt`."
        }),
    );
    props.insert(
        "references".to_string(),
        serde_json::json!({
            "type": "array", "items": { "type": "string" },
            "description": "Summary profile, flat form of `evidence.references`: supporting links."
        }),
    );
    s["required"] = serde_json::json!(["scope"]);
    s
}

/// `findings.report` for one run: the run's findings options (profile,
/// artifact store, limits, engagement) fix its schema and its contract.
pub struct FindingsReportTool {
    options: FindingWriteOptions,
}

impl FindingsReportTool {
    pub fn new(options: FindingWriteOptions) -> Self {
        Self { options }
    }
}

#[async_trait]
impl Tool for FindingsReportTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    /// The run's own profile's schema: what its model may send.
    fn input_schema(&self) -> Value {
        match self.options.profile {
            FindingProfile::Summary => summary_schema(),
            FindingProfile::Full => full_schema(),
        }
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let (input, flat) = split_flat_evidence(input)?;
        // `serde_path_to_error` so a structural error names where it is
        // (`report.call_chain[0].role: unknown variant ...`) instead of a
        // bare message the caller has to hunt for in a large report.
        let mut parsed: ReportFindingInput = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if let Some(flat) = &flat {
            if let Err(msg) = flat.check(&parsed, self.options.profile) {
                return Ok(failed(msg).timed(started));
            }
        }
        if let Some(flat) = flat.as_ref().filter(|f| f.rationale.is_some()) {
            parsed.evidence = Some(FindingEvidence {
                code_excerpt: flat.code_excerpt.clone(),
                rationale: flat.rationale.clone().unwrap_or_default(),
                references: flat.references.clone(),
            });
        }
        let paths = crate::ledger::paths(ctx);
        let attribution = attribution_from(ctx);
        let options = self.options.clone();
        // The write is synchronous and can be long (hashing and copying
        // artifacts up to the configured caps), so it runs on the blocking
        // pool rather than stalling this runtime worker.
        let result = tokio::task::spawn_blocking(move || {
            report_finding(&paths, attribution, parsed, &options)
        })
        .await;
        match result {
            Ok(Ok(out)) => Ok(ok(format!("finding_id: {}", out.id)).timed(started)),
            Ok(Err(e)) => {
                let msg = match flat {
                    Some(_) => flat_vocabulary(e),
                    None => e.to_string(),
                };
                Ok(failed(msg).timed(started))
            }
            Err(join) => {
                Ok(failed(format!("findings.report did not complete: {join}")).timed(started))
            }
        }
    }
}

/// The flat evidence keys a call sent (`action:` step form).
struct FlatEvidence {
    rationale: Option<String>,
    code_excerpt: Option<String>,
    references: Vec<String>,
}

impl FlatEvidence {
    /// The flat form's own rules, on top of `report_finding`'s.
    fn check(&self, parsed: &ReportFindingInput, profile: FindingProfile) -> Result<(), String> {
        if parsed.evidence.is_some() {
            return Err(
                "send either `evidence` or the flat `rationale` / `code_excerpt` / \
                        `references`, not both"
                    .to_string(),
            );
        }
        // Under the full profile the excerpt and references belong inside
        // `report`. `report_finding` refuses summary/severity/evidence there,
        // but `code_excerpt` / `references` become `evidence` only alongside
        // a `rationale` — sent alone they would be dropped silently.
        if profile == FindingProfile::Full
            && parsed.summary.is_none()
            && parsed.severity.is_none()
            && self.rationale.is_none()
            && (self.code_excerpt.is_some() || !self.references.is_empty())
        {
            return Err(
                "`code_excerpt` and `references` are not accepted under the full findings \
                 profile; put them in `report.evidence` / `report.references` instead"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// Take the flat evidence keys out of `input`, leaving what
/// `ReportFindingInput` reads. `None` when the call sent none of them.
fn split_flat_evidence(mut input: Value) -> Result<(Value, Option<FlatEvidence>), ToolError> {
    let Some(obj) = input.as_object_mut() else {
        return Ok((input, None));
    };
    if !FLAT_EVIDENCE_KEYS.iter().any(|k| obj.contains_key(*k)) {
        return Ok((input, None));
    }
    let string = |v: Option<Value>, key: &str| match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(ToolError::InvalidInput(format!("{key}: expected a string"))),
    };
    let rationale = string(obj.remove("rationale"), "rationale")?;
    let code_excerpt = string(obj.remove("code_excerpt"), "code_excerpt")?;
    let references = match obj.remove("references") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => serde_json::from_value(v)
            .map_err(|e| ToolError::InvalidInput(format!("references: {e}")))?,
    };
    Ok((
        input,
        Some(FlatEvidence {
            rationale,
            code_excerpt,
            references,
        }),
    ))
}

/// A `report_finding` failure, in the flat form's vocabulary: the ledger
/// calls the summary-profile detail block `evidence`; a flat call sent it as
/// `rationale`. Mapped by variant, never by rewriting the rendered text (that
/// would also rewrite an artifact path an author named `evidence`).
fn flat_vocabulary(e: ReportFindingError) -> String {
    match e {
        ReportFindingError::MissingField("evidence") => {
            "`rationale` is required under the summary findings profile".to_string()
        }
        ReportFindingError::DerivedFieldsSupplied => {
            "under the full findings profile `summary`, `severity` and `rationale` are derived \
             from `report`; omit them"
                .to_string()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod findings_profile_tests {
    use super::*;

    fn tool(profile: FindingProfile) -> FindingsReportTool {
        FindingsReportTool::new(FindingWriteOptions::default().with_profile(profile))
    }

    #[test]
    fn full_profile_advertises_report_as_required() {
        let s = tool(FindingProfile::Full).input_schema();
        let req: Vec<&str> = s["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(req.contains(&"report"), "{req:?}");
        assert!(!req.contains(&"summary"));
        assert!(s["properties"]["report"]["properties"]["root_cause"].is_object());
        assert!(s["properties"].get("summary").is_none());
    }

    #[tokio::test]
    async fn structural_errors_name_the_field_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tool = FindingsReportTool::new(FindingWriteOptions::default());
        let ctx = ToolContext::in_workspace(tmp.path());
        let mut report: Value = serde_json::from_str(include_str!(
            "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        report["call_chain"][0]["role"] = serde_json::json!("entrypoint");
        let err = tool
            .invoke(
                serde_json::json!({ "scope": "repo", "report": report.clone() }),
                &ctx,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("report.call_chain[0].role"), "{err}");
        assert!(err.contains("unknown variant `entrypoint`"), "{err}");

        report["call_chain"][0]["role"] = serde_json::json!("source");
        report["rating"]["risk_ratng"] = serde_json::json!("High");
        let err = tool
            .invoke(
                serde_json::json!({ "scope": "repo", "report": report }),
                &ctx,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("report.rating"), "{err}");
        assert!(err.contains("risk_ratng"), "{err}");
    }

    #[test]
    fn summary_profile_advertises_the_lightweight_fields() {
        let s = tool(FindingProfile::Summary).input_schema();
        let req: Vec<&str> = s["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(req.contains(&"summary") && req.contains(&"severity") && req.contains(&"evidence"));
        assert!(s["properties"].get("report").is_none());
    }
}
