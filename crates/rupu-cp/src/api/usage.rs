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
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/usage", get(get_usage))
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
}
