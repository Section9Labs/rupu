//! Time values of a [`crate::FieldKind::Time`] field: a span back from now
//! (`30m`, `12h`, `7d`, `2w`), a UTC date (`2026-10-01`) or an RFC 3339 time.
//! A span is resolved against an injected `now` at match time, so a parsed
//! query means the same thing however long it is held.

use chrono::{DateTime, Duration, NaiveDate, Utc};

/// One parsed time value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeValue {
    /// This long before now.
    Ago(Duration),
    /// A whole UTC day.
    Day(NaiveDate),
    /// An instant.
    At(DateTime<Utc>),
}

impl TimeValue {
    pub fn parse(v: &str) -> Option<Self> {
        if let Some(d) = parse_span(v) {
            return Some(Self::Ago(d));
        }
        if let Ok(day) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
            return Some(Self::Day(day));
        }
        DateTime::parse_from_rfc3339(v)
            .ok()
            .map(|t| Self::At(t.with_timezone(&Utc)))
    }

    /// `since:` keeps a time at or after this value: a day from its start.
    pub fn admits_since(self, ts: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        match self {
            Self::Ago(d) => ts >= now - d,
            Self::Day(day) => ts >= day_start(day),
            Self::At(t) => ts >= t,
        }
    }

    /// `until:` keeps a time at or before this value: a day through its end.
    pub fn admits_until(self, ts: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        match self {
            Self::Ago(d) => ts <= now - d,
            Self::Day(day) => day.succ_opt().is_none_or(|next| ts < day_start(next)),
            Self::At(t) => ts <= t,
        }
    }
}

fn day_start(day: NaiveDate) -> DateTime<Utc> {
    day.and_hms_opt(0, 0, 0).expect("midnight exists").and_utc()
}

fn parse_span(v: &str) -> Option<Duration> {
    let (at, unit) = v.char_indices().last()?;
    let digits = &v[..at];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    match unit.to_ascii_lowercase() {
        'm' => Duration::try_minutes(n),
        'h' => Duration::try_hours(n),
        'd' => Duration::try_days(n),
        'w' => Duration::try_weeks(n),
        _ => None,
    }
}

/// Validate a time value at parse time: a span lowercased, anything else as
/// written.
pub fn normalize(v: &str) -> Result<String, String> {
    match TimeValue::parse(v) {
        Some(TimeValue::Ago(_)) => Ok(v.to_ascii_lowercase()),
        Some(_) => Ok(v.to_string()),
        None => Err(format!(
            "`{v}` is not a time (expected 30m, 12h, 7d, 2w, 2026-10-01 or an RFC 3339 time)"
        )),
    }
}

/// A row's timestamp: RFC 3339, else `None` (never matched by a bound).
pub fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        parse_timestamp(s).unwrap()
    }

    #[test]
    fn spans_dates_and_instants_parse() {
        assert_eq!(
            TimeValue::parse("30m"),
            Some(TimeValue::Ago(Duration::minutes(30)))
        );
        assert_eq!(
            TimeValue::parse("2W"),
            Some(TimeValue::Ago(Duration::weeks(2)))
        );
        assert_eq!(
            TimeValue::parse("2026-10-01"),
            Some(TimeValue::Day(
                NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
            ))
        );
        assert_eq!(
            TimeValue::parse("2026-10-01T12:00:00+02:00"),
            Some(TimeValue::At(at("2026-10-01T10:00:00Z")))
        );
        for bad in [
            "",
            "d",
            "7",
            "7y",
            "-7d",
            "+7d",
            "7.5h",
            "yesterday",
            "2026-13-01",
            "7é",
        ] {
            assert_eq!(TimeValue::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_span_is_measured_back_from_now() {
        let now = at("2026-10-14T12:00:00Z");
        let week = TimeValue::parse("7d").unwrap();
        assert!(week.admits_since(at("2026-10-07T12:00:00Z"), now));
        assert!(!week.admits_since(at("2026-10-07T11:59:59Z"), now));
        assert!(week.admits_until(at("2026-10-07T12:00:00Z"), now));
        assert!(!week.admits_until(at("2026-10-07T12:00:01Z"), now));
    }

    #[test]
    fn a_date_covers_its_whole_utc_day() {
        let now = at("2026-10-14T12:00:00Z");
        let day = TimeValue::parse("2026-10-01").unwrap();
        assert!(day.admits_since(at("2026-10-01T00:00:00Z"), now));
        assert!(!day.admits_since(at("2026-09-30T23:59:59Z"), now));
        assert!(day.admits_until(at("2026-10-01T23:59:59Z"), now));
        assert!(!day.admits_until(at("2026-10-02T00:00:00Z"), now));
    }

    #[test]
    fn normalize_lowercases_spans_and_names_a_bad_value() {
        assert_eq!(normalize("12H"), Ok("12h".to_string()));
        assert_eq!(normalize("2026-10-01"), Ok("2026-10-01".to_string()));
        assert_eq!(
            normalize("soon"),
            Err(
                "`soon` is not a time (expected 30m, 12h, 7d, 2w, 2026-10-01 or an RFC 3339 time)"
                    .to_string()
            )
        );
    }
}
