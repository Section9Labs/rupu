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
#[derive(Debug, Clone)]
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

    /// First-to-trip stage. Fail closed: a zero cap is exhausted by
    /// definition, so it is `Hard` on the first call (never "unlimited") —
    /// the budget is the envelope's last-resort kill switch.
    pub fn stage(&self, usage: &dyn UsageSource, round: u32, now: DateTime<Utc>) -> BudgetStage {
        // (fraction, dimension-name) for each SET dimension.
        let mut fracs: Vec<(f64, &str)> = Vec::new();
        if let Some(cap) = self.budget.usd {
            let frac = if cap.is_nan() || cap <= 0.0 {
                f64::INFINITY
            } else {
                usage.spent_usd() / cap
            };
            fracs.push((frac, "usd"));
        }
        if let Some(cap) = self.budget.tokens {
            let frac = if cap == 0 {
                f64::INFINITY
            } else {
                usage.spent_tokens() as f64 / cap as f64
            };
            fracs.push((frac, "tokens"));
        }
        if let Some(cap) = self.budget.rounds {
            let frac = if cap == 0 {
                f64::INFINITY
            } else {
                round as f64 / cap as f64
            };
            fracs.push((frac, "rounds"));
        }
        if let Some(w) = &self.budget.wall_clock {
            if let Ok(d) = parse_duration(w) {
                let secs = d.num_seconds();
                let frac = if secs <= 0 {
                    f64::INFINITY
                } else {
                    let elapsed = (now - self.started).num_seconds().max(0);
                    elapsed as f64 / secs as f64
                };
                fracs.push((frac, "wall_clock"));
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

    fn only(f: impl FnOnce(&mut Budget)) -> Budget {
        let mut b = Budget {
            usd: None,
            tokens: None,
            wall_clock: None,
            rounds: None,
            soft_at: None,
        };
        f(&mut b);
        b
    }

    fn hard(dimension: &str) -> BudgetStage {
        BudgetStage::Hard {
            dimension: dimension.into(),
        }
    }

    #[test]
    fn zero_cap_is_immediately_hard_for_every_dimension() {
        // A 0 cap is exhausted by definition: fail closed, never "unlimited".
        let start = Utc::now();
        let idle = FixedUsage {
            usd: 0.0,
            tokens: 0,
        };
        let cases: [(&str, Budget); 4] = [
            ("usd", only(|b| b.usd = Some(0.0))),
            ("tokens", only(|b| b.tokens = Some(0))),
            ("rounds", only(|b| b.rounds = Some(0))),
            ("wall_clock", only(|b| b.wall_clock = Some("0s".into()))),
        ];
        for (dim, b) in cases {
            b.validate().expect("a zero cap is a valid budget");
            let e = BudgetEnforcer::new(b, start);
            assert_eq!(e.stage(&idle, 0, start), hard(dim), "dimension {dim}");
        }
    }

    #[test]
    fn validate_rejects_bad_budgets_and_accepts_a_normal_one() {
        assert!(budget().validate().is_ok());
        assert!(only(|b| b.usd = Some(-1.0)).validate().is_err());
        assert!(only(|b| b.usd = Some(f64::NAN)).validate().is_err());
        assert!(only(|b| b.soft_at = Some(1.5)).validate().is_err());
        assert!(only(|b| b.soft_at = Some(-0.1)).validate().is_err());
        assert!(only(|b| b.wall_clock = Some("banana".into()))
            .validate()
            .is_err());
        // Boundaries and zero are accepted.
        assert!(only(|b| b.soft_at = Some(1.0)).validate().is_ok());
        assert!(only(|b| b.usd = Some(0.0)).validate().is_ok());
    }

    #[test]
    fn hard_preempts_soft_and_names_the_first_tripped_dimension() {
        let start = Utc::now();
        let e = BudgetEnforcer::new(budget(), start);
        // usd at cap (hard) AND tokens at 90% (soft): Hard wins, usd named.
        let usage = FixedUsage {
            usd: 50.0,
            tokens: 18_000_000,
        };
        assert_eq!(e.stage(&usage, 0, start), hard("usd"));
        // Two dimensions hard at once: the first in usd, tokens, rounds,
        // wall_clock order is named.
        let both = FixedUsage {
            usd: 0.0,
            tokens: 20_000_000,
        };
        assert_eq!(e.stage(&both, 40, start), hard("tokens"));
    }

    #[test]
    fn soft_at_defaults_to_eighty_percent_when_unset() {
        let start = Utc::now();
        let e = BudgetEnforcer::new(only(|b| b.usd = Some(100.0)), start);
        let at = |usd: f64| FixedUsage { usd, tokens: 0 };
        assert_eq!(e.stage(&at(79.0), 0, start), BudgetStage::Ok);
        assert_eq!(e.stage(&at(80.0), 0, start), BudgetStage::Soft);
        assert_eq!(e.stage(&at(100.0), 0, start), hard("usd"));
    }

    #[test]
    fn soft_can_be_triggered_by_wall_clock_alone() {
        let start = Utc::now();
        let e = BudgetEnforcer::new(budget(), start); // 6h, soft_at 0.8 => 4.8h
        let idle = FixedUsage {
            usd: 0.0,
            tokens: 0,
        };
        assert_eq!(
            e.stage(&idle, 0, start + Duration::hours(4)),
            BudgetStage::Ok
        );
        assert_eq!(
            e.stage(&idle, 0, start + Duration::hours(5)),
            BudgetStage::Soft
        );
        assert_eq!(
            e.stage(&idle, 0, start + Duration::hours(6)),
            hard("wall_clock")
        );
    }
}
