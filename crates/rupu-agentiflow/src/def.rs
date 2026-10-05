use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentiflowDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub lead: String,
    pub engagement_profiles: Vec<String>,
    #[serde(default)]
    pub goals: Vec<Goal>,
    #[serde(default)]
    pub coverage: Option<crate::coverage::CoverageTarget>,
    #[serde(default)]
    pub budget: Option<crate::budget::Budget>,
    pub scope: Scope,
    pub pool: Pool,
    #[serde(default)]
    pub round: Option<RoundConfig>,
    #[serde(default)]
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub id: String,
    pub objective: String,
    pub target: GoalTarget,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub verify_with: Option<String>,
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTarget {
    #[serde(default)]
    pub findings: Option<FindingSelector>,
    #[serde(default)]
    pub asset: Option<AssetSelector>,
    #[serde(default)]
    pub count_gte: Option<u64>,
    #[serde(default)]
    pub depth_at_least: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingSelector {
    /// Matches a classification id, e.g. "CWE-94" (via FindingReport::all_classifications()).
    #[serde(default)]
    pub classification: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetSelector {
    /// Profile-namespaced kind, e.g. "network:host".
    pub kind: String,
    /// coordinate-tag -> value, e.g. {host: "1.1.2.2"}. v1 supports string coords.
    #[serde(default)]
    pub locator: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub authorized: bool,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub roots: Vec<ScopeRoot>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScopeRoot {
    /// Profile-namespaced root kind, e.g. "network:scope" / "web:target".
    pub kind: String,
    /// Remaining keys are the root kind's coordinates/attributes, captured opaquely.
    #[serde(flatten)]
    pub fields: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pool {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub workflows: WorkflowsSpec,
}

/// `workflows: all` or `workflows: [a, b]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum WorkflowsSpec {
    All(AllKeyword),
    List(Vec<String>),
}
impl Default for WorkflowsSpec {
    fn default() -> Self {
        WorkflowsSpec::List(Vec::new())
    }
}
#[derive(Debug, Clone, Deserialize)]
pub enum AllKeyword {
    #[serde(rename = "all")]
    All,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundConfig {
    #[serde(default)]
    pub lead_max_turns: Option<u32>,
    #[serde(default)]
    pub ceiling: Option<Ceiling>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ceiling {
    #[serde(default)]
    pub rounds: Option<u32>,
    #[serde(default)]
    pub wall_clock: Option<String>,
}

impl AgentiflowDef {
    pub fn parse_str(s: &str) -> Result<Self, crate::error::AgentiflowError> {
        Ok(serde_yaml::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name: acme-pentest
description: Assess the in-scope services.
lead: orchestrator-lead
engagement_profiles: [network, web]
goals:
  - id: rce
    objective: "Find 10 verified RCE issues."
    target: { findings: { classification: "CWE-94" }, count_gte: 10, verified: true }
    required: true
coverage:
  reach: 0.9
  depth: tested
budget:
  usd: 50
  wall_clock: "6h"
  rounds: 40
  soft_at: 0.8
scope:
  authorized: true
  mode: bypass
  roots:
    - kind: network:scope
      cidrs: ["10.0.0.0/24"]
      hosts: ["1.1.2.2"]
    - kind: web:target
      url: "https://app.acme.test"
pool:
  agents: [recon, exploit-verifier, orchestrator-lead]
  workflows: [network-assessment]
round:
  lead_max_turns: 200
  ceiling: { rounds: 40, wall_clock: "8h" }
trigger: manual
"#;

    #[test]
    fn parses_a_full_definition() {
        let def = AgentiflowDef::parse_str(SAMPLE).unwrap();
        assert_eq!(def.name, "acme-pentest");
        assert_eq!(def.engagement_profiles, ["network", "web"]);
        assert_eq!(def.goals.len(), 1);
        assert_eq!(def.goals[0].target.count_gte, Some(10));
        assert!(def.goals[0].target.verified);
        assert_eq!(def.scope.roots.len(), 2);
        assert_eq!(def.scope.roots[0].kind, "network:scope");
        assert_eq!(def.pool.agents.len(), 3);
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let bad = "name: x\nlead: y\nengagement_profiles: [code]\nscope: {authorized: true}\npool: {}\nbogus: 1\n";
        assert!(AgentiflowDef::parse_str(bad).is_err());
    }
}
