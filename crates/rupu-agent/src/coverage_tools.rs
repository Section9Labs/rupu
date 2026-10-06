//! Tool trait wrappers for the 4 coverage harness tools.
//!
//! These tools are injected into the agent registry when a `concerns:` block
//! is present in the agent frontmatter. They delegate to the free functions in
//! `rupu_coverage::tools` and populate `Attribution` from the `ToolContext`.

use async_trait::async_trait;
use rupu_coverage::tools::{verify_finding, VerifyError, VerifyInput};
use rupu_coverage::{
    asset_mark, coverage_concerns_detail, coverage_concerns_search, coverage_mark,
    coverage_remaining, coverage_status, report_finding, AssetMarkInput, Attribution,
    CoverageConcernsDetailInput, CoverageConcernsSearchInput, CoverageMarkInput, CoveragePaths,
    CoverageRemainingInput, CoverageStatusInput, FlatCatalog, ReportFindingInput, Surface,
    VerificationStatus,
};
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

fn attribution_from_ctx(ctx: &ToolContext) -> Attribution {
    let surface = match ctx.surface_tag.as_deref() {
        Some("agent") => Surface::Agent,
        Some("autoflow") => Surface::Autoflow,
        Some("session") => Surface::Session,
        _ => Surface::Workflow,
    };
    Attribution {
        run_id: ctx.run_id.clone().unwrap_or_default(),
        model: ctx.model.clone().unwrap_or_default(),
        surface,
        codename: ctx.codename.clone(),
        agent: ctx.agent.clone(),
        provider: ctx.provider.clone(),
    }
}

fn ok_output(text: impl Into<String>, elapsed: Instant) -> ToolOutput {
    ToolOutput {
        stdout: text.into(),
        error: None,
        duration_ms: elapsed.elapsed().as_millis() as u64,
        derived: None,
        structured: None,
    }
}

fn err_output(text: impl Into<String>, elapsed: Instant) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(text.into()),
        duration_ms: elapsed.elapsed().as_millis() as u64,
        derived: None,
        structured: None,
    }
}

// ---------------------------------------------------------------------------
// coverage_mark
// ---------------------------------------------------------------------------

pub struct CoverageMarkTool {
    paths: CoveragePaths,
    catalog: Arc<FlatCatalog>,
}

#[async_trait]
impl Tool for CoverageMarkTool {
    fn name(&self) -> &'static str {
        "coverage_mark"
    }

    fn description(&self) -> &'static str {
        "Record a coverage assertion for a (concern_id, file_path) pair. \
         Status must be one of: clean | finding | not_applicable. \
         The file must have been read at the required min_strength first, \
         unless status is not_applicable."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["concern_id", "file_path", "status", "evidence"],
            "properties": {
                "concern_id": {
                    "type": "string",
                    "description": "Concern ID from the effective catalog (e.g. stride:spoofing)."
                },
                "file_path": {
                    "type": "string",
                    "description": "Workspace-relative path of the file being marked."
                },
                "status": {
                    "type": "string",
                    "enum": ["clean", "finding", "not_applicable"],
                    "description": "Coverage assertion result."
                },
                "evidence": {
                    "type": "object",
                    "required": ["summary"],
                    "properties": {
                        "summary": { "type": "string" },
                        "line_ranges": {
                            "type": "array",
                            "items": {
                                "type": "array",
                                "items": { "type": "integer" },
                                "minItems": 2,
                                "maxItems": 2
                            }
                        },
                        "finding_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            }
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageMarkInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let attribution = attribution_from_ctx(ctx);
        match coverage_mark(&self.paths, &self.catalog, attribution, parsed).await {
            Ok(out) => {
                let mut text = if out.warnings.is_empty() {
                    "ok".to_string()
                } else {
                    format!("ok (warnings: {})", out.warnings.join("; "))
                };
                if !out.warnings.is_empty() {
                    text = format!("ok\nwarnings:\n{}", out.warnings.join("\n"));
                }
                Ok(ok_output(text, started))
            }
            Err(e) => Ok(err_output(e.to_string(), started)),
        }
    }
}

// ---------------------------------------------------------------------------
// coverage_status
// ---------------------------------------------------------------------------

pub struct CoverageStatusTool {
    paths: CoveragePaths,
}

#[async_trait]
impl Tool for CoverageStatusTool {
    fn name(&self) -> &'static str {
        "coverage_status"
    }

    fn description(&self) -> &'static str {
        "Query existing coverage assertions. Optionally filter by concern_id, \
         file_path_prefix, or since timestamp. Returns a JSON array of assertion records."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "concern_id": {
                    "type": "string",
                    "description": "Filter to this concern ID only."
                },
                "file_path_prefix": {
                    "type": "string",
                    "description": "Return only assertions whose file_path starts with this prefix."
                },
                "since": {
                    "type": "string",
                    "format": "date-time",
                    "description": "ISO-8601 timestamp. Return only assertions after this point."
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageStatusInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        match coverage_status(&self.paths, parsed) {
            Ok(assertions) => {
                let text =
                    serde_json::to_string_pretty(&assertions).unwrap_or_else(|_| "[]".to_string());
                Ok(ok_output(text, started))
            }
            Err(e) => Ok(err_output(e.to_string(), started)),
        }
    }
}

// ---------------------------------------------------------------------------
// coverage_remaining
// ---------------------------------------------------------------------------

pub struct CoverageRemainingTool {
    paths: CoveragePaths,
    catalog: Arc<FlatCatalog>,
}

#[async_trait]
impl Tool for CoverageRemainingTool {
    fn name(&self) -> &'static str {
        "coverage_remaining"
    }

    fn description(&self) -> &'static str {
        "List (concern_id, file_path) pairs that have been touched but not yet \
         asserted. Optionally filter by concern_id or min_strength. \
         Use this to discover what still needs coverage_mark calls."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "concern_id": {
                    "type": "string",
                    "description": "Filter to this concern ID only."
                },
                "min_strength": {
                    "type": "string",
                    "enum": ["glob", "cmd", "grep", "read", "edit"],
                    "description": "Minimum touch strength to include."
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageRemainingInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        match coverage_remaining(&self.paths, &self.catalog, parsed) {
            Ok(items) => {
                let text =
                    serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string());
                Ok(ok_output(text, started))
            }
            Err(e) => Ok(err_output(e.to_string(), started)),
        }
    }
}

// ---------------------------------------------------------------------------
// report_finding
// ---------------------------------------------------------------------------

pub struct ReportFindingTool {
    paths: CoveragePaths,
    options: rupu_coverage::FindingWriteOptions,
}

impl ReportFindingTool {
    /// Build the tool against an explicit ledger location and the run's
    /// findings options (profile, artifact store, limits).
    ///
    /// `register` below wires this up as part of the coverage harness. This
    /// constructor exists for the other caller: an agent that records
    /// findings WITHOUT a `concerns:` block (see `runner`'s
    /// findings-without-coverage registration). A campaign that assesses
    /// hosts rather than files has no catalog and no concerns, but still
    /// produces findings.
    pub fn new(paths: CoveragePaths, options: rupu_coverage::FindingWriteOptions) -> Self {
        Self { paths, options }
    }
}

#[async_trait]
impl Tool for ReportFindingTool {
    fn name(&self) -> &'static str {
        "report_finding"
    }

    fn description(&self) -> &'static str {
        "Record a security or quality finding in this project's ledger. Returns the \
         generated finding id (use it in coverage_mark calls and in another finding's \
         cross_references). Under the full profile send a complete `report`; a rejected \
         call lists every problem to fix."
    }

    fn input_schema(&self) -> Value {
        match self.options.profile {
            rupu_coverage::FindingProfile::Summary => summary_schema(),
            rupu_coverage::FindingProfile::Full => full_schema(),
        }
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        // `serde_path_to_error` so a structural error names where it is
        // (`report.call_chain[0].role: unknown variant ...`) instead of a
        // bare message the agent has to hunt for in a large report.
        let parsed: ReportFindingInput = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let attribution = attribution_from_ctx(ctx);
        // The write is synchronous and can be long (hashing and copying
        // artifacts up to the configured caps), so it runs on the blocking
        // pool rather than stalling this runtime worker.
        let paths = self.paths.clone();
        let options = self.options.clone();
        let result = tokio::task::spawn_blocking(move || {
            report_finding(&paths, attribution, parsed, &options)
        })
        .await;
        match result {
            Ok(Ok(out)) => Ok(ok_output(format!("finding_id: {}", out.id), started)),
            Ok(Err(e)) => Ok(err_output(e.to_string(), started)),
            Err(join) => Ok(err_output(
                format!("report_finding did not complete: {join}"),
                started,
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// finding.verify
// ---------------------------------------------------------------------------

/// Record an independent verdict on a finding another run filed.
///
/// The verifier's run id and agent name come from the `ToolContext` the
/// runner fills in, NEVER from the tool input: a model that could name its
/// own `by_run` could claim to be any run, which would defeat the
/// self-verification refusal (`verify_finding` compares `by_run` to the run
/// that filed the finding) and the evaluator's independence check. A call
/// from a context with no run id is refused rather than recorded blank.
pub struct FindingVerifyTool {
    paths: CoveragePaths,
}

impl FindingVerifyTool {
    /// Build the tool against an explicit ledger location. `register` wires
    /// it into the coverage harness; `runner` registers it for an agent that
    /// lists `finding.verify` in `tools:` and has no `concerns:` block.
    pub fn new(paths: CoveragePaths) -> Self {
        Self { paths }
    }
}

#[async_trait]
impl Tool for FindingVerifyTool {
    fn name(&self) -> &'static str {
        "finding.verify"
    }

    fn description(&self) -> &'static str {
        "Record your verdict on a finding that ANOTHER run filed: confirmed (you \
         reproduced or independently established it), disputed (you showed it is wrong), \
         or inconclusive (you could not decide). Your run and agent are recorded \
         automatically. A finding cannot be verified by the run that filed it, and a \
         finding without a full report cannot be verified. A later verdict replaces an \
         earlier one."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["finding_id", "status"],
            "properties": {
                "finding_id": {
                    "type": "string",
                    "description": "The id of the finding to verify (as returned by report_finding)."
                },
                "status": {
                    "type": "string",
                    "enum": ["confirmed", "disputed", "inconclusive"],
                    "description": "Your verdict."
                },
                "notes": {
                    "type": "string",
                    "description": "What you did and saw: how you reproduced it, or why it does not hold."
                }
            }
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let finding_id = input
            .get("finding_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ToolError::InvalidInput("finding_id is required (a non-empty string)".into())
            })?
            .to_string();
        let status = match input.get("status").and_then(Value::as_str) {
            Some("confirmed") => VerificationStatus::Confirmed,
            Some("disputed") => VerificationStatus::Disputed,
            Some("inconclusive") => VerificationStatus::Inconclusive,
            Some(other) => {
                return Err(ToolError::InvalidInput(format!(
                    "status `{other}` is not a verdict; use confirmed, disputed or inconclusive"
                )))
            }
            None => {
                return Err(ToolError::InvalidInput(
                    "status is required (confirmed, disputed or inconclusive)".into(),
                ))
            }
        };
        let notes = match input.get("notes") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(ToolError::InvalidInput(
                    "notes must be a string when given".into(),
                ))
            }
        };
        // The verifier's identity is the run executing this call. Any
        // `by_run` / `by_agent` the model put in `input` was not read above
        // and is never consulted.
        let by_run = ctx
            .run_id
            .clone()
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| {
                ToolError::Execution(
                    "finding.verify needs the calling run's id, and this context has none; \
                     no verification was recorded"
                        .into(),
                )
            })?;
        let by_agent = ctx.agent.clone().filter(|a| !a.trim().is_empty());
        let verify = VerifyInput {
            finding_id: finding_id.clone(),
            status,
            by_run,
            by_agent,
            notes,
        };
        // A locked rewrite of the findings ledger: synchronous file I/O, so
        // it runs on the blocking pool like `report_finding`.
        let paths = self.paths.clone();
        let result = tokio::task::spawn_blocking(move || verify_finding(&paths, &verify))
            .await
            .map_err(|join| {
                ToolError::Execution(format!("finding.verify did not complete: {join}"))
            })?;
        match result {
            Ok(()) => Ok(ok_output(
                format!(
                    "verification recorded: {finding_id} is {}",
                    status_word(status)
                ),
                started,
            )),
            // A refusal the agent can read and act on.
            Err(
                e @ (VerifyError::NotFound
                | VerifyError::Ambiguous
                | VerifyError::NoReport
                | VerifyError::SelfVerification
                | VerifyError::BadStatus
                | VerifyError::MissingVerifier),
            ) => Ok(err_output(e.to_string(), started)),
            // The ledger could not be locked or replaced: not the agent's
            // doing, and nothing was recorded.
            Err(VerifyError::Io(e)) => Err(ToolError::Io(e)),
        }
    }
}

fn status_word(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Confirmed => "confirmed",
        VerificationStatus::Disputed => "disputed",
        VerificationStatus::Inconclusive => "inconclusive",
        VerificationStatus::Unverified => "unverified",
    }
}

/// The lightweight record: the original schema, verbatim.
fn summary_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["scope", "summary", "severity", "evidence"],
        "properties": {
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
            "asset": {
                "type": "object",
                "required": ["kind"],
                "description": "The engagement asset this finding is about (only under an active engagement profile). Its `kind` routes the finding to the owning profile for completeness validation and stamps it into the asset graph. Omit on a plain code run.",
                "properties": {
                    "kind": { "type": "string", "description": "Profile-namespaced asset kind, e.g. \"network:service\", \"binary:function\", \"web:route\"." },
                    "coordinates": {
                        "type": "array",
                        "description": "Locator coordinates pinning the asset, each {\"t\": <tag>, \"v\": <value>}. Tags: host, port, url, path, line_range, symbol, sha256, address, offset, commit, http_route, param, resource_id.",
                        "items": { "type": "object" }
                    },
                    "label": { "type": "string", "description": "Optional human label; if omitted, the asset keeps its existing label (a new asset is labelled with its kind)." }
                }
            }
        }
    })
}

/// The full profile: locators + a complete `report`. `summary`, `severity`
/// and `evidence` are derived from the report, so they are not offered.
fn full_schema() -> Value {
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

// ---------------------------------------------------------------------------
// coverage_concerns_search
// ---------------------------------------------------------------------------

pub struct CoverageConcernsSearchTool {
    catalog: Arc<FlatCatalog>,
}

#[async_trait]
impl Tool for CoverageConcernsSearchTool {
    fn name(&self) -> &'static str {
        "coverage_concerns_search"
    }

    fn description(&self) -> &'static str {
        "Search the concern catalog by substring and/or filter. Returns up to `limit` \
matching concerns in summary form (id, name, severity, summary) by default, or full \
form if `form: 'full'`. Use when the catalog is too large to inline in the prompt."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Case-insensitive substring match against id, name, description."
                },
                "filter": {
                    "type": "object",
                    "properties": {
                        "severity": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["info", "low", "medium", "high", "critical"]
                            }
                        },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "ids": { "type": "array", "items": { "type": "string" } },
                        "applicable_to_path": { "type": "string" }
                    }
                },
                "limit": { "type": "integer", "default": 20 },
                "form": {
                    "type": "string",
                    "enum": ["summary", "full"],
                    "default": "summary"
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageConcernsSearchInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let results = coverage_concerns_search(&self.catalog, parsed);
        let text = serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
        Ok(ok_output(text, started))
    }
}

// ---------------------------------------------------------------------------
// coverage_concerns_detail
// ---------------------------------------------------------------------------

pub struct CoverageConcernsDetailTool {
    catalog: Arc<FlatCatalog>,
}

#[async_trait]
impl Tool for CoverageConcernsDetailTool {
    fn name(&self) -> &'static str {
        "coverage_concerns_detail"
    }

    fn description(&self) -> &'static str {
        "Fetch full concern records by id. Use after coverage_concerns_search finds a \
relevant concern and you need its full description, applicable_globs, or references."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["concern_ids"],
            "properties": {
                "concern_ids": { "type": "array", "items": { "type": "string" } }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageConcernsDetailInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let out = coverage_concerns_detail(&self.catalog, parsed);
        let text = serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_string());
        Ok(ok_output(text, started))
    }
}

// ---------------------------------------------------------------------------
// asset_mark (only registered under an active engagement profile)
// ---------------------------------------------------------------------------

pub struct AssetMarkTool {
    paths: CoveragePaths,
    engagement: Arc<rupu_coverage::ActiveSet>,
}

impl AssetMarkTool {
    pub fn new(paths: CoveragePaths, engagement: Arc<rupu_coverage::ActiveSet>) -> Self {
        Self { paths, engagement }
    }
}

#[async_trait]
impl Tool for AssetMarkTool {
    fn name(&self) -> &'static str {
        "asset_mark"
    }

    fn description(&self) -> &'static str {
        "Record how deeply an engagement asset has been examined, as a rung of \
         its profile's coverage depth ladder (monotonic — a shallower rung after \
         a deeper one keeps the deeper one). The effective rung is returned."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["kind", "depth"],
            "properties": {
                "kind": { "type": "string", "description": "Profile-namespaced asset kind, e.g. \"network:service\"." },
                "coordinates": {
                    "type": "array",
                    "description": "Locator coordinates pinning the asset, each {\"t\": <tag>, \"v\": <value>}.",
                    "items": { "type": "object" }
                },
                "depth": { "type": "string", "description": "The depth-ladder rung reached, e.g. \"tested\"." },
                "label": { "type": "string" }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: AssetMarkInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let paths = self.paths.clone();
        let engagement = self.engagement.clone();
        let res = tokio::task::spawn_blocking(move || asset_mark(&paths, parsed, &engagement))
            .await
            .map_err(|join| ToolError::Execution(format!("asset_mark did not complete: {join}")))?;
        match res {
            Ok(out) => Ok(ok_output(
                format!("asset {} is at depth `{}`", out.id, out.effective_depth),
                started,
            )),
            Err(e) => Ok(err_output(e.to_string(), started)),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the coverage tools into the provided registry. `asset_mark` is
/// registered only when the run has an active engagement profile (its findings
/// options carry one); otherwise the tool would have nothing to validate
/// against, so it is not offered.
pub fn register(
    registry: &mut crate::tool_registry::ToolRegistry,
    catalog: FlatCatalog,
    paths: CoveragePaths,
    findings: rupu_coverage::FindingWriteOptions,
) {
    let catalog = Arc::new(catalog);
    if let Some(engagement) = findings.engagement.clone() {
        registry.insert(
            "asset_mark",
            Arc::new(AssetMarkTool {
                paths: paths.clone(),
                engagement,
            }),
        );
    }
    registry.insert(
        "coverage_mark",
        Arc::new(CoverageMarkTool {
            paths: paths.clone(),
            catalog: catalog.clone(),
        }),
    );
    registry.insert(
        "coverage_status",
        Arc::new(CoverageStatusTool {
            paths: paths.clone(),
        }),
    );
    registry.insert(
        "coverage_remaining",
        Arc::new(CoverageRemainingTool {
            paths: paths.clone(),
            catalog: catalog.clone(),
        }),
    );
    registry.insert(
        "finding.verify",
        Arc::new(FindingVerifyTool::new(paths.clone())),
    );
    registry.insert(
        "report_finding",
        Arc::new(ReportFindingTool::new(paths, findings)),
    );
    registry.insert(
        "coverage_concerns_search",
        Arc::new(CoverageConcernsSearchTool {
            catalog: catalog.clone(),
        }),
    );
    registry.insert(
        "coverage_concerns_detail",
        Arc::new(CoverageConcernsDetailTool { catalog }),
    );
}

#[cfg(test)]
mod findings_profile_tests {
    use super::*;
    use rupu_coverage::{FindingProfile, FindingWriteOptions};

    fn tool(profile: FindingProfile) -> ReportFindingTool {
        let tmp = std::env::temp_dir();
        ReportFindingTool::new(
            CoveragePaths::new(&tmp, "t"),
            FindingWriteOptions::default().with_profile(profile),
        )
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
        let tool = ReportFindingTool::new(
            CoveragePaths::new(tmp.path(), "t"),
            FindingWriteOptions::default(),
        );
        let mut report: Value = serde_json::from_str(include_str!(
            "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        report["call_chain"][0]["role"] = serde_json::json!("entrypoint");
        let err = tool
            .invoke(
                serde_json::json!({ "scope": "repo", "report": report.clone() }),
                &ToolContext::default(),
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
                &ToolContext::default(),
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

#[cfg(test)]
mod finding_verify_tests {
    use super::*;
    use rupu_coverage::{
        read_findings, FindingProfile, FindingReport, FindingScope, FindingWriteOptions,
    };

    /// File a full-report finding as `run_id` (the same write path
    /// `report_finding` uses) and return its id.
    fn seed_full(paths: &CoveragePaths, run_id: &str) -> String {
        let report: FindingReport = serde_json::from_str(include_str!(
            "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        let input = ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: None,
            severity: None,
            concern_id: None,
            evidence: None,
            report: Some(report),
            asset: None,
        };
        let attribution = Attribution {
            run_id: run_id.into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        };
        let opts = FindingWriteOptions::default().with_profile(FindingProfile::Full);
        report_finding(paths, attribution, input, &opts).unwrap().id
    }

    fn ctx(run_id: Option<&str>, agent: Option<&str>) -> ToolContext {
        ToolContext {
            run_id: run_id.map(str::to_string),
            agent: agent.map(str::to_string),
            ..ToolContext::default()
        }
    }

    fn verification_of(paths: &CoveragePaths, id: &str) -> Option<rupu_coverage::Verification> {
        read_findings(paths)
            .unwrap()
            .into_iter()
            .find(|f| f.id == id)
            .unwrap()
            .report
            .unwrap()
            .verification
    }

    #[test]
    fn schema_requires_id_and_status_and_offers_only_verdicts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tool = FindingVerifyTool::new(CoveragePaths::new(tmp.path(), "t"));
        assert_eq!(tool.name(), "finding.verify");
        let s = tool.input_schema();
        let req: Vec<&str> = s["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(req, ["finding_id", "status"]);
        let verdicts: Vec<&str> = s["properties"]["status"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(verdicts, ["confirmed", "disputed", "inconclusive"]);
        assert!(s["properties"]["notes"].is_object());
        // The verifier's identity is never an input.
        assert!(s["properties"].get("by_run").is_none());
        assert!(s["properties"].get("by_agent").is_none());
    }

    #[tokio::test]
    async fn confirmed_records_the_verdict_with_the_callers_run_and_agent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_A");
        let tool = FindingVerifyTool::new(paths.clone());

        let out = tool
            .invoke(
                serde_json::json!({
                    "finding_id": id,
                    "status": "confirmed",
                    "notes": "reproduced with the PoC",
                }),
                &ctx(Some("run_B"), Some("exploit-verifier")),
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{:?}", out.error);
        assert!(out.stdout.contains(&id), "{}", out.stdout);

        let v = verification_of(&paths, &id).expect("verification recorded");
        assert_eq!(v.status, VerificationStatus::Confirmed);
        assert_eq!(v.by_run.as_deref(), Some("run_B"));
        assert_eq!(v.by_agent.as_deref(), Some("exploit-verifier"));
        assert_eq!(v.notes.as_deref(), Some("reproduced with the PoC"));
    }

    #[tokio::test]
    async fn a_by_run_smuggled_in_the_input_is_ignored() {
        // The model tries to pose as another run. The identity recorded is
        // the context's, and the context's run is judged for independence.
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_A");
        let tool = FindingVerifyTool::new(paths.clone());

        // Posing as someone else while really being the filing run: refused.
        let out = tool
            .invoke(
                serde_json::json!({
                    "finding_id": id, "status": "confirmed",
                    "by_run": "run_Z", "by_agent": "someone-else",
                }),
                &ctx(Some("run_A"), Some("reporter")),
            )
            .await
            .unwrap();
        assert!(out.error.is_some(), "self-verification must be refused");
        assert!(verification_of(&paths, &id).is_none());

        // Posing as the filing run while really being run_B: recorded as run_B.
        let out = tool
            .invoke(
                serde_json::json!({
                    "finding_id": id, "status": "disputed",
                    "by_run": "run_A", "by_agent": "someone-else",
                }),
                &ctx(Some("run_B"), Some("exploit-verifier")),
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{:?}", out.error);
        let v = verification_of(&paths, &id).unwrap();
        assert_eq!(v.status, VerificationStatus::Disputed);
        assert_eq!(v.by_run.as_deref(), Some("run_B"));
        assert_eq!(v.by_agent.as_deref(), Some("exploit-verifier"));
    }

    #[tokio::test]
    async fn no_run_id_in_the_context_is_refused_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_A");
        let before = std::fs::read(&paths.findings).unwrap();
        let tool = FindingVerifyTool::new(paths.clone());

        for run_id in [None, Some(""), Some("   ")] {
            let err = tool
                .invoke(
                    serde_json::json!({ "finding_id": id, "status": "confirmed" }),
                    &ctx(run_id, Some("exploit-verifier")),
                )
                .await
                .expect_err("a call with no run id must fail");
            assert!(matches!(err, ToolError::Execution(_)), "{err:?}");
        }
        assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
        assert!(verification_of(&paths, &id).is_none());
    }

    #[tokio::test]
    async fn verifying_a_finding_the_caller_filed_is_a_refusal_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_B");
        let before = std::fs::read(&paths.findings).unwrap();
        let tool = FindingVerifyTool::new(paths.clone());

        let out = tool
            .invoke(
                serde_json::json!({ "finding_id": id, "status": "confirmed" }),
                &ctx(Some("run_B"), Some("exploit-verifier")),
            )
            .await
            .unwrap();
        let err = out.error.expect("self-verification is a refusal");
        assert!(err.contains("filed it"), "{err}");
        assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    }

    #[tokio::test]
    async fn an_unknown_finding_is_a_refusal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        seed_full(&paths, "run_A");
        let tool = FindingVerifyTool::new(paths);

        let out = tool
            .invoke(
                serde_json::json!({ "finding_id": "f_nope", "status": "confirmed" }),
                &ctx(Some("run_B"), None),
            )
            .await
            .unwrap();
        assert!(out.error.unwrap().contains("no finding"));
    }

    #[tokio::test]
    async fn bad_input_is_invalid_input_not_a_write() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_A");
        let before = std::fs::read(&paths.findings).unwrap();
        let tool = FindingVerifyTool::new(paths.clone());
        let c = ctx(Some("run_B"), Some("exploit-verifier"));

        for bad in [
            // `unverified` is the absence of a verdict, not a verdict.
            serde_json::json!({ "finding_id": id, "status": "unverified" }),
            serde_json::json!({ "finding_id": id, "status": "Confirmed" }),
            serde_json::json!({ "finding_id": id }),
            serde_json::json!({ "status": "confirmed" }),
            serde_json::json!({ "finding_id": "", "status": "confirmed" }),
            serde_json::json!({ "finding_id": "  ", "status": "confirmed" }),
            serde_json::json!({ "finding_id": id, "status": "confirmed", "notes": 7 }),
        ] {
            let err = tool
                .invoke(bad.clone(), &c)
                .await
                .expect_err(&format!("{bad} must be invalid"));
            assert!(matches!(err, ToolError::InvalidInput(_)), "{bad}: {err:?}");
        }
        assert_eq!(std::fs::read(&paths.findings).unwrap(), before);
    }

    #[test]
    fn the_coverage_bundle_registers_the_tool_beside_report_finding() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut registry = crate::tool_registry::ToolRegistry::new();
        register(
            &mut registry,
            FlatCatalog {
                concerns: Vec::new(),
                sources: Default::default(),
                render_modes: Default::default(),
            },
            CoveragePaths::new(tmp.path(), "t"),
            FindingWriteOptions::default(),
        );
        let names = registry.known_tools();
        assert!(names.iter().any(|n| n == "finding.verify"), "{names:?}");
        assert!(names.iter().any(|n| n == "report_finding"), "{names:?}");
    }
}
