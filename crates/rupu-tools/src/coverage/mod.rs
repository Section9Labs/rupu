//! `coverage.*`: the coverage ledger of a run with a `concerns:` block.
//! They read the run's flattened concern catalog from
//! [`ToolServices::coverage_catalog`](crate::ToolServices::coverage_catalog)
//! and write the ledger of the run's findings scope ([`crate::ledger`]).

use crate::coverage_emit::attribution_from;
use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_coverage::{
    coverage_concerns_detail, coverage_concerns_search, coverage_mark, coverage_remaining,
    coverage_status, CoverageConcernsDetailInput, CoverageConcernsSearchInput, CoverageMarkInput,
    CoverageRemainingInput, CoverageStatusInput, FlatCatalog,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

fn parse<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))
}

/// The run's concern catalog. A run without one is never offered these
/// tools (they need [`Service::Coverage`]).
fn catalog(ctx: &ToolContext) -> Result<&Arc<FlatCatalog>, ToolError> {
    ctx.services.coverage_catalog.as_ref().ok_or_else(|| {
        ToolError::Execution(
            "this run has no `concerns:` catalog, so coverage tools can't run".into(),
        )
    })
}

fn pretty<T: serde::Serialize>(v: &T, empty: &str) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| empty.to_string())
}

// was `coverage_mark`
pub static MARK: ToolDescriptor = ToolDescriptor {
    name: "coverage.mark",
    aliases: &[Alias::any("coverage_mark")],
    effect: Effect::Record,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Record a coverage assertion for a (concern_id, file_path) pair. \
     Status must be one of: clean | finding | not_applicable. \
     The file must have been read at the required min_strength first, \
     unless status is not_applicable.",
    input_schema: mark_schema,
};

fn mark_schema() -> Value {
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

pub struct CoverageMarkTool;

#[async_trait]
impl Tool for CoverageMarkTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &MARK
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageMarkInput = parse(input)?;
        let catalog = catalog(ctx)?;
        let paths = crate::ledger::paths(ctx);
        match coverage_mark(&paths, catalog, attribution_from(ctx), parsed).await {
            Ok(out) => {
                let text = if out.warnings.is_empty() {
                    "ok".to_string()
                } else {
                    format!("ok\nwarnings:\n{}", out.warnings.join("\n"))
                };
                Ok(ok(text).timed(started))
            }
            Err(e) => Ok(failed(e.to_string()).timed(started)),
        }
    }
}

// was `coverage_status`
pub static STATUS: ToolDescriptor = ToolDescriptor {
    name: "coverage.status",
    aliases: &[Alias::any("coverage_status")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Query existing coverage assertions. Optionally filter by concern_id, \
     file_path_prefix, or since timestamp. Returns a JSON array of assertion records.",
    input_schema: status_schema,
};

fn status_schema() -> Value {
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

pub struct CoverageStatusTool;

#[async_trait]
impl Tool for CoverageStatusTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &STATUS
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageStatusInput = parse(input)?;
        catalog(ctx)?;
        match coverage_status(&crate::ledger::paths(ctx), parsed) {
            Ok(assertions) => Ok(ok(pretty(&assertions, "[]")).timed(started)),
            Err(e) => Ok(failed(e.to_string()).timed(started)),
        }
    }
}

// was `coverage_remaining`
pub static REMAINING: ToolDescriptor = ToolDescriptor {
    name: "coverage.remaining",
    aliases: &[Alias::any("coverage_remaining")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "List (concern_id, file_path) pairs that have been touched but not yet \
     asserted. Optionally filter by concern_id or min_strength. \
     Use this to discover what still needs coverage.mark calls.",
    input_schema: remaining_schema,
};

fn remaining_schema() -> Value {
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

pub struct CoverageRemainingTool;

#[async_trait]
impl Tool for CoverageRemainingTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &REMAINING
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageRemainingInput = parse(input)?;
        let catalog = catalog(ctx)?;
        match coverage_remaining(&crate::ledger::paths(ctx), catalog, parsed) {
            Ok(items) => Ok(ok(pretty(&items, "[]")).timed(started)),
            Err(e) => Ok(failed(e.to_string()).timed(started)),
        }
    }
}

// was `coverage_concerns_search`
pub static CONCERNS_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "coverage.concerns.search",
    aliases: &[Alias::any("coverage_concerns_search")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Search the concern catalog by substring and/or filter. Returns up to `limit` \
matching concerns in summary form (id, name, severity, summary) by default, or full \
form if `form: 'full'`. Use when the catalog is too large to inline in the prompt.",
    input_schema: concerns_search_schema,
};

fn concerns_search_schema() -> Value {
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

pub struct CoverageConcernsSearchTool;

#[async_trait]
impl Tool for CoverageConcernsSearchTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &CONCERNS_SEARCH
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageConcernsSearchInput = parse(input)?;
        let results = coverage_concerns_search(catalog(ctx)?, parsed);
        Ok(ok(pretty(&results, "[]")).timed(started))
    }
}

// was `coverage_concerns_detail`
pub static CONCERNS_DETAIL: ToolDescriptor = ToolDescriptor {
    name: "coverage.concerns.detail",
    aliases: &[Alias::any("coverage_concerns_detail")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Fetch full concern records by id. Use after coverage.concerns.search finds a \
relevant concern and you need its full description, applicable_globs, or references.",
    input_schema: concerns_detail_schema,
};

fn concerns_detail_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["concern_ids"],
        "properties": {
            "concern_ids": { "type": "array", "items": { "type": "string" } }
        }
    })
}

pub struct CoverageConcernsDetailTool;

#[async_trait]
impl Tool for CoverageConcernsDetailTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &CONCERNS_DETAIL
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: CoverageConcernsDetailInput = parse(input)?;
        let out = coverage_concerns_detail(catalog(ctx)?, parsed);
        Ok(ok(pretty(&out, "{}")).timed(started))
    }
}
