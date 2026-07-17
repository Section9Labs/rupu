//! `GET /api/usage` — global token + cost overview (summary + breakdown).

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/usage", get(get_usage))
        .route("/api/usage/timeline", get(get_usage_timeline))
}

#[derive(Debug, Deserialize)]
struct UsageQuery {
    since: Option<String>,
    until: Option<String>,
    group_by: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageResponse {
    summary: crate::usage::UsageSummary,
    breakdown: Vec<crate::usage::UsageBreakdownRow>,
}

/// Resolve the [since, until] window from optional RFC-3339 strings.
/// Absent `since` → now − 30 days; absent `until` → now. A present-but-unparseable
/// bound is an error (caller maps to 400) rather than a silent default.
fn resolve_window(
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

    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut all_rows: Vec<rupu_transcript::UsageRow> = Vec::new();
    for r in runs
        .iter()
        .filter(|r| r.started_at >= start && r.started_at <= end)
    {
        let paths = crate::usage::run_transcript_paths(&s.run_store, &r.id);
        let mut rows = rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default());
        // Attribute each row to the run it came from: `r` is already in hand
        // for this batch, so this is a free inline join — no re-load, no
        // cache, no separate `attribute_rows` function needed. `host_id` is
        // hardcoded "local" because this handler only ever reads the local
        // run store; the remote fan-out (multi-host attribution) is a
        // separate, later concern.
        for row in &mut rows {
            row.workflow = r.workflow_name.clone();
            row.workspace_id = r.workspace_id.clone();
            row.host_id = "local".to_string();
        }
        all_rows.extend(rows);
    }

    let summary = crate::usage::summarize(&all_rows, &s.pricing);
    let breakdown = crate::usage::breakdown(&all_rows, &s.pricing, group_by);
    Ok(Json(UsageResponse { summary, breakdown }))
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
        let paths = crate::usage::run_transcript_paths(&s.run_store, &r.id);
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

#[cfg(test)]
mod tests {
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
            final_output: None,
        };
        s.run_store.create(record, "name: wf\n").unwrap();
        write_run_transcript(transcript_path, "reviewer", input_tokens);
        s.run_store
            .append_step_result(
                run_id,
                &rupu_orchestrator::runs::StepResultRecord {
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
