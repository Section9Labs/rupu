//! The observe operator tools: netflow, usage and the dashboard.

use super::{op, OpSpec};
use crate::api::{dashboard, netflow, usage, usage_outliers};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "netflow.run",
            Method::GET,
            "/api/runs/:id/netflow",
            "List the network flows one run made, sub-agents included.\n\n`id` is the run id; `from` and `to` (RFC 3339, both inclusive) bound the window. `workflow`, `origin`, `org` and `host` keep the flows whose key is in the comma-separated list, `not_*` leave them out; here `host` is a flow filter on `host:port`, not a rupu host. Returns `{flows, hosts, window, dropped_total, asn_loaded, incomplete, exclusions_applied}`; `HTTP 404` for an unknown run.",
        )
        .query::<netflow::TimeRangeQuery>(),
        op(
            "netflow.agentiflow",
            Method::GET,
            "/api/agentiflows/:id/netflow",
            "List the network flows of a whole agentiflow: its lead and every run beneath it.\n\n`id` is the agentiflow id; `from`, `to` and the comma-separated `workflow`, `origin`, `org`, `host` and `not_*` filters work as for netflow.run, and `host` is a flow filter on `host:port`, not a rupu host. Returns `{flows, hosts, window, dropped_total, asn_loaded, incomplete, exclusions_applied}`; `HTTP 404` for an unknown agentiflow.",
        )
        .query::<netflow::TimeRangeQuery>(),
        op(
            "netflow.project",
            Method::GET,
            "/api/projects/:id/netflow",
            "List the network flows attributable to one project.\n\n`id` is the project id; `from`, `to` and the comma-separated `workflow`, `origin`, `org`, `host` and `not_*` filters work as for netflow.run, and `host` is a flow filter on `host:port`, not a rupu host. Returns `{flows, hosts, window, dropped_total, asn_loaded, incomplete, exclusions_applied}`.",
        )
        .query::<netflow::TimeRangeQuery>(),
        op(
            "netflow.global",
            Method::GET,
            "/api/netflow",
            "List the network flows across every registered workspace.\n\n`from`, `to` and the comma-separated `workflow`, `origin`, `org`, `host` and `not_*` filters work as for netflow.run, and `host` is a flow filter on `host:port`, not a rupu host. Returns `{flows, hosts, window, dropped_total, asn_loaded, incomplete, exclusions_applied}`.",
        )
        .query::<netflow::TimeRangeQuery>(),
        op(
            "netflow.graph",
            Method::GET,
            "/api/netflow/graph",
            "Read the source-to-endpoint graph of network flows.\n\n`scope` is `run:<id>` or `project:<id>`; absent or any other value is global. `from` and `to` (RFC 3339) bound the window. Returns `{nodes, edges}`.",
        )
        .query::<netflow::GraphQuery>(),
        op(
            "netflow.explorer",
            Method::GET,
            "/api/netflow/explorer",
            "Read the network explorer aggregates: sankey, timeline, histogram and KPIs.\n\n`scope` is `run:<id>` or `project:<id>`, else global; `from` and `to` bound the window; `workflow`, `origin`, `org`, `host` and their `not_*` twins are comma-separated flow filters, and `host` is a flow filter on `host:port`, not a rupu host. Returns `{sankey, timeline, histogram, kpis, dropped_total, asn_loaded, window, incomplete}`.",
        )
        .query::<netflow::ExplorerQuery>(),
        op(
            "netflow.index",
            Method::GET,
            "/api/netflow/index",
            "Read the size and budget of the netflow index.\n\nTakes no arguments. Returns `{files, flows, tier1_bytes, tier2_bytes, budget_bytes, resident_files, evictions_total}`.",
        ),
        op(
            "netflow.latest",
            Method::GET,
            "/api/netflow/latest",
            "Find the most recent run that recorded network flows.\n\n`scope` is `project:<id>`; absent means global. Returns `{latest}`, where `latest` is `{run_id, workflow, started_at, finished_at}` or null when no run recorded flows.",
        )
        .query::<netflow::LatestQuery>(),
        op(
            "usage.get",
            Method::GET,
            "/api/usage",
            "Summarize token usage and spend, with a breakdown.\n\n`since` and `until` (RFC 3339; default the last 30 days) bound the window; `group_by` is provider, model, agent, workflow, host or project; `host` picks one rupu host, absent fans out to all; `customer` is a slug or `none`. Returns `{summary, breakdown, unpriced, hosts, hosts_without_customer}`, the last present only when a customer filter left hosts out.",
        )
        .query::<usage::UsageQuery>(),
        op(
            "usage.timeline",
            Method::GET,
            "/api/usage/timeline",
            "Read local spend as a gap-filled series of day or week buckets.\n\n`since` and `until` (RFC 3339) bound the window, `bucket` is day or week, `customer` is a slug or `none`. Returns a JSON array of `{bucket, rows, pricing_error?}`, oldest first; local host only. Hosts left out by a customer filter come back as an `x-rupu-hosts-without-customer` note.",
        )
        .query::<usage::TimelineQuery>(),
        op(
            "usage.runs",
            Method::GET,
            "/api/usage/runs",
            "List local spend as one row per run and model.\n\n`since` and `until` (RFC 3339) bound the window, `workspace_id` limits it to one workspace, `customer` is a slug or `none`. Returns a JSON array of `{run_id, kind, started_at, workflow_name, agent, provider, model, input_tokens, output_tokens, total_tokens, cost_usd, priced, ...}`; local host only. Hosts left out by a customer filter come back as an `x-rupu-hosts-without-customer` note.",
        )
        .query::<usage::UsageRunsQuery>(),
        op(
            "usage.outliers",
            Method::GET,
            "/api/usage/outliers",
            "Find runs that cost far more than their baseline.\n\n`since` and `until` (RFC 3339) bound the window, `customer` is a slug or `none`. Returns a JSON array of `{run_id, kind, workflow_name, cost_usd, baseline_usd, ratio, started_at, ...}`; a workflow needs at least three priced runs to have a baseline. Hosts left out by a customer filter come back as an `x-rupu-hosts-without-customer` note.",
        )
        .query::<usage_outliers::OutliersQuery>(),
        op(
            "dashboard.get",
            Method::GET,
            "/api/dashboard",
            "Read the overview dashboard: activity, throughput, cycles, findings and fleet counts.\n\n`range` is 7d, 30d or all; `host` picks one rupu host, absent merges all; `customer` is a slug or `none`. Returns an object with `hosts` (per-host freshness), the `*_partial` flags, `active`, `terminal_buckets`, `throughput_buckets`, `cycles`, `findings_open` and `fleet`.",
        )
        .query::<dashboard::DashboardQuery>(),
    ]
}
