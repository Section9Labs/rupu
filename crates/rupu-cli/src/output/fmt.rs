//! Shared compact formatters for token counts and costs.
//!
//! Promoted out of `cmd/session.rs` so the status header and the live
//! workflow run view share one implementation. Keep these pure and
//! allocation-light: they're called on every render tick.

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
}
