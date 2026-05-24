//! Tool trait wrappers for the 4 coverage harness tools.
//!
//! These tools are injected into the agent registry when a `concerns:` block
//! is present in the agent frontmatter. They delegate to the free functions in
//! `rupu_coverage::tools` and populate `Attribution` from the `ToolContext`.

use async_trait::async_trait;
use rupu_coverage::{
    coverage_mark, coverage_remaining, coverage_status, report_finding, Attribution,
    CoverageMarkInput, CoverageRemainingInput, CoverageStatusInput, CoveragePaths, FlatCatalog,
    ReportFindingInput, Surface,
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
    }
}

fn ok_output(text: impl Into<String>, elapsed: Instant) -> ToolOutput {
    ToolOutput {
        stdout: text.into(),
        error: None,
        duration_ms: elapsed.elapsed().as_millis() as u64,
        derived: None,
    }
}

fn err_output(text: impl Into<String>, elapsed: Instant) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(text.into()),
        duration_ms: elapsed.elapsed().as_millis() as u64,
        derived: None,
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
        let parsed: CoverageMarkInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
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
        let parsed: CoverageStatusInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        match coverage_status(&self.paths, parsed) {
            Ok(assertions) => {
                let text = serde_json::to_string_pretty(&assertions)
                    .unwrap_or_else(|_| "[]".to_string());
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
        let parsed: CoverageRemainingInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        match coverage_remaining(&self.paths, &self.catalog, parsed) {
            Ok(items) => {
                let text = serde_json::to_string_pretty(&items)
                    .unwrap_or_else(|_| "[]".to_string());
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
}

#[async_trait]
impl Tool for ReportFindingTool {
    fn name(&self) -> &'static str {
        "report_finding"
    }

    fn description(&self) -> &'static str {
        "Record a security or quality finding. Returns the generated finding id \
         which can be referenced in subsequent coverage_mark calls."
    }

    fn input_schema(&self) -> Value {
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
                "scope": {
                    "type": "string",
                    "enum": ["line", "function", "file", "module", "repo"],
                    "description": "Scope at which the finding applies."
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
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: ReportFindingInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let attribution = attribution_from_ctx(ctx);
        match report_finding(&self.paths, attribution, parsed) {
            Ok(out) => Ok(ok_output(format!("finding_id: {}", out.id), started)),
            Err(e) => Ok(err_output(e.to_string(), started)),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all 4 coverage tools into the provided registry.
pub fn register(
    registry: &mut crate::tool_registry::ToolRegistry,
    catalog: FlatCatalog,
    paths: CoveragePaths,
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
        Arc::new(ReportFindingTool {
            paths,
        }),
    );
}
