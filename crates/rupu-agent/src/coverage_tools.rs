//! Tool bodies of the coverage-ledger and findings tools. Their descriptors
//! (canonical names, aliases, effects, schemas) live in
//! `rupu_tools::catalog::{coverage, findings}`; W4 moves these bodies there.
//!
//! The coverage tools are injected into the agent registry when a `concerns:`
//! block is present in the agent frontmatter. They delegate to the free
//! functions in `rupu_coverage::tools` and populate `Attribution` from the
//! `ToolContext`.

use async_trait::async_trait;
use rupu_coverage::tools::{verify_finding, VerifyError, VerifyInput};
use rupu_coverage::{
    asset_mark, coverage_concerns_detail, coverage_concerns_search, coverage_mark,
    coverage_remaining, coverage_status, report_finding, AssetMarkInput,
    CoverageConcernsDetailInput, CoverageConcernsSearchInput, CoverageMarkInput, CoveragePaths,
    CoverageRemainingInput, CoverageStatusInput, FlatCatalog, ReportFindingInput,
    VerificationStatus,
};
use rupu_tools::coverage_emit::attribution_from;
use rupu_tools::output::{failed, ok};
use rupu_tools::{Tool, ToolContext, ToolDescriptor, ToolError, ToolOutput};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn ok_in(text: impl Into<String>, started: Instant) -> ToolOutput {
    ok(text).timed(started)
}

fn failed_in(text: impl Into<String>, started: Instant) -> ToolOutput {
    failed(text).timed(started)
}

// ---------------------------------------------------------------------------
// coverage.mark
// ---------------------------------------------------------------------------

pub struct CoverageMarkTool {
    paths: CoveragePaths,
    catalog: Arc<FlatCatalog>,
}

#[async_trait]
impl Tool for CoverageMarkTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::coverage::COVERAGE_MARK
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageMarkInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let attribution = attribution_from(ctx);
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
                Ok(ok_in(text, started))
            }
            Err(e) => Ok(failed_in(e.to_string(), started)),
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
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::coverage::COVERAGE_STATUS
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageStatusInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        match coverage_status(&self.paths, parsed) {
            Ok(assertions) => {
                let text =
                    serde_json::to_string_pretty(&assertions).unwrap_or_else(|_| "[]".to_string());
                Ok(ok_in(text, started))
            }
            Err(e) => Ok(failed_in(e.to_string(), started)),
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
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::coverage::COVERAGE_REMAINING
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageRemainingInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        match coverage_remaining(&self.paths, &self.catalog, parsed) {
            Ok(items) => {
                let text =
                    serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string());
                Ok(ok_in(text, started))
            }
            Err(e) => Ok(failed_in(e.to_string(), started)),
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
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::findings::FINDINGS_REPORT
    }

    fn input_schema(&self) -> Value {
        match self.options.profile {
            rupu_coverage::FindingProfile::Summary => {
                rupu_tools::catalog::findings::summary_schema()
            }
            rupu_coverage::FindingProfile::Full => rupu_tools::catalog::findings::full_schema(),
        }
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        // `serde_path_to_error` so a structural error names where it is
        // (`report.call_chain[0].role: unknown variant ...`) instead of a
        // bare message the agent has to hunt for in a large report.
        let parsed: ReportFindingInput = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let attribution = attribution_from(ctx);
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
            Ok(Ok(out)) => Ok(ok_in(format!("finding_id: {}", out.id), started)),
            Ok(Err(e)) => Ok(failed_in(e.to_string(), started)),
            Err(join) => Ok(failed_in(
                format!("findings.report did not complete: {join}"),
                started,
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// findings.verify
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
    /// Build the tool against an explicit ledger location. It is never part
    /// of the coverage bundle (`register` does not add it): `runner`
    /// registers it only for an agent that lists `findings.verify` (or its alias `finding.verify`) in
    /// `tools:`, with or without a `concerns:` block.
    pub fn new(paths: CoveragePaths) -> Self {
        Self { paths }
    }
}

#[async_trait]
impl Tool for FindingVerifyTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::findings::FINDINGS_VERIFY
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
                    "findings.verify needs the calling run's id, and this context has none; \
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
                ToolError::Execution(format!("findings.verify did not complete: {join}"))
            })?;
        match result {
            Ok(()) => Ok(ok_in(
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
            ) => Ok(failed_in(e.to_string(), started)),
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

// ---------------------------------------------------------------------------
// coverage_concerns_search
// ---------------------------------------------------------------------------

pub struct CoverageConcernsSearchTool {
    catalog: Arc<FlatCatalog>,
}

#[async_trait]
impl Tool for CoverageConcernsSearchTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::coverage::COVERAGE_CONCERNS_SEARCH
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageConcernsSearchInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let results = coverage_concerns_search(&self.catalog, parsed);
        let text = serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
        Ok(ok_in(text, started))
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
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::coverage::COVERAGE_CONCERNS_DETAIL
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageConcernsDetailInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let out = coverage_concerns_detail(&self.catalog, parsed);
        let text = serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_string());
        Ok(ok_in(text, started))
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
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::findings::ASSETS_MARK
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: AssetMarkInput =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let paths = self.paths.clone();
        let engagement = self.engagement.clone();
        let res = tokio::task::spawn_blocking(move || asset_mark(&paths, parsed, &engagement))
            .await
            .map_err(|join| {
                ToolError::Execution(format!("assets.mark did not complete: {join}"))
            })?;
        match res {
            Ok(out) => Ok(ok_in(
                format!("asset {} is at depth `{}`", out.id, out.effective_depth),
                started,
            )),
            Err(e) => Ok(failed_in(e.to_string(), started)),
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
        registry.insert(Arc::new(AssetMarkTool {
            paths: paths.clone(),
            engagement,
        }));
    }
    registry.insert(Arc::new(CoverageMarkTool {
        paths: paths.clone(),
        catalog: catalog.clone(),
    }));
    registry.insert(Arc::new(CoverageStatusTool {
        paths: paths.clone(),
    }));
    registry.insert(Arc::new(CoverageRemainingTool {
        paths: paths.clone(),
        catalog: catalog.clone(),
    }));
    registry.insert(Arc::new(ReportFindingTool::new(paths, findings)));
    registry.insert(Arc::new(CoverageConcernsSearchTool {
        catalog: catalog.clone(),
    }));
    registry.insert(Arc::new(CoverageConcernsDetailTool { catalog }));
}

// ---------------------------------------------------------------------------
// query_findings / tag_findings
// ---------------------------------------------------------------------------

/// Read this workspace's findings, selected by a query string and paged
/// (`ledger::query`).
pub struct QueryFindingsTool {
    workspace: PathBuf,
}

impl QueryFindingsTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for QueryFindingsTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::findings::FINDINGS_QUERY
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let q: rupu_coverage::FindingQuery = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let workspace = self.workspace.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let records =
                rupu_coverage::read_workspace_findings(&workspace).map_err(|e| e.to_string())?;
            rupu_coverage::query_response(&records, &q).map_err(|e| e.to_string())
        })
        .await;
        match result {
            Ok(Ok(v)) => Ok(ok_in(
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into()),
                started,
            )),
            Ok(Err(e)) => Ok(failed_in(e, started)),
            Err(join) => Ok(failed_in(
                format!("findings.query did not complete: {join}"),
                started,
            )),
        }
    }
}

/// Add or remove tags on this workspace's findings (`ledger::tags::apply`).
pub struct TagFindingsTool {
    log: rupu_coverage::TagLog,
}

impl TagFindingsTool {
    pub fn new(log: rupu_coverage::TagLog) -> Self {
        Self { log }
    }
}

#[async_trait]
impl Tool for TagFindingsTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &rupu_tools::catalog::findings::FINDINGS_TAG
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: rupu_coverage::TagChangeInput = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let change = match parsed.into_change() {
            Ok(c) => c,
            Err(e) => return Ok(failed_in(e.to_string(), started)),
        };
        let by = rupu_coverage::TagActor::Agent(attribution_from(ctx));
        let log = self.log.clone();
        let result =
            tokio::task::spawn_blocking(move || rupu_coverage::apply(&log, &change, &by)).await;
        match result {
            Ok(Ok(outcomes)) => Ok(ok_in(
                serde_json::to_string_pretty(&serde_json::json!({ "outcomes": outcomes }))
                    .unwrap_or_else(|_| "{}".into()),
                started,
            )),
            Ok(Err(e)) => Ok(failed_in(e.to_string(), started)),
            Err(join) => Ok(failed_in(
                format!("findings.tag did not complete: {join}"),
                started,
            )),
        }
    }
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
    use rupu_coverage::{Attribution, Surface};

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
            tags: Vec::new(),
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
        assert_eq!(tool.name(), "findings.verify");
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
    fn the_coverage_bundle_registers_report_finding_but_not_finding_verify() {
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
        assert!(names.iter().any(|n| n == "findings.report"), "{names:?}");
        // A verdict is an explicit `tools:` grant, never part of the bundle.
        assert!(!names.iter().any(|n| n == "findings.verify"), "{names:?}");
    }
}
