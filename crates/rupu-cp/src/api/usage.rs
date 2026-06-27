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
    let group_by = crate::usage::GroupBy::parse(q.group_by.as_deref().unwrap_or("model"));

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
        all_rows.extend(rupu_transcript::aggregate(
            &paths,
            rupu_transcript::TimeWindow::default(),
        ));
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
            Some(other) => Err(format!("invalid bucket {other:?}: expected \"day\" or \"week\"")),
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
        Granularity::Week => {
            date - Duration::days(date.weekday().num_days_from_monday() as i64)
        }
    };
    date.format("%Y-%m-%d").to_string()
}

/// Group per-run `(started_at, rows)` by bucket key, then break each bucket down
/// per model. Returns buckets sorted chronologically (the `YYYY-MM-DD` key sorts
/// lexicographically into chronological order).
fn build_timeline(
    runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)>,
    pricing: &rupu_config::PricingConfig,
    granularity: Granularity,
) -> Vec<UsageTimelineBucket> {
    let mut grouped: std::collections::BTreeMap<String, Vec<rupu_transcript::UsageRow>> =
        std::collections::BTreeMap::new();
    for (started_at, rows) in runs_with_rows {
        let key = bucket_key(started_at, granularity);
        grouped.entry(key).or_default().extend(rows);
    }
    grouped
        .into_iter()
        .map(|(bucket, rows)| UsageTimelineBucket {
            rows: crate::usage::breakdown(&rows, pricing, crate::usage::GroupBy::Model),
            bucket,
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

    let mut runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)> = Vec::new();
    for r in runs
        .iter()
        .filter(|r| r.started_at >= start && r.started_at <= end)
    {
        let paths = crate::usage::run_transcript_paths(&s.run_store, &r.id);
        let rows = rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default());
        runs_with_rows.push((r.started_at, rows));
    }

    Ok(Json(build_timeline(runs_with_rows, &s.pricing, granularity)))
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
        DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&Utc)
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

        let buckets = build_timeline(runs, &pricing, Granularity::Day);
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
        let buckets = build_timeline(runs, &pricing, Granularity::Week);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].bucket, "2026-06-22");
        assert_eq!(buckets[0].rows[0].input_tokens, 3000);
    }
}
