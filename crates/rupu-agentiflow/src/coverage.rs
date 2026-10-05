use serde::Deserialize;

/// Required engagement-wide coverage threshold. Evaluation
/// (`CoverageEvaluator`, `CoverageOutcome`) against `rupu-coverage` evidence
/// lands in a later task; this is the parsed shape only.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageTarget {
    pub reach: f64,
    #[serde(default)]
    pub depth: Option<String>,
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
}
