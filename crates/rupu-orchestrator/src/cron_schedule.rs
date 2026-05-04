//! Cron schedule evaluation for `trigger.on: cron` workflows.
//!
//! Pure functions only — the actual `rupu cron tick` subcommand
//! lives in `rupu-cli`. The contract is:
//!
//! 1. Parse the workflow's `cron:` expression with [`parse_schedule`].
//!    The orchestrator's parse-time validation already caught
//!    obviously-malformed strings; this layer takes the live `cron`
//!    crate's stricter view.
//!
//! 2. On each tick, [`should_fire`] decides whether the schedule
//!    "wanted to fire" between the last successful tick and now. The
//!    caller persists `last_fired` per workflow under
//!    `<global>/cron-state/<workflow>.last_fired`.
//!
//! ## Cron-expression dialect
//!
//! The `cron` crate accepts 6- and 7-field expressions (with seconds
//! and an optional year); we restrict to the standard 5-field
//! `min hour dom month dow` form to match what users put in
//! `crontab(5)` and what our schema validator already accepts. We
//! transform 5-field input into the 6-field form by prefixing `0 ` so
//! the schedule fires at second 0 of the minute.
//!
//! ## "Should fire" semantics
//!
//! Given a `last_fired` timestamp (or `None` for "never fired") and a
//! current `now`, the schedule fires iff there exists at least one
//! cron-firing time `t` such that `last_fired < t <= now`. When
//! `last_fired` is `None` we substitute `now - 1 minute` so the very
//! first invocation can fire if the schedule matches the current
//! minute. This mirrors what a tick loop running at 1-minute
//! granularity from `crontab` would naturally do.

use chrono::{DateTime, Duration, Utc};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CronError {
    #[error("invalid cron expression `{expr}`: {reason}")]
    Invalid { expr: String, reason: String },
}

/// Parse a 5-field cron expression into a [`cron::Schedule`].
///
/// The `cron` crate natively expects 6 or 7 fields (it adds `seconds`
/// at the front). We accept the standard 5-field form and prefix
/// `0 ` so the schedule fires at second 0 of the matching minute.
pub fn parse_schedule(expr: &str) -> Result<cron::Schedule, CronError> {
    let trimmed = expr.trim();
    let field_count = trimmed.split_whitespace().count();
    let augmented = match field_count {
        5 => format!("0 {trimmed}"),
        6 | 7 => trimmed.to_string(),
        other => {
            return Err(CronError::Invalid {
                expr: expr.to_string(),
                reason: format!("expected 5 (or 6/7) fields, got {other}"),
            });
        }
    };
    cron::Schedule::from_str(&augmented).map_err(|e| CronError::Invalid {
        expr: expr.to_string(),
        reason: e.to_string(),
    })
}

/// Decide whether `schedule` "wanted to fire" between `last_fired`
/// (exclusive) and `now` (inclusive). When `last_fired` is `None`,
/// treat it as `now - 1 minute` so the first tick after process start
/// can still fire on the current minute's match.
pub fn should_fire(
    schedule: &cron::Schedule,
    last_fired: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    let after = last_fired.unwrap_or_else(|| now - Duration::minutes(1));
    schedule
        .after(&after)
        .next()
        .is_some_and(|next| next <= now)
}

/// Best-effort next firing time (used by `rupu cron list` to show
/// when each workflow will next fire). Returns `None` if the schedule
/// has no future firings (cron's year wildcard rules out this case
/// for any standard expression, but the API can still return None for
/// an unsatisfiable combination of fields).
pub fn next_fire_after(schedule: &cron::Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    schedule.after(&after).next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn parse_accepts_5_field_form() {
        let s = parse_schedule("0 4 * * *").expect("parse");
        // Sanity: next firing after 03:00 UTC should be 04:00 UTC.
        let next = s
            .after(&Utc.with_ymd_and_hms(2026, 1, 1, 3, 0, 0).unwrap())
            .next()
            .unwrap();
        assert_eq!(next.hour(), 4);
        assert_eq!(next.minute(), 0);
    }

    #[test]
    fn parse_accepts_6_field_form() {
        // `cron`'s native syntax — should pass through unchanged.
        let s = parse_schedule("0 0 4 * * *").expect("parse 6-field");
        let next = s
            .after(&Utc.with_ymd_and_hms(2026, 1, 1, 3, 0, 0).unwrap())
            .next()
            .unwrap();
        assert_eq!(next.hour(), 4);
    }

    #[test]
    fn parse_rejects_garbage() {
        let err = parse_schedule("hello world").unwrap_err();
        assert!(matches!(err, CronError::Invalid { .. }));
    }

    #[test]
    fn parse_rejects_empty() {
        let err = parse_schedule("").unwrap_err();
        assert!(matches!(err, CronError::Invalid { .. }));
    }

    #[test]
    fn should_fire_when_now_lands_on_schedule_with_no_prior_run() {
        // Every minute schedule, never fired — first tick fires.
        let s = parse_schedule("* * * * *").unwrap();
        let now = at("2026-05-04T12:34:00Z");
        assert!(should_fire(&s, None, now));
    }

    #[test]
    fn should_not_fire_when_now_is_before_first_match() {
        // Daily 04:00 schedule, ticking at 03:30 with no prior run.
        let s = parse_schedule("0 4 * * *").unwrap();
        let now = at("2026-05-04T03:30:00Z");
        assert!(!should_fire(&s, None, now));
    }

    #[test]
    fn should_fire_when_match_landed_between_last_and_now() {
        // Daily 04:00; last fired yesterday at 04:00; now 04:01 today.
        let s = parse_schedule("0 4 * * *").unwrap();
        let last = at("2026-05-03T04:00:00Z");
        let now = at("2026-05-04T04:01:00Z");
        assert!(should_fire(&s, Some(last), now));
    }

    #[test]
    fn should_not_fire_when_already_fired_for_this_match() {
        // Daily 04:00; last fired today at 04:00; now 04:30 — next
        // firing is tomorrow, so don't fire.
        let s = parse_schedule("0 4 * * *").unwrap();
        let last = at("2026-05-04T04:00:00Z");
        let now = at("2026-05-04T04:30:00Z");
        assert!(!should_fire(&s, Some(last), now));
    }

    #[test]
    fn should_fire_handles_multi_minute_drift() {
        // Tick missed the exact minute; 04:00 fired at 04:03 because
        // system cron was busy. Should still fire (we only fire once
        // because the next match isn't until tomorrow).
        let s = parse_schedule("0 4 * * *").unwrap();
        let last = at("2026-05-03T04:00:00Z");
        let now = at("2026-05-04T04:03:00Z");
        assert!(should_fire(&s, Some(last), now));
    }
}
