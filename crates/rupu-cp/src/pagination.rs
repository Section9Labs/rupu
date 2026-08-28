//! Shared offset/limit pagination for the list endpoints.
//!
//! Query params are lenient: a missing or unparseable bound falls back to the
//! default (offset 0, limit 20) rather than erroring, so a bad query string
//! never 500s a list. `limit` is clamped to `[1, 200]`.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Default page size when `limit` is absent.
pub const DEFAULT_LIMIT: usize = 20;
/// Hard cap on `limit`.
pub const MAX_LIMIT: usize = 200;

/// Optional `?offset=&limit=` query params for a list endpoint.
#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

impl PageQuery {
    /// Resolved offset (default 0).
    pub fn offset(&self) -> usize {
        self.offset.unwrap_or(0)
    }
    /// Resolved limit (default `DEFAULT_LIMIT`, clamped to `[1, MAX_LIMIT]`).
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
}

/// Slice `items` to the `[offset, offset+limit)` window. Out-of-range offset
/// yields an empty vec. Consumes the input so handlers can compute expensive
/// per-row work on the returned page only.
pub fn paginate<T>(items: Vec<T>, page: &PageQuery) -> Vec<T> {
    let offset = page.offset();
    let limit = page.limit();
    items.into_iter().skip(offset).take(limit).collect()
}

/// Optional `?since=&until=` RFC-3339 query params (perf & interaction arc,
/// Plan 5 Task 5) narrowing a list to a date range on the row's own
/// started/created timestamp — applied by callers BEFORE [`paginate`], so
/// the offset/limit window walks the already-narrowed set rather than a
/// slice-then-filter that would silently return short or empty pages.
///
/// **Boundary is closed at both ends**: `since <= ts <= until`. This matches
/// `crate::api::usage::resolve_window`'s existing `[start, end]` convention
/// (`filter(|r| r.started_at >= start && r.started_at <= end)` in
/// `local_usage`) — a row stamped exactly at `until` is included, not
/// excluded, and a caller building `until` from a date picker's "last day"
/// should pass that day's `23:59:59.999Z` (the same convention the web's
/// `windowFromDayRange` already uses for `/api/usage*`) to make the window
/// cover the whole day rather than stopping at its first instant.
///
/// **Lenient, never a 400/500** — deliberately different from
/// `resolve_window`'s stricter "unparseable bound is an error" contract:
/// these are list endpoints, and `pagination.rs`'s own discipline (a bad
/// `offset`/`limit` degrades to the default rather than erroring) applies
/// here too. A present-but-unparseable `since`/`until` string degrades to
/// "no bound on that side" rather than failing the request.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct DateRangeQuery {
    pub since: Option<String>,
    pub until: Option<String>,
}

fn parse_rfc3339_lenient(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

impl DateRangeQuery {
    /// The parsed lower bound, or `None` when absent or unparseable.
    pub fn since(&self) -> Option<DateTime<Utc>> {
        self.since.as_deref().and_then(parse_rfc3339_lenient)
    }

    /// The parsed upper bound, or `None` when absent or unparseable.
    pub fn until(&self) -> Option<DateTime<Utc>> {
        self.until.as_deref().and_then(parse_rfc3339_lenient)
    }

    /// True when at least one bound actually parsed. A range that isn't
    /// active must never narrow a list at all — see `contains_str`'s doc
    /// comment for why this matters beyond just "nothing to filter."
    pub fn is_active(&self) -> bool {
        self.since().is_some() || self.until().is_some()
    }

    /// True when `ts` falls in `[since, until]` — whichever bounds actually
    /// parsed; an absent/unparseable bound imposes no constraint on that
    /// side. For a row whose own timestamp is a non-optional
    /// `DateTime<Utc>` (e.g. `RunRecord::started_at`).
    pub fn contains(&self, ts: DateTime<Utc>) -> bool {
        let after_since = match self.since() {
            Some(s) => ts >= s,
            None => true,
        };
        let before_until = match self.until() {
            Some(u) => ts <= u,
            None => true,
        };
        after_since && before_until
    }

    /// Same as [`contains`](Self::contains), but reads the row's timestamp
    /// as an `Option<&str>` — the shape most list-endpoint rows carry
    /// (`AgentRunRow::started_at`, `AutoflowEventRow::at`, a session's JSON
    /// `created_at`, ...).
    ///
    /// When this range isn't active at all (`is_active() == false`), every
    /// row passes regardless of whether its own timestamp is present or
    /// parses — a range filter nobody asked for must never start hiding
    /// rows just because their timestamp happens to be absent or malformed.
    /// Once a bound genuinely IS set, a row whose timestamp is missing or
    /// fails to parse is **excluded**: there is no honest basis to claim an
    /// unknown timestamp falls inside an operator-chosen window.
    pub fn contains_str(&self, ts: Option<&str>) -> bool {
        if !self.is_active() {
            return true;
        }
        match ts.and_then(parse_rfc3339_lenient) {
            Some(dt) => self.contains(dt),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(offset: Option<usize>, limit: Option<usize>) -> PageQuery {
        PageQuery { offset, limit }
    }

    #[test]
    fn default_limit_is_20() {
        let items: Vec<u32> = (0..50).collect();
        let page = paginate(items, &q(None, None));
        assert_eq!(page.len(), 20);
        assert_eq!(page[0], 0);
        assert_eq!(page[19], 19);
    }

    #[test]
    fn offset_and_limit_slice() {
        let items: Vec<u32> = (0..50).collect();
        let page = paginate(items, &q(Some(20), Some(5)));
        assert_eq!(page, vec![20, 21, 22, 23, 24]);
    }

    #[test]
    fn offset_past_end_is_empty() {
        let items: Vec<u32> = (0..10).collect();
        assert!(paginate(items, &q(Some(100), Some(20))).is_empty());
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(q(None, Some(0)).limit(), 1);
        assert_eq!(q(None, Some(9999)).limit(), MAX_LIMIT);
        assert_eq!(q(None, Some(50)).limit(), 50);
    }

    // ── DateRangeQuery ────────────────────────────────────────────────────

    fn dr(since: Option<&str>, until: Option<&str>) -> DateRangeQuery {
        DateRangeQuery {
            since: since.map(str::to_string),
            until: until.map(str::to_string),
        }
    }

    #[test]
    fn inactive_range_admits_everything_including_unparseable_timestamps() {
        let range = dr(None, None);
        assert!(!range.is_active());
        assert!(range.contains_str(Some("2026-08-15T00:00:00Z")));
        assert!(range.contains_str(Some("not a timestamp")));
        assert!(range.contains_str(None));
    }

    #[test]
    fn boundary_is_closed_inclusive_on_both_ends() {
        let range = dr(Some("2026-08-01T00:00:00Z"), Some("2026-08-26T23:59:59Z"));
        // Exactly at `since` — included.
        assert!(range.contains_str(Some("2026-08-01T00:00:00Z")));
        // Exactly at `until` — included (closed, not half-open).
        assert!(range.contains_str(Some("2026-08-26T23:59:59Z")));
        // One second before `since` / after `until` — excluded.
        assert!(!range.contains_str(Some("2026-07-31T23:59:59Z")));
        assert!(!range.contains_str(Some("2026-08-27T00:00:00Z")));
        // Comfortably inside — included.
        assert!(range.contains_str(Some("2026-08-15T12:00:00Z")));
    }

    #[test]
    fn since_only_has_no_upper_bound() {
        let range = dr(Some("2026-08-01T00:00:00Z"), None);
        assert!(range.contains_str(Some("2099-01-01T00:00:00Z")));
        assert!(!range.contains_str(Some("2026-07-31T23:59:59Z")));
    }

    #[test]
    fn until_only_has_no_lower_bound() {
        let range = dr(None, Some("2026-08-26T23:59:59Z"));
        assert!(range.contains_str(Some("2000-01-01T00:00:00Z")));
        assert!(!range.contains_str(Some("2026-08-27T00:00:00Z")));
    }

    #[test]
    fn active_range_excludes_missing_or_unparseable_timestamps() {
        let range = dr(Some("2026-08-01T00:00:00Z"), None);
        assert!(!range.contains_str(None));
        assert!(!range.contains_str(Some("garbage")));
    }

    #[test]
    fn bad_bound_degrades_to_no_constraint_never_errors() {
        // A malformed `since`/`until` string must never panic or propagate
        // an error — it just stops constraining that side, exactly like
        // `PageQuery`'s malformed offset/limit degrading to defaults.
        let range = dr(Some("not-a-date"), Some("also-not-a-date"));
        assert!(!range.is_active());
        assert!(range.contains_str(Some("2026-08-15T00:00:00Z")));
    }
}
