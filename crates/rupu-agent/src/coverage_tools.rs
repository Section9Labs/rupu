//! Tool trait wrappers for the 4 coverage harness tools.
//!
//! These tools are injected into the agent registry when a `concerns:` block
//! is present in the agent frontmatter. They delegate to the free functions in
//! `rupu_coverage::tools` and populate `Attribution` from the `ToolContext`.

use async_trait::async_trait;
use rupu_coverage::{
    asset_mark, coverage_concerns_detail, coverage_concerns_search, coverage_mark,
    coverage_remaining, coverage_status, report_finding, AssetMarkInput, Attribution,
    CoverageConcernsDetailInput, CoverageConcernsSearchInput, CoverageMarkInput, CoveragePaths,
    CoverageRemainingInput, CoverageStatusInput, FlatCatalog, ReportFindingInput, Surface,
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
        // `asset` is advertised ONLY under an active engagement. Without one a
        // code-only run's schema is exactly what it was before profiles
        // existed, and the agent is never invited to send `asset`.
        //
        // This is also the only thing keeping a stray `asset` out: with no
        // engagement `report_finding` DROPS `input.asset` (the native `code`
        // path never routes or registers it), so a model that sends one
        // anyway has it silently ignored rather than rejected.
        let engaged = self.options.engagement.is_some();
        match self.options.profile {
            rupu_coverage::FindingProfile::Summary => summary_schema(engaged),
            rupu_coverage::FindingProfile::Full => full_schema(engaged),
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

/// The properties that identify an asset, shared by `report_finding`'s `asset`
/// and `asset_mark`'s input (`asset_mark` adds `depth`). They mirror
/// `rupu_coverage::AssetInput` / `AssetMarkInput`.
fn asset_identity_properties() -> serde_json::Map<String, Value> {
    let Value::Object(props) = serde_json::json!({
        "kind": {
            "type": "string",
            "description": "Profile-namespaced asset kind, e.g. `binary:function`. It must be one the active engagement profiles declare (listed in your instructions)."
        },
        "locator": {
            "type": "array",
            "minItems": 1,
            "items": { "type": "object" },
            "description": "The coordinates that identify the asset: a list of single-key objects, e.g. [{\"sha256\": \"<hex>\"}, {\"address\": 4198400}, {\"symbol\": \"main\"}]. Coordinates: path, line_range {start,end}, symbol, commit, sha256, offset, address, host, port {number,proto}, url, http_route {method,path}, param, resource_id {scheme,id}. Use the coordinates the kind declares."
        },
        "parent": {
            "type": "string",
            "description": "Id of this asset's parent asset (the `asset_id` an earlier asset_mark returned), if it has one."
        },
        "label": {
            "type": "string",
            "description": "A human-readable name for the asset. Omit it to derive one from the kind's label template."
        }
    }) else {
        unreachable!("object literal")
    };
    props
}

/// `report_finding`'s optional `asset` property.
fn asset_property() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["kind", "locator"],
        "description": "The asset this finding is about (an engagement is active). Omit it for a finding about a source file: `file_path` and `scope` identify that.",
        "properties": asset_identity_properties()
    })
}

/// The lightweight record. `with_asset` adds the optional `asset` property
/// (an engagement is active); without it the schema is the original, verbatim.
fn summary_schema(with_asset: bool) -> Value {
    let mut s = serde_json::json!({
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
            }
        }
    });
    if with_asset {
        s["properties"]
            .as_object_mut()
            .expect("object schema")
            .insert("asset".to_string(), asset_property());
    }
    s
}

/// The full profile: locators + a complete `report`. `summary`, `severity`
/// and `evidence` are derived from the report, so they are not offered.
fn full_schema(with_asset: bool) -> Value {
    let mut s = summary_schema(with_asset);
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
// asset_mark
// ---------------------------------------------------------------------------

/// Register an asset and set how deeply it has been covered. Needs an active
/// engagement (it is checked against the profile's kinds and depth ladder), so
/// it is registered only for an agent that lists `asset_mark` in `tools:`.
pub struct AssetMarkTool {
    paths: CoveragePaths,
    options: rupu_coverage::FindingWriteOptions,
}

impl AssetMarkTool {
    /// Build the tool against an explicit ledger location and the run's
    /// findings options (the active engagement rides in `options`).
    pub fn new(paths: CoveragePaths, options: rupu_coverage::FindingWriteOptions) -> Self {
        Self { paths, options }
    }
}

#[async_trait]
impl Tool for AssetMarkTool {
    fn name(&self) -> &'static str {
        "asset_mark"
    }

    fn description(&self) -> &'static str {
        "Register an asset (a function, a host, a route, ...) and record how deeply you \
         have covered it. `kind` must be an asset kind an active engagement profile \
         declares and `depth` a rung of that profile's depth ladder. Re-marking an asset \
         updates its depth. Returns the asset id (usable as another asset's `parent`)."
    }

    fn input_schema(&self) -> Value {
        let mut props = asset_identity_properties();
        props.insert(
            "depth".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "How deeply the asset has been covered: a rung of the owning engagement profile's depth ladder (listed in your instructions)."
            }),
        );
        serde_json::json!({
            "type": "object",
            "required": ["kind", "locator", "depth"],
            "properties": props
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: AssetMarkInput = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let attribution = attribution_from_ctx(ctx);
        let paths = self.paths.clone();
        let options = self.options.clone();
        let result =
            tokio::task::spawn_blocking(move || asset_mark(&paths, attribution, parsed, &options))
                .await;
        match result {
            Ok(Ok(out)) => Ok(ok_output(
                format!(
                    "asset_id: {}\nkind: {}\ndepth: {}",
                    out.id, out.kind, out.depth
                ),
                started,
            )),
            Ok(Err(e)) => Ok(err_output(e.to_string(), started)),
            Err(join) => Ok(err_output(
                format!("asset_mark did not complete: {join}"),
                started,
            )),
        }
    }
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
// Registration
// ---------------------------------------------------------------------------

/// Register all 6 coverage tools into the provided registry.
pub fn register(
    registry: &mut crate::tool_registry::ToolRegistry,
    catalog: FlatCatalog,
    paths: CoveragePaths,
    findings: rupu_coverage::FindingWriteOptions,
) {
    let catalog = Arc::new(catalog);
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

/// Register `asset_mark` against the run's ledger location and findings
/// options. Separate from [`register`] because it is an explicit `tools:`
/// grant on both the `concerns:` path and the findings-without-coverage path,
/// never part of the always-on coverage set (a code-only run must not gain it).
pub fn register_asset_mark(
    registry: &mut crate::tool_registry::ToolRegistry,
    paths: CoveragePaths,
    findings: rupu_coverage::FindingWriteOptions,
) {
    registry.insert("asset_mark", Arc::new(AssetMarkTool::new(paths, findings)));
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
mod engagement_schema_tests {
    use super::*;
    use rupu_coverage::{FindingProfile, FindingWriteOptions};

    fn engaged(profile: FindingProfile) -> FindingWriteOptions {
        let set = rupu_coverage::profile::builtin_registry()
            .unwrap()
            .active_set(&["binary".to_string()])
            .unwrap();
        FindingWriteOptions::default()
            .with_profile(profile)
            .with_engagement(Some(Arc::new(set)))
    }

    fn finding_tool(opts: FindingWriteOptions) -> ReportFindingTool {
        ReportFindingTool::new(CoveragePaths::new(&std::env::temp_dir(), "t"), opts)
    }

    #[test]
    fn asset_is_advertised_only_under_an_engagement() {
        for profile in [FindingProfile::Summary, FindingProfile::Full] {
            let plain =
                finding_tool(FindingWriteOptions::default().with_profile(profile)).input_schema();
            assert!(plain["properties"].get("asset").is_none(), "{profile:?}");

            let mut with = finding_tool(engaged(profile)).input_schema();
            let asset = &with["properties"]["asset"];
            assert_eq!(asset["required"], serde_json::json!(["kind", "locator"]));
            assert!(asset["properties"]["locator"].is_object());
            // Optional: never added to the schema's own required list.
            assert!(!with["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "asset"));

            // The engaged schema differs from the plain one by `asset` alone.
            with["properties"].as_object_mut().unwrap().remove("asset");
            assert_eq!(with, plain, "{profile:?}");
        }
    }

    #[test]
    fn the_advertised_locator_shapes_are_the_ones_the_core_parses() {
        // The schema description tells the model how to spell coordinates;
        // keep it honest against the real serde shapes.
        let loc: rupu_coverage::Locator = serde_json::from_value(serde_json::json!([
            { "path": "a" },
            { "line_range": { "start": 1, "end": 9 } },
            { "symbol": "main" },
            { "commit": "abc" },
            { "sha256": "ab" },
            { "offset": 16 },
            { "address": 4198400 },
            { "host": "10.0.0.1" },
            { "port": { "number": 443, "proto": "tcp" } },
            { "url": "https://x" },
            { "http_route": { "method": "GET", "path": "/x" } },
            { "param": "q" },
            { "resource_id": { "scheme": "ocid", "id": "i" } },
        ]))
        .expect("every coordinate the description names parses");
        assert_eq!(loc.0.len(), 13);
        let desc = asset_identity_properties()["locator"]["description"]
            .as_str()
            .unwrap()
            .to_string();
        for c in &loc.0 {
            assert!(desc.contains(c.tag()), "description omits `{}`", c.tag());
        }
    }

    #[test]
    fn asset_mark_schema_requires_kind_locator_and_depth() {
        let t = AssetMarkTool::new(
            CoveragePaths::new(&std::env::temp_dir(), "t"),
            FindingWriteOptions::default(),
        );
        let s = t.input_schema();
        assert_eq!(
            s["required"],
            serde_json::json!(["kind", "locator", "depth"])
        );
        for k in ["kind", "locator", "parent", "label", "depth"] {
            assert!(s["properties"][k].is_object(), "{k}");
        }
        assert_eq!(t.name(), "asset_mark");
    }

    #[tokio::test]
    async fn asset_mark_without_an_engagement_is_a_loud_tool_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let t = AssetMarkTool::new(
            CoveragePaths::new(tmp.path(), "t"),
            FindingWriteOptions::default(),
        );
        let out = t
            .invoke(
                serde_json::json!({
                    "kind": "binary:function",
                    "locator": [{ "symbol": "main" }],
                    "depth": "analyzed"
                }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        let err = out.error.expect("must not silently no-op");
        assert!(err.contains("engagement"), "{err}");
    }

    #[tokio::test]
    async fn asset_mark_names_the_bad_field_path() {
        let t = AssetMarkTool::new(
            CoveragePaths::new(&std::env::temp_dir(), "t"),
            engaged(FindingProfile::Summary),
        );
        let err = t
            .invoke(
                serde_json::json!({
                    "kind": "binary:function",
                    "locator": [{ "bogus": 1 }],
                    "depth": "analyzed"
                }),
                &ToolContext::default(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("locator"), "{err}");
    }
}
