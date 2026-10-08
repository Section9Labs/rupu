//! Descriptors of the findings and asset-ledger tools (bodies in
//! `rupu-agent`'s `coverage_tools` until W4 moves them here).

use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use serde_json::Value;

// was `report_finding`
pub static FINDINGS_REPORT: ToolDescriptor = ToolDescriptor {
    name: "findings.report",
    aliases: &[Alias::any("report_finding"), Alias::any("findings.record")],
    effect: Effect::Record,
    needs: &[Service::Findings],
    uses: &[],
    description: "Record a security or quality finding in this project's ledger. Returns the \
     generated finding id (use it in coverage.mark calls and in another finding's \
     cross_references). Under the full profile send a complete `report`; a rejected \
     call lists every problem to fix.",
    input_schema: full_schema,
};

// was `finding.verify`
pub static FINDINGS_VERIFY: ToolDescriptor = ToolDescriptor {
    name: "findings.verify",
    aliases: &[Alias::any("finding.verify")],
    effect: Effect::Record,
    needs: &[Service::Findings],
    uses: &[],
    description: "Record your verdict on a finding that ANOTHER run filed: confirmed (you \
     reproduced or independently established it), disputed (you showed it is wrong), \
     or inconclusive (you could not decide). Your run and agent are recorded \
     automatically. A finding cannot be verified by the run that filed it, and a \
     finding without a full report cannot be verified. A later verdict replaces an \
     earlier one.",
    input_schema: findings_verify_schema,
};

fn findings_verify_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["finding_id", "status"],
        "properties": {
            "finding_id": {
                "type": "string",
                "description": "The id of the finding to verify (as returned by findings.report)."
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

// was `asset_mark`
pub static ASSETS_MARK: ToolDescriptor = ToolDescriptor {
    name: "assets.mark",
    aliases: &[Alias::any("asset_mark")],
    effect: Effect::Record,
    needs: &[Service::Findings, Service::Engagement],
    uses: &[],
    description: "Record how deeply an engagement asset has been examined, as a rung of \
     its profile's coverage depth ladder (monotonic — a shallower rung after \
     a deeper one keeps the deeper one). The effective rung is returned.",
    input_schema: assets_mark_schema,
};

fn assets_mark_schema() -> Value {
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

// was `query_findings`
pub static FINDINGS_QUERY: ToolDescriptor = ToolDescriptor {
    name: "findings.query",
    aliases: &[Alias::any("query_findings")],
    effect: Effect::Read,
    needs: &[Service::Findings],
    uses: &[],
    description: "List findings recorded in this project with a one-line query in `q` (e.g. \
     `severity>=high tag:needs-poc -has:poc`). Returns one page of slim rows (id, title, \
     severity, location, tags), `next_cursor`, `total`, and `tags_in_use` — reuse an \
     existing tag where it fits. `all: true` returns every match unpaged, however many: \
     page with `cursor` instead unless you need the whole set.",
    input_schema: findings_query_schema,
};

fn findings_query_schema() -> Value {
    rupu_coverage::query_input_schema()
}

// was `tag_findings`
pub static FINDINGS_TAG: ToolDescriptor = ToolDescriptor {
    name: "findings.tag",
    aliases: &[Alias::any("tag_findings")],
    effect: Effect::Record,
    needs: &[Service::Findings],
    uses: &[],
    description: "Add or remove tags on findings in this project, one or many at once. Tags are \
     free-form: lowercase a-z, 0-9 and . _ : / -, starting with a letter or digit (e.g. \
     class:sqli, needs-poc, status:triaged). Prefer tags already in use (findings.query \
     lists them). An unknown finding id rejects the whole call; adding a tag a finding \
     already has changes nothing. Returns each finding's tags before and after.",
    input_schema: findings_tag_schema,
};

fn findings_tag_schema() -> Value {
    rupu_coverage::tag_input_schema()
}

/// The lightweight record: the original schema, verbatim.
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
