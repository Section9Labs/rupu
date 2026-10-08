//! Descriptors of the coverage-ledger tools (bodies in `rupu-agent`'s
//! `coverage_tools` until W4 moves them here).

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use serde_json::Value;

// was `coverage_mark`
pub static COVERAGE_MARK: ToolDescriptor = ToolDescriptor {
    name: "coverage.mark",
    aliases: &[Alias::any("coverage_mark")],
    effect: Effect::Record,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Record a coverage assertion for a (concern_id, file_path) pair. \
     Status must be one of: clean | finding | not_applicable. \
     The file must have been read at the required min_strength first, \
     unless status is not_applicable.",
    input_schema: coverage_mark_schema,
};

fn coverage_mark_schema() -> Value {
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

// was `coverage_status`
pub static COVERAGE_STATUS: ToolDescriptor = ToolDescriptor {
    name: "coverage.status",
    aliases: &[Alias::any("coverage_status")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Query existing coverage assertions. Optionally filter by concern_id, \
     file_path_prefix, or since timestamp. Returns a JSON array of assertion records.",
    input_schema: coverage_status_schema,
};

fn coverage_status_schema() -> Value {
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

// was `coverage_remaining`
pub static COVERAGE_REMAINING: ToolDescriptor = ToolDescriptor {
    name: "coverage.remaining",
    aliases: &[Alias::any("coverage_remaining")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "List (concern_id, file_path) pairs that have been touched but not yet \
     asserted. Optionally filter by concern_id or min_strength. \
     Use this to discover what still needs coverage.mark calls.",
    input_schema: coverage_remaining_schema,
};

fn coverage_remaining_schema() -> Value {
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

// was `coverage_concerns_search`
pub static COVERAGE_CONCERNS_SEARCH: ToolDescriptor = ToolDescriptor {
    name: "coverage.concerns.search",
    aliases: &[Alias::any("coverage_concerns_search")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Search the concern catalog by substring and/or filter. Returns up to `limit` \
matching concerns in summary form (id, name, severity, summary) by default, or full \
form if `form: 'full'`. Use when the catalog is too large to inline in the prompt.",
    input_schema: coverage_concerns_search_schema,
};

fn coverage_concerns_search_schema() -> Value {
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

// was `coverage_concerns_detail`
pub static COVERAGE_CONCERNS_DETAIL: ToolDescriptor = ToolDescriptor {
    name: "coverage.concerns.detail",
    aliases: &[Alias::any("coverage_concerns_detail")],
    effect: Effect::Read,
    needs: &[Service::Coverage],
    uses: &[],
    description: "Fetch full concern records by id. Use after coverage.concerns.search finds a \
relevant concern and you need its full description, applicable_globs, or references.",
    input_schema: coverage_concerns_detail_schema,
};

fn coverage_concerns_detail_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["concern_ids"],
        "properties": {
            "concern_ids": { "type": "array", "items": { "type": "string" } }
        }
    })
}
