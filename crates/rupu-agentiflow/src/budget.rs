use serde::Deserialize;

/// Stop/throttle conditions on spend, tokens, wall-clock time, and round
/// count. All fields are optional — an unset field never triggers a stop.
/// Enforcement (`BudgetEnforcer`, `UsageSource`, `BudgetStage`) lands in a
/// later task; this is the parsed shape only.
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
    /// Placeholder — real validation (e.g. `soft_at` in `[0, 1]`, `wall_clock`
    /// parses as a duration) lands with the enforcer in a later task.
    pub fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}
