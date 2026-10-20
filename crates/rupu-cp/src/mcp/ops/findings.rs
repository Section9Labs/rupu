//! The findings operator tools: findings, tags, report exports, coverage and
//! the asset inventory.

use super::{op, OpSpec};
use crate::api::{assets, coverage, findings};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "findings.list",
            Method::GET,
            "/api/findings",
            "List the findings recorded across every workspace.\n\n`q` is a findings query (`severity>=high tag:needs-poc`); `ws_id`, `workflow`, `run_id` and `customer` (a slug or `none`) narrow the scope first. Returns `{findings, summary, facets, tags_unavailable}`: slim rows without report bodies, per-severity counts, the values in use per query key, and the workspaces whose tag log could not be read. A bad `q` is `HTTP 400` naming the offending token.",
        )
        .query::<findings::FindingsQuery>(),
        op(
            "findings.get",
            Method::GET,
            "/api/findings/:id",
            "Read one finding with its whole report.\n\n`id` is the finding id; `ws_id` picks the workspace when the id exists in more than one. Returns the finding object (record, project, customer, tags and their history, report) with a per-claim `evidence_status` (current, changed, missing, unknown) checked against the workspace's files now.",
        )
        .query::<findings::FindingWsQuery>(),
        op(
            "findings.tags",
            Method::GET,
            "/api/findings/tags",
            "List the finding tags in use.\n\n`ws_id` limits it to one workspace. Returns a JSON array of `{tag, count}`, most used first.",
        )
        .query::<findings::TagsQuery>(),
        op(
            "findings.tag",
            Method::POST,
            "/api/findings/tags",
            "Add or remove tags on findings, attributed to the operator.\n\nGive exactly one of `finding_ids` (tagged in every workspace holding them) or `findings` (`{ws_id, id}` pairs, tagged only there), at most 1000 in all, with `add` and `remove` tag lists. Atomic per workspace. Returns `{workspaces, unknown}`: what changed per workspace and the ids no workspace holds; 404 when none matched. The change is recorded as `via: cp` with `client: mcp`.",
        )
        .body::<findings::TagFindingsBody>(),
        op(
            "findings.export",
            Method::GET,
            "/api/findings/:id/export",
            "Export one finding as a report file.\n\n`id` is the finding id; `format` is md, html or pdf; `ws_id` picks the workspace. Returns the file as an embedded resource, text for md and html and a blob for pdf. pdf answers `HTTP 501` on a build without the pdf feature.",
        )
        .query::<findings::ExportQuery>()
        .bytes(),
        op(
            "findings.export_many",
            Method::POST,
            "/api/findings/export",
            "Export a report over the findings a filter selects.\n\n`format` (md, html or pdf) is required; `title`, `ids`, `ws_id`, `run_id`, `min_severity`, `owner`, `cwe` and `include_summaries` narrow the selection, and `split` makes a zip with a file per finding plus an index. Returns the file as an embedded resource; `HTTP 404` when nothing matches.",
        )
        .body::<findings::ExportBody>()
        .read_only()
        .bytes(),
        op(
            "findings.artifact",
            Method::GET,
            "/api/findings/:id/artifacts/:sha256",
            "Fetch a file a finding's report references.\n\n`id` is the finding, `sha256` the artifact's digest as the report lists it; `ws_id` picks the workspace. Returns the file as an embedded resource; `HTTP 404` when the finding does not reference that digest or the file is unavailable, `HTTP 409` when a referenced workspace file changed.",
        )
        .query::<findings::FindingWsQuery>()
        .bytes(),
        op(
            "coverage.list",
            Method::GET,
            "/api/coverage",
            "List the coverage targets of every workspace.\n\nTakes no arguments. Returns a JSON array of `{ws_id, project, target_id, assertion_lines, has_catalog, findings}`.",
        ),
        op(
            "coverage.get",
            Method::GET,
            "/api/coverage/:target",
            "Read one coverage target.\n\n`target` is the target id; `ws_id` picks the workspace when ids collide. Returns `{ws_id, project, target_id, assertion_lines, has_catalog, assertions, findings, files}`; `HTTP 404` for an unknown target.",
        )
        .query::<coverage::GetCoverageQuery>(),
        op(
            "coverage.audit",
            Method::GET,
            "/api/coverage/:target/audit",
            "Audit a coverage target for gaps.\n\n`target` is the target id; `ws_id` picks the workspace. Returns the audit report object; `HTTP 404` for an unknown target.",
        )
        .query::<coverage::GetCoverageQuery>(),
        op(
            "coverage.catalog",
            Method::GET,
            "/api/coverage/:target/catalog",
            "Read a coverage target's effective concern catalog.\n\n`target` is the target id; `ws_id` picks the workspace. Returns the flattened catalog object; `HTTP 404` when the target has no catalog.",
        )
        .query::<coverage::GetCoverageQuery>(),
        op(
            "coverage.runs",
            Method::GET,
            "/api/coverage/:target/runs",
            "List the runs that recorded coverage for a target.\n\n`target` is the target id; `ws_id` picks the workspace. Returns a JSON array of run entries; `HTTP 404` for an unknown target.",
        )
        .query::<coverage::GetCoverageQuery>(),
        op(
            "coverage.diff",
            Method::GET,
            "/api/coverage/:target/diff",
            "Compare coverage between two runs of a target.\n\n`base` (default `previous`) and `compare` (default `latest`) are `latest`, `previous` or a run id; `ws_id` picks the workspace. Returns the run diff object; `HTTP 400` when a selector cannot resolve, `HTTP 404` for an unknown target.",
        )
        .query::<coverage::DiffQuery>(),
        op(
            "coverage.templates",
            Method::GET,
            "/api/coverage/templates",
            "List the bundled concern templates.\n\nTakes no arguments. Returns a JSON array of `{name, version, description, concern_count, severity_breakdown}`.",
        ),
        op(
            "coverage.template",
            Method::GET,
            "/api/coverage/templates/:name",
            "Read one bundled concern template.\n\n`name` is the template's name. Returns the whole template object with its concerns; `HTTP 404` for an unknown name.",
        ),
        op(
            "assets.list",
            Method::GET,
            "/api/assets",
            "List the engagement assets (hosts, services, sites) recorded in coverage.\n\n`ws_id` and `target` narrow it. Returns `{assets}`, each row with id, ws_id, project, target_id, kind, profile, sub_kind, parent, label, depth and flattened coords.",
        )
        .query::<assets::AssetQuery>(),
    ]
}
