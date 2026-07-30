//! Shared compact formatters for token counts and costs.
//!
//! Promoted out of `cmd/session.rs` so the status header and the live
//! workflow run view share one implementation. Keep these pure and
//! allocation-light: they're called on every render tick.

use chrono::{DateTime, Utc};

/// Format a token count with compact K / M units, e.g. `1.2M`, `45K`,
/// `980`. Values under 10 in each unit keep one decimal; larger values
/// round to whole units. Sub-1000 counts render verbatim.
pub fn format_token_compact(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        if m < 10.0 {
            format!("{m:.1}M")
        } else {
            format!("{m:.0}M")
        }
    } else if n >= 1_000 {
        let k = n as f64 / 1_000.0;
        if k < 10.0 {
            format!("{k:.1}K")
        } else {
            format!("{k:.0}K")
        }
    } else {
        n.to_string()
    }
}

/// Format a USD cost as `$3.40` (always two decimals). Used by the
/// live workflow dashboard's cost meter.
pub fn format_cost_compact(usd: f64) -> String {
    format!("${usd:.2}")
}

/// Seconds in each grade. Beyond a year, relative ages stop being
/// useful and we show the date instead.
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const YEAR: i64 = 365 * DAY;

/// Render how long ago `then` was, relative to `now`.
///
/// Grades: `just now` under a minute, then minutes, hours, days, and
/// weeks. Past one year it falls back to an absolute `YYYY-MM-DD`.
///
/// `now` is a parameter rather than a clock read so this is
/// deterministic under test. Future timestamps clamp to `just now`,
/// because clock skew between whatever wrote the record and this
/// process must not surface as a negative age.
pub fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds();
    if secs < MINUTE {
        return "just now".to_string();
    }
    if secs < HOUR {
        return format!("{}m ago", secs / MINUTE);
    }
    if secs < DAY {
        return format!("{}h ago", secs / HOUR);
    }
    if secs < WEEK {
        return format!("{}d ago", secs / DAY);
    }
    if secs < YEAR {
        return format!("{}w ago", secs / WEEK);
    }
    then.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_under_1k_render_verbatim() {
        assert_eq!(format_token_compact(0), "0");
        assert_eq!(format_token_compact(42), "42");
        assert_eq!(format_token_compact(999), "999");
    }

    #[test]
    fn tokens_in_thousands() {
        assert_eq!(format_token_compact(1_000), "1.0K");
        assert_eq!(format_token_compact(1_200), "1.2K");
        assert_eq!(format_token_compact(45_000), "45K");
        assert_eq!(format_token_compact(999_999), "1000K");
    }

    #[test]
    fn tokens_in_millions() {
        assert_eq!(format_token_compact(1_000_000), "1.0M");
        assert_eq!(format_token_compact(1_200_000), "1.2M");
        assert_eq!(format_token_compact(12_000_000), "12M");
    }

    #[test]
    fn cost_is_two_decimals() {
        assert_eq!(format_cost_compact(3.4), "$3.40");
        assert_eq!(format_cost_compact(0.0), "$0.00");
        assert_eq!(format_cost_compact(12.345), "$12.35");
    }

    use chrono::{TimeZone, Utc};

    fn at(secs_ago: i64) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
        let now = Utc.with_ymd_and_hms(2026, 7, 30, 12, 0, 0).unwrap();
        (now - chrono::Duration::seconds(secs_ago), now)
    }

    #[test]
    fn under_a_minute_is_just_now() {
        let (then, now) = at(0);
        assert_eq!(relative_time(then, now), "just now");
        let (then, now) = at(59);
        assert_eq!(relative_time(then, now), "just now");
    }

    #[test]
    fn minute_boundary() {
        let (then, now) = at(60);
        assert_eq!(relative_time(then, now), "1m ago");
        let (then, now) = at(3_540); // 59m
        assert_eq!(relative_time(then, now), "59m ago");
    }

    #[test]
    fn hour_boundary() {
        let (then, now) = at(3_600);
        assert_eq!(relative_time(then, now), "1h ago");
        let (then, now) = at(82_800); // 23h
        assert_eq!(relative_time(then, now), "23h ago");
    }

    #[test]
    fn day_boundary() {
        let (then, now) = at(86_400);
        assert_eq!(relative_time(then, now), "1d ago");
        let (then, now) = at(518_400); // 6d
        assert_eq!(relative_time(then, now), "6d ago");
    }

    #[test]
    fn week_boundary() {
        let (then, now) = at(604_800);
        assert_eq!(relative_time(then, now), "1w ago");
        let (then, now) = at(1_209_600);
        assert_eq!(relative_time(then, now), "2w ago");
    }

    #[test]
    fn beyond_a_year_falls_back_to_absolute_date() {
        // "63w ago" is not useful. Show the date instead.
        let (then, now) = at(31_536_000);
        assert_eq!(relative_time(then, now), "2025-07-30");
    }

    #[test]
    fn future_timestamps_clamp_to_just_now() {
        // Clock skew between the writer and this process is real and
        // must not render as a negative age.
        let (then, now) = at(-500);
        assert_eq!(relative_time(then, now), "just now");
    }
}
