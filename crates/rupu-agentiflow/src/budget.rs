use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

/// Stop/throttle conditions on spend, tokens, wall-clock time, and round
/// count. All fields are optional — an unset field never triggers a stop.
/// `BudgetEnforcer` evaluates this against a `UsageSource`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    #[serde(default)]
    pub usd: Option<f64>,
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub wall_clock: Option<String>,
    #[serde(default)]
    pub rounds: Option<u32>,
    #[serde(default)]
    pub soft_at: Option<f64>,
}

impl Budget {
    /// Fail-closed shape check: `usd >= 0`, `soft_at` in `[0, 1]`, and
    /// `wall_clock` parses as a duration (`Ns`/`Nm`/`Nh`/`Nd`).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(u) = self.usd {
            if u.is_nan() || u < 0.0 {
                return Err("budget.usd must be >= 0".into());
            }
        }
        if let Some(s) = self.soft_at {
            if !(0.0..=1.0).contains(&s) {
                return Err("budget.soft_at must be in [0,1]".into());
            }
        }
        if let Some(w) = &self.wall_clock {
            parse_duration(w)?;
        }
        Ok(())
    }
}

/// Port: how much has been spent so far. The envelope's real source is the
/// run's usage ledger; tests use a fixed value.
pub trait UsageSource: Send + Sync {
    fn spent_usd(&self) -> f64;
    fn spent_tokens(&self) -> u64;
}

/// Where the run stands against its budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetStage {
    /// Every set dimension is below the soft threshold.
    Ok,
    /// At least one set dimension has crossed `soft_at` (but none hit its cap).
    Soft,
    /// A set dimension reached its cap; `dimension` names the first to trip.
    Hard { dimension: String },
}

/// Evaluates a `Budget` over time. First-to-trip across the four dimensions
/// (usd, tokens, rounds, wall_clock); unset dimensions are ignored.
pub struct BudgetEnforcer {
    budget: Budget,
    started: DateTime<Utc>,
    soft_at: f64,
}

impl BudgetEnforcer {
    /// `soft_at` defaults to 0.8 when the budget leaves it unset.
    pub fn new(budget: Budget, started: DateTime<Utc>) -> Self {
        let soft_at = budget.soft_at.unwrap_or(0.8);
        Self {
            budget,
            started,
            soft_at,
        }
    }

    pub fn stage(&self, usage: &dyn UsageSource, round: u32, now: DateTime<Utc>) -> BudgetStage {
        // (fraction, dimension-name) for each SET dimension.
        let mut fracs: Vec<(f64, &str)> = Vec::new();
        if let Some(cap) = self.budget.usd {
            if cap > 0.0 {
                fracs.push((usage.spent_usd() / cap, "usd"));
            }
        }
        if let Some(cap) = self.budget.tokens {
            if cap > 0 {
                fracs.push((usage.spent_tokens() as f64 / cap as f64, "tokens"));
            }
        }
        if let Some(cap) = self.budget.rounds {
            if cap > 0 {
                fracs.push((round as f64 / cap as f64, "rounds"));
            }
        }
        if let Some(w) = &self.budget.wall_clock {
            if let Ok(d) = parse_duration(w) {
                let secs = d.num_seconds().max(1) as f64;
                let elapsed = (now - self.started).num_seconds().max(0) as f64;
                fracs.push((elapsed / secs, "wall_clock"));
            }
        }
        if let Some((_, dim)) = fracs.iter().find(|(f, _)| *f >= 1.0) {
            return BudgetStage::Hard {
                dimension: (*dim).to_string(),
            };
        }
        if fracs.iter().any(|(f, _)| *f >= self.soft_at) {
            return BudgetStage::Soft;
        }
        BudgetStage::Ok
    }
}

/// Parse `Ns` / `Nm` / `Nh` / `Nd` into a `chrono::Duration`. Out-of-range
/// counts are an error rather than a panic.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("no unit in `{s}`"))?;
    let (num, unit) = s.split_at(split);
    let n: i64 = num.parse().map_err(|_| format!("bad number in `{s}`"))?;
    let d = match unit {
        "s" => Duration::try_seconds(n),
        "m" => Duration::try_minutes(n),
        "h" => Duration::try_hours(n),
        "d" => Duration::try_days(n),
        other => return Err(format!("unknown duration unit `{other}`")),
    };
    d.ok_or_else(|| format!("duration `{s}` is out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    struct FixedUsage {
        usd: f64,
        tokens: u64,
    }
    impl UsageSource for FixedUsage {
        fn spent_usd(&self) -> f64 {
            self.usd
        }
        fn spent_tokens(&self) -> u64 {
            self.tokens
        }
    }

    fn budget() -> Budget {
        Budget {
            usd: Some(50.0),
            tokens: Some(20_000_000),
            wall_clock: Some("6h".into()),
            rounds: Some(40),
            soft_at: Some(0.8),
        }
    }

    #[test]
    fn hard_when_any_dimension_hits_cap() {
        let start = Utc::now();
        let e = BudgetEnforcer::new(budget(), start);
        let now = start;
        assert!(matches!(
            e.stage(
                &FixedUsage {
                    usd: 50.0,
                    tokens: 0
                },
                0,
                now
            ),
            BudgetStage::Hard { .. }
        ));
        assert!(matches!(
            e.stage(
                &FixedUsage {
                    usd: 0.0,
                    tokens: 0
                },
                40,
                now
            ),
            BudgetStage::Hard { .. }
        ));
        let late = start + Duration::hours(6);
        assert!(matches!(
            e.stage(
                &FixedUsage {
                    usd: 0.0,
                    tokens: 0
                },
                0,
                late
            ),
            BudgetStage::Hard { .. }
        ));
    }

    #[test]
    fn soft_at_threshold_then_ok_below() {
        let start = Utc::now();
        let e = BudgetEnforcer::new(budget(), start);
        assert!(matches!(
            e.stage(
                &FixedUsage {
                    usd: 40.0,
                    tokens: 0
                },
                0,
                start
            ),
            BudgetStage::Soft
        )); // 80% USD
        assert!(matches!(
            e.stage(
                &FixedUsage {
                    usd: 10.0,
                    tokens: 0
                },
                0,
                start
            ),
            BudgetStage::Ok
        ));
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("6h").unwrap(), Duration::hours(6));
        assert_eq!(parse_duration("30m").unwrap(), Duration::minutes(30));
        assert!(parse_duration("banana").is_err());
    }
}
