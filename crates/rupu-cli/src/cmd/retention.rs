use anyhow::{anyhow, bail};

/// Parse a retention cutoff like `30d` / `12h` / `1w` into a
/// `chrono::Duration`.
///
/// Uses `chrono::Duration`'s `try_*` constructors, not the panicking
/// `seconds`/`minutes`/`hours`/`days`/`weeks` ones: an operator-supplied
/// amount is untrusted input, and `chrono::Duration::days(i64::MAX)`
/// (reachable via `rupu netflow prune --older-than 99999999999999d`, or
/// any other caller of this shared parser) panics rather than erroring
/// — an out-of-range amount overflows the internal microsecond
/// representation. `try_*` returns `None` for exactly that case, mapped
/// here to the same "invalid duration" error every other malformed
/// input already produces, so this function's whole contract stays
/// "never panics on untrusted input, always `Result`".
pub fn parse_retention_duration(value: &str) -> anyhow::Result<chrono::Duration> {
    let trimmed = value.trim();
    let unit = trimmed
        .chars()
        .last()
        .ok_or_else(|| anyhow!("invalid duration `{value}`"))?;
    let amount: i64 = trimmed[..trimmed.len().saturating_sub(1)]
        .parse()
        .map_err(|e| anyhow!("invalid duration `{value}`: {e}"))?;
    let duration = match unit {
        's' => chrono::Duration::try_seconds(amount),
        'm' => chrono::Duration::try_minutes(amount),
        'h' => chrono::Duration::try_hours(amount),
        'd' => chrono::Duration::try_days(amount),
        'w' => chrono::Duration::try_weeks(amount),
        _ => bail!("invalid duration `{value}`"),
    };
    duration.ok_or_else(|| anyhow!("invalid duration `{value}`: amount out of range"))
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
