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

    /// Resolve `engagement_profiles` against `registry` into the active set
    /// the rest of validation (and the envelope) routes kinds through.
    pub fn resolve_profiles(
        &self,
        registry: &rupu_coverage::ProfileRegistry,
    ) -> Result<rupu_coverage::ActiveSet, crate::error::AgentiflowError> {
        registry
            .active_set(&self.engagement_profiles)
            .map_err(|e| crate::error::AgentiflowError::UnknownProfile(e.to_string()))
    }

    /// Fail-closed validation against the resolved profile set: the scope
    /// must be authorized, there must be something to steer toward, the lead
    /// must be in the pool, every goal predicate must be well-formed and name
    /// a kind/depth an active profile actually defines, every scope root
    /// must be a ROOT kind of an active profile, and the coverage stop (if
    /// any) must be reachable (reach, kinds and depth are all checked).
    pub fn validate(
        &self,
        active: &rupu_coverage::ActiveSet,
    ) -> Result<(), crate::error::AgentiflowError> {
        use crate::error::AgentiflowError::Invalid;
        if !self.scope.authorized {
            return Err(Invalid("scope.authorized must be true".into()));
        }
        if self.goals.is_empty() && self.coverage.is_none() {
            return Err(Invalid("at least one of goals/coverage is required".into()));
        }
        if !self.pool.agents.iter().any(|a| a == &self.lead) {
            return Err(Invalid(format!(
                "lead `{}` is not in pool.agents",
                self.lead
            )));
        }
        // goal predicates
        for g in &self.goals {
            match (&g.target.findings, &g.target.asset) {
                (Some(_), Some(_)) => {
                    return Err(Invalid(format!(
                        "goal `{}`: target has both findings and asset",
                        g.id
                    )))
                }
                (None, None) => {
                    return Err(Invalid(format!(
                        "goal `{}`: target has neither findings nor asset",
                        g.id
                    )))
                }
                (Some(_), None) => {
                    if g.target.count_gte.is_none() {
                        return Err(Invalid(format!(
                            "goal `{}`: findings target requires count_gte",
                            g.id
                        )));
                    }
                }
                (None, Some(a)) => {
                    let Some(depth) = g.target.depth_at_least.as_deref() else {
                        return Err(Invalid(format!(
                            "goal `{}`: asset target requires depth_at_least",
                            g.id
                        )));
                    };
                    if g.target.verified {
                        return Err(Invalid(format!(
                            "goal `{}`: `verified` is not valid on an asset target (the depth rung is the evidence)",
                            g.id
                        )));
                    }
                    // kind must be owned by an active profile; depth a rung of its ladder
                    let profile = active.profile_for_kind(&a.kind).ok_or_else(|| {
                        Invalid(format!(
                            "goal `{}`: asset kind `{}` not owned by any active profile",
                            g.id, a.kind
                        ))
                    })?;
                    // `profile_for_kind` only matches the profile prefix; the kind
                    // itself must also be one the profile defines (ids are
                    // namespaced `"<profile>:<kind>"`).
                    if !profile.asset_kinds.iter().any(|k| k.id == a.kind) {
                        return Err(Invalid(format!(
                            "goal `{}`: asset kind `{}` is not defined by profile `{}`",
                            g.id, a.kind, profile.id
                        )));
                    }
                    if !profile.coverage.depth_ladder.iter().any(|d| d == depth) {
                        return Err(Invalid(format!(
                            "goal `{}`: depth `{}` is not a rung of `{}`'s ladder",
                            g.id, depth, profile.id
                        )));
                    }
                    // Every locator key must be one the evaluator can match, or the
                    // goal could never be met (structured coords / typos).
                    if let Some(key) = a
                        .locator
                        .keys()
                        .find(|k| !crate::goal::V1_STRING_COORD_TAGS.contains(&k.as_str()))
                    {
                        return Err(Invalid(format!(
                            "goal `{}`: locator key `{}` is not a v1-matchable string coordinate",
                            g.id, key
                        )));
                    }
                }
            }
        }
        // scope roots must name a ROOT kind (parent == None) of an active profile
        for root in &self.scope.roots {
            let profile = active.profile_for_kind(&root.kind).ok_or_else(|| {
                Invalid(format!(
                    "scope root kind `{}` not owned by any active profile",
                    root.kind
                ))
            })?;
            // The registry namespaces every kind id (and `parent`) as
            // `"<profile>:<kind>"`, so compare against the full kind string.
            let is_root = profile
                .asset_kinds
                .iter()
                .any(|k| k.id == root.kind && k.parent.is_none());
            if !is_root {
                return Err(Invalid(format!(
                    "scope root `{}` is not a root asset kind of profile `{}`",
                    root.kind, profile.id
                )));
            }
        }
        // coverage stop must be reachable: a stop that can never fire just burns
        // the budget to the ceiling with no explanation
        if let Some(ct) = &self.coverage {
            Self::validate_coverage(ct, active)?;
        }
        // budget sanity
        if let Some(b) = &self.budget {
            b.validate().map_err(Invalid)?;
        }
        Ok(())
    }

    /// The `coverage:` block must describe a stop that can actually fire:
    /// `reach` in `(0.0, 1.0]`, every `kinds` entry a distinct kind an active
    /// profile defines, and `depth` a rung of the ladder of EVERY profile the
    /// evaluator would measure (all active profiles when `kinds` is unset,
    /// else the profiles owning the named kinds).
    fn validate_coverage(
        ct: &crate::coverage::CoverageTarget,
        active: &rupu_coverage::ActiveSet,
    ) -> Result<(), crate::error::AgentiflowError> {
        use crate::error::AgentiflowError::Invalid;
        if !(ct.reach.is_finite() && ct.reach > 0.0 && ct.reach <= 1.0) {
            return Err(Invalid(
                "coverage.reach must be finite and in (0.0, 1.0]".into(),
            ));
        }
        // The profiles whose ladders the evaluator will measure `depth` on.
        let mut evaluated: Vec<&rupu_coverage::EngagementProfile> = Vec::new();
        match &ct.kinds {
            None => evaluated.extend(active.profiles()),
            Some(kinds) => {
                // Namespaced kinds each entry resolves to; two entries that
                // resolve to the same kind would be counted twice.
                let mut seen: std::collections::BTreeSet<String> = Default::default();
                for entry in kinds {
                    let owners: Vec<(&rupu_coverage::EngagementProfile, String)> = if entry
                        .contains(':')
                    {
                        let profile = active.profile_for_kind(entry).ok_or_else(|| {
                            Invalid(format!(
                                "coverage.kinds entry `{entry}` not owned by any active profile"
                            ))
                        })?;
                        if !profile.asset_kinds.iter().any(|k| &k.id == entry) {
                            return Err(Invalid(format!(
                                "coverage.kinds entry `{entry}` is not defined by profile `{}`",
                                profile.id
                            )));
                        }
                        vec![(profile, entry.clone())]
                    } else {
                        // Bare: the evaluator namespaces it under each active
                        // profile; only profiles that define it measure anything.
                        let owners: Vec<_> = active
                            .profiles()
                            .filter_map(|p| {
                                let nk = format!("{}:{entry}", p.id);
                                p.asset_kinds.iter().any(|k| k.id == nk).then_some((p, nk))
                            })
                            .collect();
                        if owners.is_empty() {
                            return Err(Invalid(format!(
                                    "coverage.kinds entry `{entry}` is not defined by any active profile"
                                )));
                        }
                        owners
                    };
                    for (profile, nk) in owners {
                        if !seen.insert(nk.clone()) {
                            return Err(Invalid(format!(
                                "coverage.kinds contains duplicate kind `{entry}` (resolves to `{nk}`)"
                            )));
                        }
                        if !evaluated.iter().any(|p| p.id == profile.id) {
                            evaluated.push(profile);
                        }
                    }
                }
            }
        }
        if let Some(depth) = ct.depth.as_deref() {
            if let Some(profile) = evaluated
                .iter()
                .find(|p| !p.coverage.depth_ladder.iter().any(|d| d == depth))
            {
                return Err(Invalid(format!(
                    "coverage.depth `{depth}` is not a rung of profile `{}`'s ladder",
                    profile.id
                )));
            }
        }
        Ok(())
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
    - kind: network:host
      host: "1.1.2.2"
    - kind: web:site
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
        assert_eq!(def.scope.roots[0].kind, "network:host");
        assert_eq!(def.pool.agents.len(), 3);
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let bad = "name: x\nlead: y\nengagement_profiles: [code]\nscope: {authorized: true}\npool: {}\nbogus: 1\n";
        assert!(AgentiflowDef::parse_str(bad).is_err());
    }

    fn active() -> rupu_coverage::ActiveSet {
        rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".into(), "web".into()])
            .unwrap()
    }

    #[test]
    fn validate_accepts_the_sample() {
        let def = AgentiflowDef::parse_str(SAMPLE).unwrap();
        def.validate(&active()).unwrap();
    }

    #[test]
    fn validate_rejects_unauthorized_scope() {
        let mut def = AgentiflowDef::parse_str(SAMPLE).unwrap();
        def.scope.authorized = false;
        assert!(def.validate(&active()).is_err());
    }

    #[test]
    fn validate_rejects_lead_not_in_pool() {
        let mut def = AgentiflowDef::parse_str(SAMPLE).unwrap();
        def.lead = "ghost".into();
        assert!(def.validate(&active()).is_err());
    }

    #[test]
    fn validate_rejects_goal_with_both_findings_and_asset() {
        let yaml = SAMPLE.replace(
            "target: { findings: { classification: \"CWE-94\" }, count_gte: 10, verified: true }",
            "target: { findings: { classification: \"CWE-94\" }, asset: { kind: \"network:host\" }, count_gte: 1 }",
        );
        let def = AgentiflowDef::parse_str(&yaml).unwrap();
        assert!(def.validate(&active()).is_err());
    }

    #[test]
    fn validate_rejects_scope_root_kind_not_a_profile_root() {
        let mut def = AgentiflowDef::parse_str(SAMPLE).unwrap();
        def.scope.roots[0].kind = "network:service".into(); // service has a parent (host) → not a root
        assert!(def.validate(&active()).is_err());
    }

    /// The five brief tests only assert `is_err()`; pin the REASON so a
    /// rejection can't pass for the wrong cause.
    fn reason(def: &AgentiflowDef) -> String {
        def.validate(&active()).unwrap_err().to_string()
    }

    fn with_target(target: &str) -> AgentiflowDef {
        let yaml = SAMPLE.replace(
            "target: { findings: { classification: \"CWE-94\" }, count_gte: 10, verified: true }",
            &format!("target: {target}"),
        );
        AgentiflowDef::parse_str(&yaml).unwrap()
    }

    #[test]
    fn validate_rejection_reasons_are_specific() {
        let mut d = AgentiflowDef::parse_str(SAMPLE).unwrap();
        d.scope.authorized = false;
        assert!(reason(&d).contains("scope.authorized"));

        let mut d = AgentiflowDef::parse_str(SAMPLE).unwrap();
        d.lead = "ghost".into();
        assert!(reason(&d).contains("not in pool.agents"));

        let mut d = AgentiflowDef::parse_str(SAMPLE).unwrap();
        d.goals.clear();
        d.coverage = None;
        assert!(reason(&d).contains("goals/coverage"));

        let d = with_target(
            "{ findings: { classification: \"CWE-94\" }, asset: { kind: \"network:host\" }, count_gte: 1 }",
        );
        assert!(reason(&d).contains("both findings and asset"));

        let d = with_target("{ count_gte: 1 }");
        assert!(reason(&d).contains("neither findings nor asset"));

        let d = with_target("{ findings: { classification: \"CWE-94\" } }");
        assert!(reason(&d).contains("requires count_gte"));

        let mut d = AgentiflowDef::parse_str(SAMPLE).unwrap();
        d.scope.roots[0].kind = "network:service".into();
        assert!(reason(&d).contains("not a root asset kind"));

        let mut d = AgentiflowDef::parse_str(SAMPLE).unwrap();
        d.scope.roots[0].kind = "code:repo".into(); // `code` is not active
        assert!(reason(&d).contains("not owned by any active profile"));
    }

    #[test]
    fn validate_asset_goal_checks_kind_depth_and_verified() {
        let ok = with_target(
            "{ asset: { kind: \"network:host\", locator: { host: \"1.1.2.2\" } }, depth_at_least: exploited }",
        );
        ok.validate(&active()).unwrap();

        let d = with_target("{ asset: { kind: \"network:host\" } }");
        assert!(reason(&d).contains("requires depth_at_least"));

        let d = with_target(
            "{ asset: { kind: \"network:host\" }, depth_at_least: exploited, verified: true }",
        );
        assert!(reason(&d).contains("`verified` is not valid on an asset target"));

        let d = with_target("{ asset: { kind: \"code:file\" }, depth_at_least: exploited }");
        assert!(reason(&d).contains("not owned by any active profile"));

        // `mapped` is a web rung, not a network rung.
        let d = with_target("{ asset: { kind: \"network:host\" }, depth_at_least: mapped }");
        assert!(reason(&d).contains("not a rung of `network`'s ladder"));

        // ...but it is valid against a web kind.
        let d = with_target("{ asset: { kind: \"web:site\" }, depth_at_least: mapped }");
        d.validate(&active()).unwrap();
    }

    #[test]
    fn validate_asset_goal_kind_must_be_defined_by_its_profile() {
        // `network:` resolves to the `network` profile and `exploited` is on
        // its ladder, but `bogus` is not one of its asset kinds.
        let d = with_target("{ asset: { kind: \"network:bogus\" }, depth_at_least: exploited }");
        assert!(reason(&d).contains("is not defined by profile `network`"));

        // A real kind of the same profile still passes (host AND the child
        // kind service).
        let d = with_target("{ asset: { kind: \"network:host\" }, depth_at_least: exploited }");
        d.validate(&active()).unwrap();
        let d = with_target("{ asset: { kind: \"network:service\" }, depth_at_least: tested }");
        d.validate(&active()).unwrap();

        // `count_gte` on an asset target is valid (the required asset count).
        let d = with_target(
            "{ asset: { kind: \"network:host\" }, depth_at_least: tested, count_gte: 5 }",
        );
        d.validate(&active()).unwrap();
    }

    #[test]
    fn validate_asset_goal_locator_keys_must_be_v1_matchable() {
        // A structured coordinate tag can never match in v1 (the goal would be
        // unmeetable and burn budget), so it is rejected up front with the
        // offending key named.
        let d = with_target(
            "{ asset: { kind: \"network:host\", locator: { port: \"443\" } }, depth_at_least: exploited }",
        );
        let r = reason(&d);
        assert!(
            r.contains("locator key `port` is not a v1-matchable string coordinate"),
            "{r}"
        );
        assert!(r.contains("goal `"), "{r}");

        // A typo for a real tag is the same class of mistake.
        let d = with_target(
            "{ asset: { kind: \"network:host\", locator: { hostname: \"a\" } }, depth_at_least: exploited }",
        );
        let r = reason(&d);
        assert!(
            r.contains("locator key `hostname` is not a v1-matchable string coordinate"),
            "{r}"
        );

        // One bad key among good ones is still rejected.
        let d = with_target(
            "{ asset: { kind: \"network:host\", locator: { host: \"1.1.2.2\", ip: \"x\" } }, depth_at_least: exploited }",
        );
        assert!(reason(&d).contains("locator key `ip`"));

        // A v1 string tag still validates.
        let d = with_target(
            "{ asset: { kind: \"network:host\", locator: { host: \"1.1.2.2\" } }, depth_at_least: exploited }",
        );
        d.validate(&active()).unwrap();
    }

    /// `SAMPLE` with its `coverage:` block replaced by `inline` (a YAML flow
    /// mapping, e.g. `{ reach: 0.5 }`).
    fn with_coverage(inline: &str) -> AgentiflowDef {
        let old = "coverage:\n  reach: 0.9\n  depth: tested\n";
        assert!(SAMPLE.contains(old), "SAMPLE coverage block changed");
        let yaml = SAMPLE.replace(old, &format!("coverage: {inline}\n"));
        AgentiflowDef::parse_str(&yaml).unwrap()
    }

    /// Validate `def` against an arbitrary active set. The scope roots are cut
    /// to the first (a `network:` root) so a profile set without `web` doesn't
    /// trip the scope-root check before the coverage check.
    fn validate_against(
        mut def: AgentiflowDef,
        ids: &[&str],
    ) -> Result<(), crate::error::AgentiflowError> {
        def.scope.roots.truncate(1);
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let active = rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&ids)
            .unwrap();
        def.validate(&active)
    }

    #[test]
    fn validate_coverage_accepts_well_formed_blocks() {
        // The sample's own block (network+web both have a `tested` rung).
        AgentiflowDef::parse_str(SAMPLE)
            .unwrap()
            .validate(&active())
            .unwrap();
        // reach is inclusive at 1.0; depth/kinds are optional.
        with_coverage("{ reach: 1.0 }").validate(&active()).unwrap();
        // Bare and namespaced kinds, each owned by an active profile.
        with_coverage("{ reach: 0.5, kinds: [host, \"web:site\"], depth: tested }")
            .validate(&active())
            .unwrap();
        // Only the OWNING profile's ladder is consulted when kinds are given:
        // `enumerated` is a network rung and web is not evaluated.
        with_coverage("{ reach: 0.5, kinds: [\"network:host\"], depth: enumerated }")
            .validate(&active())
            .unwrap();
    }

    #[test]
    fn validate_coverage_reach_must_be_finite_and_in_unit_interval() {
        for bad in ["1.5", "0.0", "-0.1", ".nan", ".inf"] {
            let d = with_coverage(&format!("{{ reach: {bad} }}"));
            let r = reason(&d);
            assert!(
                r.contains("coverage.reach must be finite and in (0.0, 1.0]"),
                "reach {bad}: {r}"
            );
        }
    }

    #[test]
    fn validate_coverage_kinds_must_be_defined_and_unique() {
        // Namespaced, owner active, but not a kind the profile defines.
        let r = reason(&with_coverage("{ reach: 0.5, kinds: [\"network:bogus\"] }"));
        assert!(
            r.contains("coverage.kinds entry `network:bogus` is not defined by profile `network`"),
            "{r}"
        );

        // Namespaced, owner not active (`code` is not in network+web).
        let r = reason(&with_coverage("{ reach: 0.5, kinds: [\"code:file\"] }"));
        assert!(
            r.contains("coverage.kinds entry `code:file` not owned by any active profile"),
            "{r}"
        );

        // Bare, defined by no active profile.
        let r = reason(&with_coverage("{ reach: 0.5, kinds: [bogus] }"));
        assert!(
            r.contains("coverage.kinds entry `bogus` is not defined by any active profile"),
            "{r}"
        );
        // ...including a bare kind that only an INACTIVE profile defines.
        let r = reason(&with_coverage("{ reach: 0.5, kinds: [file] }"));
        assert!(
            r.contains("`file` is not defined by any active profile"),
            "{r}"
        );

        // Exact duplicate.
        let r = reason(&with_coverage("{ reach: 0.5, kinds: [host, host] }"));
        assert!(r.contains("duplicate kind `host`"), "{r}");
        // Bare + namespaced spelling of the same kind would be counted twice.
        let r = reason(&with_coverage(
            "{ reach: 0.5, kinds: [host, \"network:host\"] }",
        ));
        assert!(
            r.contains("duplicate kind `network:host` (resolves to `network:host`)"),
            "{r}"
        );
    }

    #[test]
    fn validate_coverage_depth_must_be_on_every_evaluated_profiles_ladder() {
        // No kinds: EVERY active profile is evaluated. `mapped` is a web rung
        // only, so network (first by id) is the offender.
        let r = reason(&with_coverage("{ reach: 0.5, depth: mapped }"));
        assert!(
            r.contains("coverage.depth `mapped` is not a rung of profile `network`'s ladder"),
            "{r}"
        );

        // network+code, no kinds: `tested` is a network rung but not a code
        // rung, so the coverage stop could never be reached for code's assets.
        let d = with_coverage("{ reach: 0.5, depth: tested }");
        let r = validate_against(d, &["network", "code"])
            .unwrap_err()
            .to_string();
        assert!(
            r.contains("coverage.depth `tested` is not a rung of profile `code`'s ladder"),
            "{r}"
        );

        // With kinds given only the OWNING profiles are checked: scoping to
        // network's kind makes `tested` fine even though code lacks it...
        let d = with_coverage("{ reach: 0.5, kinds: [\"network:host\"], depth: tested }");
        validate_against(d, &["network", "code"]).unwrap();
        // ...but naming code's kind brings code's ladder back in.
        let d = with_coverage("{ reach: 0.5, kinds: [\"code:file\"], depth: tested }");
        let r = validate_against(d, &["network", "code"])
            .unwrap_err()
            .to_string();
        assert!(
            r.contains("coverage.depth `tested` is not a rung of profile `code`'s ladder"),
            "{r}"
        );

        // A rung every evaluated profile shares passes.
        let d = with_coverage("{ reach: 0.5, depth: tested }");
        validate_against(d, &["network", "web"]).unwrap();
    }

    #[test]
    fn resolve_profiles_maps_registry_errors_to_unknown_profile() {
        let registry = rupu_coverage::builtin_registry().unwrap();
        let def = AgentiflowDef::parse_str(SAMPLE).unwrap();
        assert!(def.resolve_profiles(&registry).is_ok());

        let mut bad = AgentiflowDef::parse_str(SAMPLE).unwrap();
        bad.engagement_profiles = vec!["no-such-profile".into()];
        assert!(matches!(
            bad.resolve_profiles(&registry),
            Err(crate::error::AgentiflowError::UnknownProfile(_))
        ));
    }
}
