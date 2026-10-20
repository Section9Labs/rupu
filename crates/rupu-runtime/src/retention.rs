//! Retention cutoffs (`30d`, `12h`, `1w`): what `--older-than` and the
//! `[storage]` / `[netflow]` retention keys say, for every prune.

/// A retention value that isn't `<amount><s|m|h|d|w>` (or overflows).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RetentionError(String);

/// Parse a retention cutoff like `30d` / `12h` / `1w` into a
/// `chrono::Duration`.
///
/// Uses `chrono::Duration`'s `try_*` constructors, not the panicking
/// `seconds`/`minutes`/`hours`/`days`/`weeks` ones: an operator-supplied
/// amount is untrusted input, and `chrono::Duration::days(i64::MAX)`
/// panics rather than erroring — an out-of-range amount overflows the
/// internal microsecond representation. `try_*` returns `None` for exactly
/// that case, mapped here to the same "invalid duration" error every other
/// malformed input already produces, so this function never panics on
/// untrusted input.
pub fn parse_retention_duration(value: &str) -> Result<chrono::Duration, RetentionError> {
    let invalid = |detail: Option<String>| {
        RetentionError(match detail {
            Some(d) => format!("invalid duration `{value}`: {d}"),
            None => format!("invalid duration `{value}`"),
        })
    };
    let trimmed = value.trim();
    let unit = trimmed.chars().last().ok_or_else(|| invalid(None))?;
    let amount: i64 = trimmed[..trimmed.len().saturating_sub(1)]
        .parse()
        .map_err(|e: std::num::ParseIntError| invalid(Some(e.to_string())))?;
    let duration = match unit {
        's' => chrono::Duration::try_seconds(amount),
        'm' => chrono::Duration::try_minutes(amount),
        'h' => chrono::Duration::try_hours(amount),
        'd' => chrono::Duration::try_days(amount),
        'w' => chrono::Duration::try_weeks(amount),
        _ => return Err(invalid(None)),
    };
    duration.ok_or_else(|| invalid(Some("amount out of range".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_known_unit() {
        assert_eq!(
            parse_retention_duration("30d").unwrap(),
            chrono::Duration::days(30)
        );
        assert_eq!(
            parse_retention_duration("12h").unwrap(),
            chrono::Duration::hours(12)
        );
        assert_eq!(
            parse_retention_duration("1w").unwrap(),
            chrono::Duration::weeks(1)
        );
        assert_eq!(
            parse_retention_duration("45m").unwrap(),
            chrono::Duration::minutes(45)
        );
        assert_eq!(
            parse_retention_duration("5s").unwrap(),
            chrono::Duration::seconds(5)
        );
    }

    #[test]
    fn rejects_an_unknown_unit() {
        assert!(parse_retention_duration("30x").is_err());
    }

    #[test]
    fn rejects_a_non_numeric_amount() {
        assert!(parse_retention_duration("abcd").is_err());
    }

    /// The panic this whole fix exists to close (netflow-per-run Plan 3
    /// Task 2 review round 2, Minor 1): `chrono::Duration::days(i64::
    /// MAX)` overflows the internal microsecond representation and
    /// panics rather than erroring. An out-of-range amount in ANY unit
    /// must return `Err`, never abort the process.
    #[test]
    fn an_amount_that_would_overflow_returns_an_error_not_a_panic() {
        assert!(parse_retention_duration("99999999999999d").is_err());
        assert!(parse_retention_duration("99999999999999999w").is_err());
        assert!(parse_retention_duration(&format!("{}s", i64::MAX)).is_err());
    }
}
