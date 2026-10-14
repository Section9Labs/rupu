//! Time values of a [`crate::FieldKind::Time`] field: a span back from now
//! (`30m`, `12h`, `7d`, `2w`), a UTC date (`2026-10-01`) or an RFC 3339 time.
//! A span is resolved against an injected `now` at match time, so a parsed
//! query means the same thing however long it is held. A span is at most
//! [`MAX_SPAN_DAYS`], so measuring it back from any `now` stays a time.

use chrono::{DateTime, Duration, NaiveDate, Utc};

/// The longest span: 10 000 years.
pub const MAX_SPAN_DAYS: i64 = 3_652_425;

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
        match parse_span(v) {
            Some(Span::Within(d)) => return Some(Self::Ago(d)),
            Some(Span::TooLong) => return None,
            None => {}
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
            // Before the earliest time there is: every time is after it.
            Self::Ago(d) => now.checked_sub_signed(d).is_none_or(|t| ts >= t),
            Self::Day(day) => ts >= day_start(day),
            Self::At(t) => ts >= t,
        }
    }

    /// `until:` keeps a time at or before this value: a day through its end.
    pub fn admits_until(self, ts: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        match self {
            Self::Ago(d) => now.checked_sub_signed(d).is_some_and(|t| ts <= t),
            Self::Day(day) => day.succ_opt().is_none_or(|next| ts < day_start(next)),
            Self::At(t) => ts <= t,
        }
    }
}

fn day_start(day: NaiveDate) -> DateTime<Utc> {
    day.and_hms_opt(0, 0, 0).expect("midnight exists").and_utc()
}

/// A value written as a span: one within [`MAX_SPAN_DAYS`], or longer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    Within(Duration),
    TooLong,
}

/// `None` when `v` is not written as a span at all.
fn parse_span(v: &str) -> Option<Span> {
    let (at, unit) = v.char_indices().last()?;
    let digits = &v[..at];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let to_span: fn(i64) -> Option<Duration> = match unit.to_ascii_lowercase() {
        'm' => Duration::try_minutes,
        'h' => Duration::try_hours,
        'd' => Duration::try_days,
        'w' => Duration::try_weeks,
        _ => return None,
    };
    let span = digits.parse().ok().and_then(to_span);
    Some(match span {
        Some(d) if d <= Duration::days(MAX_SPAN_DAYS) => Span::Within(d),
        _ => Span::TooLong,
    })
}

/// Validate a time value at parse time: a span lowercased, anything else as
/// written.
pub fn normalize(v: &str) -> Result<String, String> {
    if parse_span(v) == Some(Span::TooLong) {
        return Err(format!("`{v}` is too long a span (at most 10000 years)"));
    }
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
    fn a_span_too_long_to_measure_back_is_a_bad_value() {
        for v in ["100000000d", "100000000D", "9223372036854775807m"] {
            assert_eq!(TimeValue::parse(v), None, "{v}");
            assert_eq!(
                normalize(v),
                Err(format!("`{v}` is too long a span (at most 10000 years)")),
            );
        }
        assert_eq!(normalize("521000w"), Ok("521000w".to_string()));
    }

    #[test]
    fn the_longest_span_still_measures_back_from_any_now() {
        let span = TimeValue::parse("3652425d").unwrap();
        let earliest = DateTime::<Utc>::MIN_UTC;
        assert!(span.admits_since(earliest, earliest));
        assert!(!span.admits_until(earliest + Duration::days(1), earliest));
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

/// The time half of the web lockstep: the same fixture the web's
/// `lib/query/time.lockstep.test.ts` runs. Fix the parser, never the
/// fixture, unless the spec changed.
#[cfg(test)]
mod lockstep {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Fixture {
        now: String,
        values: Vec<ValueCase>,
        bounds: Vec<BoundCase>,
    }

    #[derive(serde::Deserialize)]
    struct ValueCase {
        v: String,
        normalized: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct BoundCase {
        v: String,
        ts: String,
        since: bool,
        until: bool,
    }

    #[test]
    fn time_values_match_the_shared_fixture() {
        let f: Fixture =
            serde_json::from_str(include_str!("../tests/fixtures/time_values.json")).unwrap();
        let now = parse_timestamp(&f.now).unwrap();
        for c in &f.values {
            assert_eq!(normalize(&c.v).ok(), c.normalized, "{:?}", c.v);
        }
        for c in &f.bounds {
            let bound = TimeValue::parse(&c.v).unwrap();
            let ts = parse_timestamp(&c.ts);
            let since = ts.is_some_and(|t| bound.admits_since(t, now));
            let until = ts.is_some_and(|t| bound.admits_until(t, now));
            assert_eq!((since, until), (c.since, c.until), "{:?} vs {:?}", c.v, c.ts);
        }
    }
}
