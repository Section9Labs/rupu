//! Guardrail: the built-in engagement profiles and the stock fleet stay in
//! lockstep.
//!
//!   1. Every `[bundle]` agent/workflow a built-in profile names is shipped
//!      by the stock fleet (`FLEET_MANIFEST`). This permanently closes the
//!      gap this whole feature fixed — a profile that names a fleet member
//!      which does not exist (a typo, or a new bundle entry with no shipped
//!      definition) fails here instead of silently leaving an engagement
//!      with nothing to draw on.
//!   2. Every fleet member is named by at least one profile bundle — no
//!      orphaned agent/workflow ships without a profile that uses it.

use std::collections::HashSet;

use rupu_agent::AgentSpec;
use rupu_cli::templates::FLEET_MANIFEST;
use rupu_coverage::builtin_profiles;
use rupu_orchestrator::Workflow;

fn fleet_agents() -> HashSet<String> {
    FLEET_MANIFEST
        .iter()
        .filter_map(|t| {
            t.target_relpath
                .strip_prefix("agents/")
                .map(|n| n.trim_end_matches(".md").to_string())
        })
        .collect()
}

fn fleet_workflows() -> HashSet<String> {
    FLEET_MANIFEST
        .iter()
        .filter_map(|t| {
            t.target_relpath
                .strip_prefix("workflows/")
                .map(|n| n.trim_end_matches(".yaml").to_string())
        })
        .collect()
}

#[test]
fn every_builtin_bundle_name_is_shipped_in_the_fleet() {
    let agents = fleet_agents();
    let workflows = fleet_workflows();

    for (id, profile) in builtin_profiles() {
        for a in &profile.bundle.agents {
            assert!(
                agents.contains(a),
                "profile `{id}` bundle names agent `{a}`, but the stock fleet ships no `agents/{a}.md`"
            );
        }
        for w in &profile.bundle.workflows {
            assert!(
                workflows.contains(w),
                "profile `{id}` bundle names workflow `{w}`, but the stock fleet ships no `workflows/{w}.yaml`"
            );
        }
    }
}

#[test]
fn every_fleet_member_is_named_by_some_profile_bundle() {
    let mut referenced_agents: HashSet<String> = HashSet::new();
    let mut referenced_workflows: HashSet<String> = HashSet::new();
    for (_id, profile) in builtin_profiles() {
        referenced_agents.extend(profile.bundle.agents.iter().cloned());
        referenced_workflows.extend(profile.bundle.workflows.iter().cloned());
    }

    let orphan_agents: Vec<String> = fleet_agents()
        .difference(&referenced_agents)
        .cloned()
        .collect();
    assert!(
        orphan_agents.is_empty(),
        "fleet ships agents no profile bundle references: {orphan_agents:?}"
    );

    let orphan_workflows: Vec<String> = fleet_workflows()
        .difference(&referenced_workflows)
        .cloned()
        .collect();
    assert!(
        orphan_workflows.is_empty(),
        "fleet ships workflows no profile bundle references: {orphan_workflows:?}"
    );
}

#[test]
fn every_fleet_agent_parses_and_its_name_matches_its_filename() {
    for t in FLEET_MANIFEST {
        let Some(stem) = t
            .target_relpath
            .strip_prefix("agents/")
            .map(|n| n.trim_end_matches(".md"))
        else {
            continue;
        };
        let spec = AgentSpec::parse(t.content)
            .unwrap_or_else(|e| panic!("fleet agent `{}` does not parse: {e}", t.target_relpath));
        // The loader keys agents by their frontmatter `name`, and the
        // profile bundles reference that name — which must equal the file
        // stem, or an installed bundle agent would never resolve.
        assert_eq!(
            spec.name, stem,
            "fleet agent `{}` has frontmatter name `{}`, expected `{stem}`",
            t.target_relpath, spec.name
        );
    }
}

#[test]
fn every_fleet_workflow_parses() {
    for t in FLEET_MANIFEST {
        if t.target_relpath.strip_prefix("workflows/").is_none() {
            continue;
        }
        Workflow::parse(t.content).unwrap_or_else(|e| {
            panic!("fleet workflow `{}` does not parse: {e}", t.target_relpath)
        });
    }
}
