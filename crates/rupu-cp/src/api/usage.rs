//! `GET /api/usage` — global token + cost overview (summary + breakdown),
//! fanned out across every registered host.
//!
//! Two rules carried over from `/api/dashboard`'s fan-out (see that module's
//! doc comment):
//!
//! 1. A host that cannot report contributes NOTHING to the aggregate, never
//!    zeros. Its reporting state is carried in `hosts[]`.
//! 2. `priced: false` on `UsageSummary` told you spend was partial but not by
//!    how much or because of what. `unpriced` (below) names the models and
//!    counts the rows, because a silent under-count on an attribution page is
//!    worse than no page.

use crate::{
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, Utc};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/usage", get(get_usage))
        .route("/api/usage/timeline", get(get_usage_timeline))
        .route("/api/usage/runs", get(get_usage_runs))
}

#[derive(Debug, Deserialize)]
struct UsageQuery {
    since: Option<String>,
    until: Option<String>,
    group_by: Option<String>,
    host: Option<String>,
}

/// The models we could not price, named.
///
/// `UsageSummary.priced == false` tells you spend is partial but not by how
/// much or because of what. On an attribution page that is not good enough: a
/// silent under-count is worse than no number.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UnpricedGap {
    /// Distinct model ids with no resolvable price.
    models: Vec<String>,
    /// How many token rows those models account for.
    rows: u64,
}

/// The `rupu usage --group-by` name matching a CP grouping dimension, or
/// `None` when the CLI has no equivalent.
///
/// `Host` maps to the CLI's default `composite`: a remote is a single host
/// from our point of view, so only its totals matter and
/// [`collapse_to_single_host_row`] rebuilds the one row we want from the
/// report's summary. `Project` genuinely has no counterpart — the CLI
/// cannot group by workspace — so it returns `None` and the host is
/// reported `unavailable` with that reason rather than silently omitted.
fn cli_group_name(group_by: crate::usage::GroupBy) -> Option<&'static str> {
    use crate::usage::GroupBy;
    match group_by {
        GroupBy::Provider => Some("provider"),
        GroupBy::Model => Some("model"),
        GroupBy::Agent => Some("agent"),
        GroupBy::Workflow => Some("workflow"),
        GroupBy::Host => Some("composite"),
        GroupBy::Project => None,
    }
}

/// Replace a report's per-dimension rows with the single row `group_by=host`
/// wants: this whole host's totals, taken from the summary the remote
/// already computed. The caller tags it with the real registered host id.
fn collapse_to_single_host_row(mut body: RemoteUsageBody) -> RemoteUsageBody {
    let s = &body.summary;
    body.breakdown = vec![crate::usage::UsageBreakdownRow {
        provider: String::new(),
        model: String::new(),
        agent: String::new(),
        workflow: String::new(),
        host_id: String::new(),
        workspace_id: String::new(),
        input_tokens: s.input_tokens,
        output_tokens: s.output_tokens,
        cached_tokens: s.cached_tokens,
        total_tokens: s.total_tokens,
        cost_usd: s.cost_usd,
        priced: s.priced,
        runs: s.runs,
    }];
    body
}

/// Fetch one remote host's usage body, over whichever surface that host
/// actually has.
///
/// Tries the structured [`HostConnector::usage_rollup`] first: the default
/// impl returns `Unsupported` instantly with no I/O, so an HTTP host falls
/// straight through to the generic-GET proxy it has always used, and an SSH
/// host — which has no generic-GET surface at all — answers by shelling
/// `rupu usage --format json`. A real transport failure (`Unreachable`)
/// propagates rather than being retried on the other surface.
///
/// The outer `Result` is transport failure; the inner one is "this build
/// cannot read that body", which the caller renders as an offline host with
/// a reason — never as a silent zero contribution.
async fn remote_usage_body(
    conn: &dyn crate::host::connector::HostConnector,
    proxy_path: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    group_by: crate::usage::GroupBy,
) -> Result<Result<RemoteUsageBody, String>, HostConnectorError> {
    let cli_group = cli_group_name(group_by);
    if let Some(cli_group) = cli_group {
        match conn
            .usage_rollup(&start.to_rfc3339(), &end.to_rfc3339(), cli_group)
            .await
        {
            Ok(report) => {
                let body = usage_body_from_remote_report(&report).map(|b| {
                    if group_by == crate::usage::GroupBy::Host {
                        collapse_to_single_host_row(b)
                    } else {
                        b
                    }
                });
                return Ok(body);
            }
            // No structured surface on this transport — fall through to the
            // generic-GET proxy below.
            Err(HostConnectorError::Unsupported(_)) => {}
            Err(other) => return Err(other),
        }
    }

    let v = conn.proxy_get_json(proxy_path).await?;
    Ok(serde_json::from_value::<RemoteUsageBody>(v).map_err(|e| e.to_string()))
}

/// Map a remote `rupu usage --format json` report onto the API body the
/// per-host fan-out expects.
///
/// The two shapes are close but not identical: the CLI nests its totals
/// under `summary` with `total_`-prefixed names, spells "this row could not
/// be fully priced" as `cost_partial` (the API's `priced`, inverted), and
/// carries no `total_tokens` per row. Each `rows` entry only populates the
/// identity fields its `group_by` selected, so the rest map to empty
/// strings — the UI labels by the grouped dimension anyway.
///
/// `host_id` is deliberately left empty here: the caller overwrites it with
/// the real registered host id (a remote reports its own rows as "local").
fn usage_body_from_remote_report(report: &serde_json::Value) -> Result<RemoteUsageBody, String> {
    let summary_val = report
        .get("summary")
        .ok_or_else(|| "usage report has no `summary`".to_string())?;
    let u64_at = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let str_at = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    // `cost_partial` is the CLI's inverse of the API's `priced`.
    let partial_at = |v: &serde_json::Value| {
        v.get("cost_partial")
            .and_then(|x| x.as_bool())
            .unwrap_or(false)
    };

    let summary = crate::usage::UsageSummary {
        input_tokens: u64_at(summary_val, "total_input_tokens"),
        output_tokens: u64_at(summary_val, "total_output_tokens"),
        cached_tokens: u64_at(summary_val, "total_cached_tokens"),
        total_tokens: u64_at(summary_val, "total_tokens"),
        cost_usd: summary_val.get("total_cost_usd").and_then(|x| x.as_f64()),
        priced: !partial_at(summary_val),
        runs: u64_at(summary_val, "total_runs"),
    };

    let empty = Vec::new();
    let rows = report
        .get("rows")
        .and_then(|r| r.as_array())
        .unwrap_or(&empty);

    let mut breakdown = Vec::with_capacity(rows.len());
    let mut unpriced_models = std::collections::BTreeSet::new();
    let mut unpriced_rows = 0u64;

    for r in rows {
        let input_tokens = u64_at(r, "input_tokens");
        let output_tokens = u64_at(r, "output_tokens");
        let priced = !partial_at(r) && r.get("cost_usd").and_then(|x| x.as_f64()).is_some();
        let model = str_at(r, "model");
        if !priced {
            // Name the model when the remote grouped by one; otherwise the
            // group label is the most specific thing we were told.
            let label = if model.is_empty() {
                str_at(r, "group")
            } else {
                model.clone()
            };
            if !label.is_empty() {
                unpriced_models.insert(label);
            }
            unpriced_rows += 1;
        }
        breakdown.push(crate::usage::UsageBreakdownRow {
            provider: str_at(r, "provider"),
            model,
            agent: str_at(r, "agent"),
            workflow: str_at(r, "workflow"),
            // Overwritten by the caller with the real registered host id.
            host_id: String::new(),
            // A remote's workspace ids are meaningless in our registry.
            workspace_id: String::new(),
            input_tokens,
            output_tokens,
            cached_tokens: u64_at(r, "cached_tokens"),
            // The CLI report has no per-row total; cached tokens are not
            // added in, matching `crate::usage::breakdown`'s own arithmetic.
            total_tokens: input_tokens + output_tokens,
            cost_usd: r.get("cost_usd").and_then(|x| x.as_f64()),
            priced,
            runs: u64_at(r, "runs"),
        });
    }

    Ok(RemoteUsageBody {
        summary,
        breakdown,
        unpriced: UnpricedGap {
            models: unpriced_models.into_iter().collect(),
            rows: unpriced_rows,
        },
    })
}

/// Distinct unpriced model ids + the row count they account for, computed
/// with the SAME price-resolution path `summarize`/`breakdown` use
/// (`rupu_config::pricing::lookup`) — no second lookup implementation to
/// drift out of sync with what actually drove `priced = false`.
fn unpriced_gap(
    rows: &[rupu_transcript::UsageRow],
    pricing: &rupu_config::PricingConfig,
) -> UnpricedGap {
    use std::collections::BTreeSet;
    let mut models: BTreeSet<String> = BTreeSet::new();
    let mut count = 0u64;
    for r in rows {
        if rupu_config::pricing::lookup(pricing, &r.provider, &r.model, &r.agent).is_none() {
            models.insert(r.model.clone());
            count += 1;
        }
    }
    UnpricedGap {
        models: models.into_iter().collect(),
        rows: count,
    }
}

/// Union many hosts' unpriced gaps: models union (distinct across the
/// fleet), rows sum (each host's rows are disjoint — its own runs).
fn merge_unpriced(gaps: impl Iterator<Item = UnpricedGap>) -> UnpricedGap {
    use std::collections::BTreeSet;
    let mut models: BTreeSet<String> = BTreeSet::new();
    let mut rows = 0u64;
    for g in gaps {
        models.extend(g.models);
        rows += g.rows;
    }
    UnpricedGap {
        models: models.into_iter().collect(),
        rows,
    }
}

/// Merge already-grouped breakdown rows from multiple hosts, re-grouping by
/// the SAME key `crate::usage::breakdown` would use for `group_by`. Needed
/// because a remote host's `/api/usage` response arrives pre-aggregated (its
/// own raw `UsageRow`s never cross the wire) — this is a second-stage fold
/// over already-summed rows, not a duplicate of `breakdown`'s row-level
/// grouping.
fn merge_breakdown_rows(
    rows: Vec<crate::usage::UsageBreakdownRow>,
    group_by: crate::usage::GroupBy,
) -> Vec<crate::usage::UsageBreakdownRow> {
    use crate::usage::GroupBy;
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<String, crate::usage::UsageBreakdownRow> = BTreeMap::new();
    for row in rows {
        let key = match group_by {
            GroupBy::Provider => row.provider.clone(),
            GroupBy::Model => row.model.clone(),
            GroupBy::Agent => row.agent.clone(),
            GroupBy::Workflow => row.workflow.clone(),
            GroupBy::Host => row.host_id.clone(),
            GroupBy::Project => row.workspace_id.clone(),
        };
        groups
            .entry(key)
            .and_modify(|acc| {
                acc.input_tokens += row.input_tokens;
                acc.output_tokens += row.output_tokens;
                acc.cached_tokens += row.cached_tokens;
                acc.total_tokens += row.total_tokens;
                acc.runs += row.runs;
                acc.cost_usd = match (acc.cost_usd, row.cost_usd) {
                    (Some(a), Some(b)) => Some(a + b),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                if !row.priced {
                    acc.priced = false;
                }
            })
            .or_insert(row);
    }
    let mut out: Vec<_> = groups.into_values().collect();
    out.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.model.cmp(&b.model))
    });
    out
}

/// One host's reporting state for the `/api/usage` freshness strip. Mirrors
/// `/api/dashboard`'s `HostFreshness` exactly (see that module's doc comment
/// for the "not reported ≠ zero" rationale) — duplicated rather than shared
/// because the two responses' merge semantics differ enough that a shared
/// type would need its own set of exceptions.
#[derive(Debug, Serialize)]
struct HostFreshness {
    host_id: String,
    name: String,
    transport_kind: String,
    /// `"ok"` | `"offline"` | `"unavailable"`.
    state: &'static str,
    /// Present only when `state == "ok"`.
    captured_at: Option<DateTime<Utc>>,
    /// Human-readable cause when `state != "ok"`.
    reason: Option<String>,
}

/// One host's parsed `/api/usage` contribution, fed into the merge below.
struct HostUsage {
    summary: crate::usage::UsageSummary,
    breakdown: Vec<crate::usage::UsageBreakdownRow>,
    unpriced: UnpricedGap,
}

/// Wire shape parsed out of a remote host's `/api/usage?host=local&...`
/// response. Only the fields this endpoint needs to re-aggregate.
#[derive(Deserialize)]
#[cfg_attr(test, derive(Debug))]
struct RemoteUsageBody {
    summary: crate::usage::UsageSummary,
    breakdown: Vec<crate::usage::UsageBreakdownRow>,
    unpriced: UnpricedGap,
}

#[derive(Debug, Serialize)]
struct UsageResponse {
    summary: crate::usage::UsageSummary,
    breakdown: Vec<crate::usage::UsageBreakdownRow>,
    unpriced: UnpricedGap,
    hosts: Vec<HostFreshness>,
}

/// Resolve the [since, until] window from optional RFC-3339 strings.
/// Absent `since` → now − 30 days; absent `until` → now. A present-but-unparseable
/// bound is an error (caller maps to 400) rather than a silent default.
///
/// `pub(crate)`: `crate::api::usage_outliers` reuses this exact window
/// resolution (Task W1) rather than re-deriving its own, so `/api/usage/runs`
/// and `/api/usage/outliers` can't drift apart on how `since`/`until` default
/// or fail to parse.
pub(crate) fn resolve_window(
    since: Option<&str>,
    until: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let parse = |s: &str| -> Result<DateTime<Utc>, String> {
        DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| format!("invalid timestamp {s:?}: {e}"))
    };
    let start = match since {
        Some(s) => parse(s)?,
        None => now - Duration::days(30),
    };
    let end = match until {
        Some(u) => parse(u)?,
        None => now,
    };
    Ok((start, end))
}

/// Read the local run store and build this host's own usage contribution for
/// `[start, end]`. Split out of `get_usage` so the fan-out loop below can call
/// it for the `"local"` target without a network round trip — mirrors how
/// `/api/dashboard` special-cases `host_id == "local"` by resolving straight
/// to the in-process connector rather than proxying to itself.
fn local_usage(
    s: &AppState,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    group_by: crate::usage::GroupBy,
) -> Result<HostUsage, ApiError> {
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut all_rows: Vec<rupu_transcript::UsageRow> = Vec::new();
    for r in runs
        .iter()
        .filter(|r| r.started_at >= start && r.started_at <= end)
    {
        let paths = crate::usage::run_transcript_paths_resolved(&s.run_store, &s.hosts, &r.id);
        let mut rows = rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default());
        // Attribute each row to the run it came from: `r` is already in hand
        // for this batch, so this is a free inline join — no re-load, no
        // cache, no separate `attribute_rows` function needed. `host_id` is
        // hardcoded "local" because this only ever reads the local run
        // store; a REMOTE host's rows carry ITS OWN "local" tag from ITS
        // point of view — see the `GroupBy::Host` override in the fan-out
        // loop below, which is what keeps `group_by=host` meaningful across
        // more than one host.
        for row in &mut rows {
            row.workflow = r.workflow_name.clone();
            row.workspace_id = r.workspace_id.clone();
            row.host_id = "local".to_string();
        }
        all_rows.extend(rows);
    }

    let summary = crate::usage::summarize(&all_rows, &s.pricing);
    let breakdown = crate::usage::breakdown(&all_rows, &s.pricing, group_by);
    let unpriced = unpriced_gap(&all_rows, &s.pricing);
    Ok(HostUsage {
        summary,
        breakdown,
        unpriced,
    })
}

async fn get_usage(
    State(s): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> ApiResult<Json<UsageResponse>> {
    let (start, end) = resolve_window(q.since.as_deref(), q.until.as_deref(), Utc::now())
        .map_err(ApiError::bad_request)?;
    let group_by = match q.group_by.as_deref() {
        None => crate::usage::GroupBy::Model,
        Some(g) => crate::usage::GroupBy::parse(g).ok_or_else(|| {
            ApiError::bad_request(format!(
                "unknown group_by {g:?}; expected provider | model | agent | workflow | host | project"
            ))
        })?,
    };

    // Which hosts to ask: one named host, or every registered host. Mirrors
    // `/api/dashboard`'s exact scoping idiom (`HostRegistry` has no per-id
    // lookup; `list_hosts()` is the only enumeration surface).
    let targets: Vec<_> = match q.host.as_deref() {
        Some(id) => {
            let found = s
                .hosts
                .list_hosts()
                .into_iter()
                .find(|h| h.id == id)
                .ok_or_else(|| ApiError::not_found(format!("unknown host {id}")))?;
            vec![found]
        }
        None => s.hosts.list_hosts(),
    };

    let futs = targets.into_iter().map(|h| {
        let registry = Arc::clone(&s.hosts);
        let state = s.clone();
        let host_id = h.id.clone();
        let name = h.name.clone();
        let (transport_kind, _base_url) = crate::api::hosts::transport_fields(&h.transport);
        async move {
            if host_id == "local" {
                return match local_usage(&state, start, end, group_by) {
                    Ok(usage) => (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "ok",
                            captured_at: Some(Utc::now()),
                            reason: None,
                        },
                        Some(usage),
                    ),
                    Err(e) => (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(e.1),
                        },
                        None,
                    ),
                };
            }

            // Remote host: proxy `GET /api/usage` on the remote's OWN local
            // data (`host=local` — the recursion base every HTTP connector
            // call scopes to, same as `/api/dashboard`, so a multi-hop
            // topology never double-counts). `since`/`until` are forwarded
            // as the already-RESOLVED bounds (not the possibly-absent
            // originals) so every host sums the SAME window; `group_by` is
            // forwarded too so a remote's breakdown groups identically.
            let conn = match registry.resolve(&host_id) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(host_id = %host_id, error = %e, "usage: could not resolve host connector");
                    return (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(e.to_string()),
                        },
                        None,
                    );
                }
            };

            let path = format!(
                "/api/usage?host=local&since={}&until={}&group_by={}",
                urlencoding_rfc3339(start),
                urlencoding_rfc3339(end),
                group_by.as_str(),
            );

            // A transport with no generic-GET surface (SSH) answers through
            // its structured `usage_rollup` instead, shelling `rupu usage
            // --format json` on the remote. `remote_usage_body` picks the
            // path and normalizes both onto `RemoteUsageBody`.
            match remote_usage_body(conn.as_ref(), &path, start, end, group_by).await {
                Ok(v) => match v {
                    Ok(body) => {
                        // `group_by=host`: the remote's own rows are tagged
                        // "local" from ITS point of view — override to the
                        // ACTUAL registered host id so a fleet of hosts
                        // doesn't collapse into one "local" bucket.
                        let breakdown = if group_by == crate::usage::GroupBy::Host {
                            body.breakdown
                                .into_iter()
                                .map(|mut row| {
                                    row.host_id = host_id.clone();
                                    row
                                })
                                .collect()
                        } else {
                            body.breakdown
                        };
                        (
                            HostFreshness {
                                host_id,
                                name,
                                transport_kind,
                                state: "ok",
                                captured_at: Some(Utc::now()),
                                reason: None,
                            },
                            Some(HostUsage {
                                summary: body.summary,
                                breakdown,
                                unpriced: body.unpriced,
                            }),
                        )
                    }
                    Err(e) => (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(format!("bad usage response: {e}")),
                        },
                        None,
                    ),
                },
                // A transport with NEITHER surface for this request: a
                // Tunnel/Bucket host (no generic GET, no `usage_rollup`),
                // or an SSH host asked to group by `project`, which the
                // remote CLI cannot do. Both connectors answer INSTANTLY —
                // no round trip, no stall. Renders `unavailable` with the
                // reason, never a silent omission from `hosts[]`.
                Err(HostConnectorError::Invalid(reason))
                | Err(HostConnectorError::Unsupported(reason)) => (
                    HostFreshness {
                        host_id,
                        name,
                        transport_kind,
                        state: "unavailable",
                        captured_at: None,
                        reason: Some(reason),
                    },
                    None,
                ),
                Err(e) => {
                    tracing::warn!(host_id = %host_id, error = %e, "usage: proxy_get_json failed");
                    (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(e.to_string()),
                        },
                        None,
                    )
                }
            }
        }
    });

    let results = join_all(futs).await;

    // A non-reporting host contributes NOTHING — never zeros — to the merge;
    // its state is carried in `hosts` instead. Same rule as `/api/dashboard`.
    let mut hosts = Vec::new();
    let mut reported = Vec::new();
    for (freshness, usage) in results {
        hosts.push(freshness);
        if let Some(u) = usage {
            reported.push(u);
        }
    }

    let summary = crate::usage::rollup(reported.iter().map(|u| u.summary.clone()));
    let breakdown = merge_breakdown_rows(
        reported.iter().flat_map(|u| u.breakdown.clone()).collect(),
        group_by,
    );
    let unpriced = merge_unpriced(reported.into_iter().map(|u| u.unpriced));

    Ok(Json(UsageResponse {
        summary,
        breakdown,
        unpriced,
        hosts,
    }))
}

/// RFC-3339 timestamp, percent-encoded for use as a query-string value.
/// `chrono`'s `to_rfc3339()` on a `DateTime<Utc>` renders the `+00:00` offset
/// form (not `Z`): both the `:` separators and the `+` sign are unsafe to
/// leave bare — form-encoded query strings decode a bare `+` as a space,
/// which is exactly what silently corrupted the forwarded timestamp before
/// this was caught by an end-to-end fan-out test.
fn urlencoding_rfc3339(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339().replace('+', "%2B").replace(':', "%3A")
}

/// Bucket granularity for the usage timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Day,
    Week,
}

impl Granularity {
    /// Parse the `bucket` query param. `None`/absent → `Day`; `"day"`/`"week"`
    /// map to their variant; anything else is an error (caller maps to 400).
    fn parse(s: Option<&str>) -> Result<Self, String> {
        match s {
            None | Some("day") => Ok(Granularity::Day),
            Some("week") => Ok(Granularity::Week),
            Some(other) => Err(format!(
                "invalid bucket {other:?}: expected \"day\" or \"week\""
            )),
        }
    }
}

/// One time bucket of the usage timeline: a `YYYY-MM-DD` key plus the per-model
/// breakdown of every run whose `started_at` falls in that bucket.
#[derive(Debug, Serialize)]
struct UsageTimelineBucket {
    bucket: String,
    rows: Vec<crate::usage::UsageBreakdownRow>,
}

#[derive(Debug, Deserialize)]
struct TimelineQuery {
    since: Option<String>,
    until: Option<String>,
    bucket: Option<String>,
}

/// Map a timestamp to its bucket key. `Day` → that day's `YYYY-MM-DD`; `Week`
/// → the Monday (ISO) of that week, also `YYYY-MM-DD`.
fn bucket_key(dt: DateTime<Utc>, granularity: Granularity) -> String {
    let date = dt.date_naive();
    let date = match granularity {
        Granularity::Day => date,
        Granularity::Week => date - Duration::days(date.weekday().num_days_from_monday() as i64),
    };
    date.format("%Y-%m-%d").to_string()
}

/// Gap-fill start for the timeline, or `None` when the store has no runs at all
/// (caller returns an empty series). Clamps the window start up to the
/// first-ever run: bounded windows (7d/30d) draw the full window with zeros;
/// the unbounded `all` window starts at the first run instead of the epoch.
fn timeline_fill_start(
    window_start: DateTime<Utc>,
    earliest_run: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    earliest_run.map(|earliest| window_start.max(earliest))
}

/// Every bucket key from `fill_start` to `fill_end` inclusive, at the granularity.
/// `Day` → one key per calendar day; `Week` → one key per ISO week, starting from
/// the Monday on or before `fill_start`. Produces `YYYY-MM-DD` keys identical in
/// form to [`bucket_key`], so they align with grouped run buckets.
fn enumerate_bucket_keys(
    fill_start: DateTime<Utc>,
    fill_end: DateTime<Utc>,
    granularity: Granularity,
) -> Vec<String> {
    let mut cursor = match granularity {
        Granularity::Day => fill_start.date_naive(),
        Granularity::Week => {
            let d = fill_start.date_naive();
            d - Duration::days(d.weekday().num_days_from_monday() as i64)
        }
    };
    let end = fill_end.date_naive();
    let step = match granularity {
        Granularity::Day => Duration::days(1),
        Granularity::Week => Duration::days(7),
    };
    let mut keys = Vec::new();
    while cursor <= end {
        keys.push(cursor.format("%Y-%m-%d").to_string());
        cursor += step;
    }
    keys
}

/// Group per-run `(started_at, rows)` by bucket key, then emit a CONTINUOUS run
/// of buckets from `fill_start` to `fill_end` inclusive at the granularity —
/// synthesizing an empty bucket (`rows: []`) for every period with no runs, so
/// the timeline has no gaps and reaches `fill_end`. Buckets are chronological.
fn build_timeline(
    runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)>,
    pricing: &rupu_config::PricingConfig,
    granularity: Granularity,
    fill_start: DateTime<Utc>,
    fill_end: DateTime<Utc>,
) -> Vec<UsageTimelineBucket> {
    let mut grouped: std::collections::BTreeMap<String, Vec<rupu_transcript::UsageRow>> =
        std::collections::BTreeMap::new();
    for (started_at, rows) in runs_with_rows {
        let key = bucket_key(started_at, granularity);
        grouped.entry(key).or_default().extend(rows);
    }
    enumerate_bucket_keys(fill_start, fill_end, granularity)
        .into_iter()
        .map(|bucket| {
            let rows = grouped
                .get(&bucket)
                .map(|rows| crate::usage::breakdown(rows, pricing, crate::usage::GroupBy::Model))
                .unwrap_or_default();
            UsageTimelineBucket { rows, bucket }
        })
        .collect()
}

async fn get_usage_timeline(
    State(s): State<AppState>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<Vec<UsageTimelineBucket>>> {
    let (start, end) = resolve_window(q.since.as_deref(), q.until.as_deref(), Utc::now())
        .map_err(ApiError::bad_request)?;
    let granularity = Granularity::parse(q.bucket.as_deref()).map_err(ApiError::bad_request)?;

    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Clamp the fill start to the first-ever run; no runs at all → empty series.
    let earliest_overall = runs.iter().map(|r| r.started_at).min();
    let Some(fill_start) = timeline_fill_start(start, earliest_overall) else {
        return Ok(Json(Vec::new()));
    };

    let mut runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)> = Vec::new();
    for r in runs
        .iter()
        .filter(|r| r.started_at >= start && r.started_at <= end)
    {
        let paths = crate::usage::run_transcript_paths_resolved(&s.run_store, &s.hosts, &r.id);
        let rows = rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default());
        runs_with_rows.push((r.started_at, rows));
    }

    Ok(Json(build_timeline(
        runs_with_rows,
        &s.pricing,
        granularity,
        fill_start,
        end,
    )))
}

/// One flat `(run × model)` usage row — the finest grain the client needs to
/// filter the `/usage` spend graph interactively (exclude a run or a
/// pivot-key and every bucket it fed instantly rescales, client-side, with no
/// refetch). Local-only, like `/api/usage/timeline` and `/api/usage/outliers`
/// above: reads `s.run_store` directly, no host fan-out — `host_id` is always
/// `"local"`.
///
/// `cost_usd` is priced HERE, server-side, with the SAME
/// `rupu_config::pricing::lookup` path `summarize`/`breakdown` use — the
/// client only ever sums a server-priced number, never prices client-side,
/// so the two surfaces can't drift.
#[derive(Debug, Serialize)]
struct UsageRunRow {
    run_id: String,
    started_at: DateTime<Utc>,
    workflow_name: String,
    agent: String,
    provider: String,
    model: String,
    workspace_id: String,
    host_id: String,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    total_tokens: u64,
    /// `None` = unpriced. Never fabricated — mirrors `UsageBreakdownRow.cost_usd`.
    cost_usd: Option<f64>,
    priced: bool,
}

#[derive(Debug, Deserialize)]
struct UsageRunsQuery {
    since: Option<String>,
    until: Option<String>,
    workspace_id: Option<String>,
}

/// `GET /api/usage/runs?since=&until=&workspace_id=` — flat per-`(run × model)` rows.
///
/// **Local-only**: reads `s.run_store` directly, exactly like
/// `/api/usage/timeline` and `/api/usage/outliers` — no host fan-out. A
/// multi-host fleet view is a follow-up (see the `?host=` fan-out on
/// `/api/usage` for the pattern), not silently faked here.
///
/// Reuses `local_usage`'s exact per-run join (attribute each `UsageRow` to
/// its `RunRecord` inline, no re-load) rather than re-deriving it, then
/// flattens: one `UsageRunRow` per `UsageRow` a run produced, carrying that
/// run's `run_id`/`started_at` and a per-row price from the SAME
/// `rupu_config::pricing::lookup` path `summarize`/`breakdown` use.
async fn get_usage_runs(
    State(s): State<AppState>,
    Query(q): Query<UsageRunsQuery>,
) -> ApiResult<Json<Vec<UsageRunRow>>> {
    let (start, end) = resolve_window(q.since.as_deref(), q.until.as_deref(), Utc::now())
        .map_err(ApiError::bad_request)?;

    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut out = Vec::new();
    for r in runs.iter().filter(|r| {
        r.started_at >= start
            && r.started_at <= end
            && q.workspace_id
                .as_deref()
                .is_none_or(|w| r.workspace_id == w)
    }) {
        let paths = crate::usage::run_transcript_paths_resolved(&s.run_store, &s.hosts, &r.id);
        let mut rows = rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default());
        // Same inline attribution `local_usage` does above: `r` is already in
        // hand for this batch, so no re-load/cache/separate join function.
        for row in &mut rows {
            row.workflow = r.workflow_name.clone();
            row.workspace_id = r.workspace_id.clone();
            row.host_id = "local".to_string();
        }
        for row in rows {
            let priced_cost =
                rupu_config::pricing::lookup(&s.pricing, &row.provider, &row.model, &row.agent)
                    .map(|price| {
                        price.cost_usd(row.input_tokens, row.output_tokens, row.cached_tokens)
                    });
            out.push(UsageRunRow {
                run_id: r.id.clone(),
                started_at: r.started_at,
                workflow_name: row.workflow,
                agent: row.agent,
                provider: row.provider,
                model: row.model,
                workspace_id: row.workspace_id,
                host_id: row.host_id,
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cached_tokens: row.cached_tokens,
                total_tokens: row.input_tokens + row.output_tokens,
                priced: priced_cost.is_some(),
                cost_usd: priced_cost,
            });
        }
    }

    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    /// Shape captured from a real `rupu --format json usage --group-by
    /// model` run, not inferred from the CSV headers: the report nests its
    /// totals under `summary` with `total_`-prefixed names, and each `rows`
    /// entry carries only the identity fields its `group_by` populates.
    #[test]
    fn remote_usage_report_maps_onto_the_cp_body() {
        let report = serde_json::json!({
            "kind": "usage_breakdown",
            "version": 1,
            "group_by": "model",
            "summary": {
                "total_input_tokens": 100,
                "total_output_tokens": 50,
                "total_cached_tokens": 10,
                "total_tokens": 150,
                "total_runs": 2,
                "total_cost_usd": 1.25,
                "cost_partial": false
            },
            "rows": [{
                "group": "opus",
                "model": "opus",
                "input_tokens": 100,
                "output_tokens": 50,
                "cached_tokens": 10,
                "runs": 2,
                "cost_usd": 1.25,
                "cost_partial": false
            }]
        });

        let body = usage_body_from_remote_report(&report).expect("mappable");

        assert_eq!(body.summary.input_tokens, 100);
        assert_eq!(body.summary.output_tokens, 50);
        assert_eq!(body.summary.total_tokens, 150);
        assert_eq!(body.summary.runs, 2);
        assert_eq!(body.summary.cost_usd, Some(1.25));
        assert!(body.summary.priced, "cost_partial:false -> priced");

        assert_eq!(body.breakdown.len(), 1);
        let row = &body.breakdown[0];
        assert_eq!(row.model, "opus");
        assert_eq!(row.total_tokens, 150, "input + output, cached excluded");
        assert_eq!(row.runs, 2);
        assert!(row.priced);
        assert!(
            row.host_id.is_empty(),
            "the caller tags rows with the real host id"
        );
    }

    /// A partially-priced remote must not be reported as fully priced, and
    /// its unpriced models must be named — the Usage screen surfaces that
    /// gap explicitly rather than showing a silently-low cost.
    #[test]
    fn remote_usage_report_carries_the_unpriced_gap() {
        let report = serde_json::json!({
            "summary": {
                "total_input_tokens": 10, "total_output_tokens": 5,
                "total_cached_tokens": 0, "total_tokens": 15,
                "total_runs": 2, "total_cost_usd": 0.5, "cost_partial": true
            },
            "rows": [
                {"group": "opus", "model": "opus", "input_tokens": 10,
                 "output_tokens": 5, "cached_tokens": 0, "runs": 1,
                 "cost_usd": 0.5, "cost_partial": false},
                {"group": "mystery", "model": "mystery", "input_tokens": 0,
                 "output_tokens": 0, "cached_tokens": 0, "runs": 1,
                 "cost_usd": null, "cost_partial": true}
            ]
        });

        let body = usage_body_from_remote_report(&report).expect("mappable");

        assert!(
            !body.summary.priced,
            "cost_partial:true -> not fully priced"
        );
        assert_eq!(body.unpriced.models, vec!["mystery".to_string()]);
        assert_eq!(body.unpriced.rows, 1);
    }

    /// A transport with no generic-GET surface at all — an SSH host. Its
    /// `proxy_get_json` errors the way the real one does; only
    /// `usage_rollup` answers.
    struct RollupOnlyConnector {
        report: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl crate::host::connector::HostConnector for RollupOnlyConnector {
        async fn usage_rollup(
            &self,
            _since: &str,
            _until: &str,
            _group_by: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            Ok(self.report.clone())
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            Err(HostConnectorError::Invalid(
                "proxy_get_json is not supported for ssh hosts".into(),
            ))
        }
        async fn info(&self) -> Result<crate::host::connector::HostInfo, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn launch_run(
            &self,
            _req: crate::launcher::LaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn launch_agent(
            &self,
            _req: crate::agent_launcher::AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn start_session(
            &self,
            _req: crate::session_starter::SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn send_session_turn(
            &self,
            _req: crate::session_sender::SendMessageRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn list_runs(
            &self,
            _params: crate::host::connector::RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn reject_run(
            &self,
            _run_id: &str,
            _reason: Option<&str>,
        ) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn cancel_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn stream_run_events(
            &self,
            _run_id: &str,
        ) -> Result<crate::host::connector::EventByteStream, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
    }

    fn sample_report() -> serde_json::Value {
        serde_json::json!({
            "summary": {
                "total_input_tokens": 100, "total_output_tokens": 50,
                "total_cached_tokens": 10, "total_tokens": 150,
                "total_runs": 2, "total_cost_usd": 1.25, "cost_partial": false
            },
            "rows": [
                {"group": "opus", "model": "opus", "input_tokens": 60,
                 "output_tokens": 30, "cached_tokens": 6, "runs": 1,
                 "cost_usd": 0.75, "cost_partial": false},
                {"group": "sonnet", "model": "sonnet", "input_tokens": 40,
                 "output_tokens": 20, "cached_tokens": 4, "runs": 1,
                 "cost_usd": 0.5, "cost_partial": false}
            ]
        })
    }

    /// The behaviour change this task exists for: an SSH host used to report
    /// `unavailable` and contribute nothing, because `proxy_get_json` is the
    /// only surface the fan-out knew about. It now answers through
    /// `usage_rollup` and contributes real numbers.
    #[tokio::test]
    async fn a_host_without_generic_get_still_reports_usage_through_the_rollup() {
        let conn = RollupOnlyConnector {
            report: sample_report(),
        };

        let body = remote_usage_body(
            &conn,
            "/api/usage?host=local",
            Utc::now() - chrono::Duration::days(30),
            Utc::now(),
            crate::usage::GroupBy::Model,
        )
        .await
        .expect("the rollup surface answers; no transport failure")
        .expect("and this build can read its body");

        assert_eq!(body.summary.total_tokens, 150);
        assert_eq!(body.breakdown.len(), 2, "grouped by model, two models");
    }

    /// `group_by=host` wants ONE row per host. The remote CLI has no `host`
    /// grouping, so the rows it returns are collapsed back to the host's own
    /// totals rather than leaking a per-model breakdown into a host pivot.
    #[tokio::test]
    async fn host_grouping_collapses_the_remote_rows_to_a_single_total() {
        let conn = RollupOnlyConnector {
            report: sample_report(),
        };

        let body = remote_usage_body(
            &conn,
            "/api/usage?host=local",
            Utc::now() - chrono::Duration::days(30),
            Utc::now(),
            crate::usage::GroupBy::Host,
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(body.breakdown.len(), 1, "one row for this whole host");
        assert_eq!(body.breakdown[0].total_tokens, 150, "the host's total");
        assert_eq!(body.breakdown[0].runs, 2);
    }

    /// `group_by=project` has no remote-CLI equivalent. The host must say so
    /// and render `unavailable` — never quietly report a different pivot's
    /// numbers under the project label.
    #[tokio::test]
    async fn project_grouping_is_unsupported_rather_than_silently_wrong() {
        let conn = RollupOnlyConnector {
            report: sample_report(),
        };

        let err = remote_usage_body(
            &conn,
            "/api/usage?host=local",
            Utc::now() - chrono::Duration::days(30),
            Utc::now(),
            crate::usage::GroupBy::Project,
        )
        .await
        .expect_err("no surface can serve a project pivot on this transport");

        assert!(
            matches!(err, HostConnectorError::Invalid(_)),
            "falls through to the generic GET, which this transport refuses: {err:?}"
        );
    }

    /// A body this build cannot read is an error the caller renders as an
    /// offline host with a reason — never a silent zero contribution.
    #[test]
    fn remote_usage_report_without_a_summary_is_an_error() {
        let report = serde_json::json!({ "rows": [] });
        assert!(usage_body_from_remote_report(&report).is_err());
    }

    use super::*;

    #[test]
    fn resolve_window_defaults_to_30_days() {
        let now = DateTime::parse_from_rfc3339("2026-06-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (start, end) = resolve_window(None, None, now).unwrap();
        assert_eq!(end, now);
        assert_eq!(start, now - Duration::days(30));
    }

    #[test]
    fn resolve_window_parses_explicit_bounds() {
        let now = Utc::now();
        let (start, end) = resolve_window(
            Some("2026-01-01T00:00:00Z"),
            Some("2026-02-01T00:00:00Z"),
            now,
        )
        .unwrap();
        assert_eq!(
            start,
            DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        assert_eq!(
            end,
            DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn resolve_window_rejects_garbage() {
        let now = Utc::now();
        assert!(resolve_window(Some("not-a-date"), None, now).is_err());
    }

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn urow(provider: &str, model: &str, input: u64, output: u64) -> rupu_transcript::UsageRow {
        rupu_transcript::UsageRow {
            provider: provider.into(),
            model: model.into(),
            agent: "a".into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            runs: 1,
            ..rupu_transcript::UsageRow::default()
        }
    }

    #[test]
    fn bucket_key_day_is_the_calendar_day() {
        assert_eq!(
            bucket_key(dt("2026-06-24T13:45:00Z"), Granularity::Day),
            "2026-06-24"
        );
    }

    #[test]
    fn bucket_key_week_is_the_iso_monday() {
        // 2026-06-24 is a Wednesday; its ISO week starts Monday 2026-06-22.
        assert_eq!(
            bucket_key(dt("2026-06-24T13:45:00Z"), Granularity::Week),
            "2026-06-22"
        );
        // A Monday maps to itself.
        assert_eq!(
            bucket_key(dt("2026-06-22T00:00:00Z"), Granularity::Week),
            "2026-06-22"
        );
        // A Sunday maps back to the prior Monday.
        assert_eq!(
            bucket_key(dt("2026-06-28T23:59:00Z"), Granularity::Week),
            "2026-06-22"
        );
    }

    #[test]
    fn granularity_parse_accepts_day_week_default_and_rejects_other() {
        assert_eq!(Granularity::parse(None).unwrap(), Granularity::Day);
        assert_eq!(Granularity::parse(Some("day")).unwrap(), Granularity::Day);
        assert_eq!(Granularity::parse(Some("week")).unwrap(), Granularity::Week);
        assert!(Granularity::parse(Some("bogus")).is_err());
    }

    #[test]
    fn timeline_fill_start_clamps_to_window_then_first_run() {
        let start = dt("2026-06-01T00:00:00Z");
        // No runs at all → None (caller returns an empty series).
        assert_eq!(timeline_fill_start(start, None), None);
        // First run older than the window → clamp up to the window start.
        assert_eq!(
            timeline_fill_start(start, Some(dt("2026-05-10T00:00:00Z"))),
            Some(start)
        );
        // First run inside the window → start at the first run (no flat lead-in).
        let first = dt("2026-06-10T08:00:00Z");
        assert_eq!(timeline_fill_start(start, Some(first)), Some(first));
    }

    #[test]
    fn enumerate_bucket_keys_daily_is_inclusive_continuous() {
        let keys = enumerate_bucket_keys(
            dt("2026-06-10T23:00:00Z"),
            dt("2026-06-13T01:00:00Z"),
            Granularity::Day,
        );
        assert_eq!(
            keys,
            vec!["2026-06-10", "2026-06-11", "2026-06-12", "2026-06-13"]
        );
    }

    #[test]
    fn enumerate_bucket_keys_weekly_snaps_to_monday_and_steps_weeks() {
        // 2026-06-24 is a Wednesday (week of 06-22); end 2026-07-06 is a Monday.
        let keys = enumerate_bucket_keys(
            dt("2026-06-24T10:00:00Z"),
            dt("2026-07-06T10:00:00Z"),
            Granularity::Week,
        );
        assert_eq!(keys, vec!["2026-06-22", "2026-06-29", "2026-07-06"]);
    }

    #[test]
    fn build_timeline_gap_fills_empty_days_between_and_after_runs() {
        let pricing = rupu_config::PricingConfig::default();
        let runs = vec![
            (
                dt("2026-06-10T10:00:00Z"),
                vec![urow("anthropic", "claude-sonnet-4-6", 1000, 0)],
            ),
            (
                dt("2026-06-12T10:00:00Z"),
                vec![urow("anthropic", "claude-sonnet-4-6", 2000, 0)],
            ),
        ];
        // Fill from the first run through 06-15 (e.g. "now").
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Day,
            dt("2026-06-10T10:00:00Z"),
            dt("2026-06-15T00:00:00Z"),
        );
        let keys: Vec<&str> = buckets.iter().map(|b| b.bucket.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "2026-06-10",
                "2026-06-11",
                "2026-06-12",
                "2026-06-13",
                "2026-06-14",
                "2026-06-15"
            ]
        );
        // Populated days carry rows; gap days are empty.
        assert!(!buckets[0].rows.is_empty()); // 06-10
        assert!(buckets[1].rows.is_empty()); // 06-11
        assert!(!buckets[2].rows.is_empty()); // 06-12
        assert!(buckets[3].rows.is_empty()); // 06-13
        assert!(buckets[5].rows.is_empty()); // 06-15 (reaches the end)
        assert_eq!(buckets[2].rows[0].input_tokens, 2000);
    }

    #[test]
    fn build_timeline_reaches_end_when_no_recent_activity() {
        let pricing = rupu_config::PricingConfig::default();
        let runs = vec![(
            dt("2026-06-10T10:00:00Z"),
            vec![urow("anthropic", "claude-sonnet-4-6", 1000, 0)],
        )];
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Day,
            dt("2026-06-10T10:00:00Z"),
            dt("2026-06-13T00:00:00Z"),
        );
        assert_eq!(buckets.len(), 4); // 06-10..06-13 inclusive
        assert_eq!(buckets.last().unwrap().bucket, "2026-06-13");
        assert!(buckets.last().unwrap().rows.is_empty());
    }

    #[test]
    fn build_timeline_buckets_by_day_with_per_model_breakdown() {
        let pricing = rupu_config::PricingConfig::default();
        let runs = vec![
            (
                dt("2026-06-24T10:00:00Z"),
                vec![
                    urow("anthropic", "claude-sonnet-4-6", 1_000_000, 0),
                    urow("internal-vllm", "llama-3-70b", 100, 50),
                ],
            ),
            (
                dt("2026-06-24T20:00:00Z"),
                vec![urow("anthropic", "claude-sonnet-4-6", 1_000_000, 0)],
            ),
            (
                dt("2026-06-25T09:00:00Z"),
                vec![urow("anthropic", "claude-sonnet-4-6", 500_000, 0)],
            ),
        ];

        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Day,
            dt("2026-06-24T10:00:00Z"),
            dt("2026-06-25T09:00:00Z"),
        );
        assert_eq!(buckets.len(), 2);
        // Chronological order.
        assert_eq!(buckets[0].bucket, "2026-06-24");
        assert_eq!(buckets[1].bucket, "2026-06-25");

        // Day 1: two models, sonnet rows summed across both runs.
        let d1 = &buckets[0];
        let sonnet = d1
            .rows
            .iter()
            .find(|r| r.model == "claude-sonnet-4-6")
            .unwrap();
        assert_eq!(sonnet.input_tokens, 2_000_000);
        assert_eq!(sonnet.runs, 2);
        assert!((sonnet.cost_usd.unwrap() - 6.0).abs() < 1e-9); // 2M * $3/M
        let llama = d1.rows.iter().find(|r| r.model == "llama-3-70b").unwrap();
        assert_eq!(llama.input_tokens, 100);
        assert_eq!(llama.output_tokens, 50);
        assert!(!llama.priced);

        // Day 2: single model.
        let d2 = &buckets[1];
        assert_eq!(d2.rows.len(), 1);
        assert_eq!(d2.rows[0].model, "claude-sonnet-4-6");
        assert_eq!(d2.rows[0].input_tokens, 500_000);
    }

    #[test]
    fn build_timeline_week_collapses_days_into_one_bucket() {
        let pricing = rupu_config::PricingConfig::default();
        let runs = vec![
            (
                dt("2026-06-24T10:00:00Z"), // Wednesday
                vec![urow("anthropic", "claude-sonnet-4-6", 1000, 0)],
            ),
            (
                dt("2026-06-25T10:00:00Z"), // Thursday, same ISO week
                vec![urow("anthropic", "claude-sonnet-4-6", 2000, 0)],
            ),
        ];
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Week,
            dt("2026-06-24T10:00:00Z"),
            dt("2026-06-25T10:00:00Z"),
        );
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].bucket, "2026-06-22");
        assert_eq!(buckets[0].rows[0].input_tokens, 3000);
    }

    #[test]
    fn build_timeline_week_gap_fills_intervening_empty_week() {
        // 2026-06-10 is a Wednesday → ISO week Mon = 2026-06-08.
        // 2026-06-24 is a Wednesday → ISO week Mon = 2026-06-22.
        // The intervening Monday 2026-06-15 has no run and must be synthesised
        // as an empty bucket so the series has no gaps.
        let pricing = rupu_config::PricingConfig::default();
        let runs = vec![
            (
                dt("2026-06-10T10:00:00Z"),
                vec![urow("anthropic", "claude-sonnet-4-6", 1000, 200)],
            ),
            (
                dt("2026-06-24T10:00:00Z"),
                vec![urow("anthropic", "claude-sonnet-4-6", 3000, 400)],
            ),
        ];
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Week,
            dt("2026-06-10T10:00:00Z"),
            dt("2026-06-24T10:00:00Z"),
        );

        let keys: Vec<&str> = buckets.iter().map(|b| b.bucket.as_str()).collect();
        assert_eq!(keys, vec!["2026-06-08", "2026-06-15", "2026-06-22"]);

        // First week (2026-06-08) has the run from 2026-06-10 → non-empty.
        assert!(!buckets[0].rows.is_empty());
        assert_eq!(buckets[0].rows[0].input_tokens, 1000);

        // Middle week (2026-06-15) has no run → synthesised empty bucket.
        assert!(buckets[1].rows.is_empty());

        // Last week (2026-06-22) has the run from 2026-06-24 → non-empty.
        assert!(!buckets[2].rows.is_empty());
        assert_eq!(buckets[2].rows[0].input_tokens, 3000);
    }

    /// Write a two-line transcript: `RunStart` (anchors provider/model/agent)
    /// followed by one `Usage` event carrying `input_tokens`.
    fn write_run_transcript(path: &std::path::Path, agent: &str, input_tokens: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let start = rupu_transcript::Event::RunStart {
            run_id: "r".into(),
            workspace_id: "ws".into(),
            agent: agent.into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            started_at: Utc::now(),
            mode: rupu_transcript::RunMode::Ask,
            schema: None,
            system_prompt: None,
        };
        let usage = rupu_transcript::Event::Usage {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            served_model: None,
            input_tokens,
            output_tokens: 0,
            cached_tokens: 0,
        };
        let mut buf = Vec::new();
        for ev in [&start, &usage] {
            let mut line = serde_json::to_vec(ev).unwrap();
            line.push(b'\n');
            buf.extend(line);
        }
        std::fs::write(path, &buf).unwrap();
    }

    /// Register a run of `workflow_name` bound to `workspace_id`, with one
    /// completed step whose transcript reports `input_tokens` of usage.
    fn seed_workflow_run(
        s: &AppState,
        run_id: &str,
        workflow_name: &str,
        workspace_id: &str,
        transcript_path: &std::path::Path,
        input_tokens: u32,
    ) {
        let record = rupu_orchestrator::RunRecord {
            id: run_id.into(),
            workflow_name: workflow_name.into(),
            status: rupu_orchestrator::RunStatus::Completed,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: workspace_id.into(),
            workspace_path: std::path::PathBuf::from("/tmp/proj"),
            transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
        };
        s.run_store.create(record, "name: wf\n").unwrap();
        write_run_transcript(transcript_path, "reviewer", input_tokens);
        s.run_store
            .append_step_result(
                run_id,
                &rupu_orchestrator::runs::StepResultRecord {
                    run_outcome: None,
                    step_id: "s1".into(),
                    run_id: run_id.into(),
                    transcript_path: transcript_path.to_path_buf(),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: rupu_orchestrator::runs::StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: Utc::now(),
                    loop_iteration: None,
                },
            )
            .unwrap();
    }

    #[tokio::test]
    async fn get_usage_attributes_rows_to_workflow_inline_from_the_run_in_hand() {
        // Two runs under two different workflows. Before attribution both
        // rows' `workflow` field is blank and would collapse into a single
        // bucket under GroupBy::Workflow — this is the failing case the
        // inline join in `get_usage` must fix.
        let tmp = tempfile::TempDir::new().unwrap();
        let s = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );

        seed_workflow_run(
            &s,
            "run_1",
            "nightly-review",
            "ws_a",
            &tmp.path().join("t1.jsonl"),
            1000,
        );
        seed_workflow_run(
            &s,
            "run_2",
            "hotfix",
            "ws_b",
            &tmp.path().join("t2.jsonl"),
            500,
        );

        let Json(resp) = get_usage(
            State(s),
            Query(UsageQuery {
                since: None,
                until: None,
                group_by: Some("workflow".into()),
                host: None,
            }),
        )
        .await
        .expect("handler should not error");

        assert_eq!(
            resp.breakdown.len(),
            2,
            "two distinct workflows must not collapse into one bucket: {:?}",
            resp.breakdown
        );
        let nightly = resp
            .breakdown
            .iter()
            .find(|r| r.workflow == "nightly-review")
            .expect("nightly-review row present");
        assert_eq!(nightly.input_tokens, 1000);
        let hotfix = resp
            .breakdown
            .iter()
            .find(|r| r.workflow == "hotfix")
            .expect("hotfix row present");
        assert_eq!(hotfix.input_tokens, 500);
    }

    #[tokio::test]
    async fn get_usage_group_by_project_attributes_from_workspace_id_not_a_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );

        seed_workflow_run(
            &s,
            "run_1",
            "nightly-review",
            "ws_a",
            &tmp.path().join("t1.jsonl"),
            1000,
        );
        seed_workflow_run(
            &s,
            "run_2",
            "hotfix",
            "ws_b",
            &tmp.path().join("t2.jsonl"),
            500,
        );

        let Json(resp) = get_usage(
            State(s),
            Query(UsageQuery {
                since: None,
                until: None,
                group_by: Some("project".into()),
                host: None,
            }),
        )
        .await
        .expect("handler should not error");

        assert_eq!(resp.breakdown.len(), 2);
        assert!(resp
            .breakdown
            .iter()
            .any(|r| r.workspace_id == "ws_a" && r.input_tokens == 1000));
        assert!(resp
            .breakdown
            .iter()
            .any(|r| r.workspace_id == "ws_b" && r.input_tokens == 500));
    }

    #[tokio::test]
    async fn get_usage_timeline_returns_empty_vec_when_store_has_no_runs() {
        // AppState::new over a fresh tempdir → RunStore::list returns Ok(vec![])
        // → timeline_fill_start(_, None) returns None → handler short-circuits
        // with Ok(Json(vec![])).
        let tmp = tempfile::TempDir::new().unwrap();
        let s = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        let result = get_usage_timeline(
            State(s),
            Query(TimelineQuery {
                since: None,
                until: None,
                bucket: None,
            }),
        )
        .await
        .expect("handler should not error on empty store");
        assert!(
            result.0.is_empty(),
            "expected empty timeline for empty store"
        );
    }
}
