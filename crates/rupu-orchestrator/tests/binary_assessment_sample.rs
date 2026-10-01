//! The `binary-assessment` sample workflow under `.rupu/workflows/` parses,
//! selects the `binary` engagement for the whole run, and runs the sample
//! `binary-analyst` agent, which declares the same engagement itself. The
//! behaviour of that engagement is covered end to end in
//! `rupu-agent/tests/engagement_binary_e2e.rs`.

use rupu_agent::AgentSpec;
use rupu_orchestrator::Workflow;
use std::path::PathBuf;

fn repo_file(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
        .canonicalize()
        .unwrap_or_else(|e| panic!("{rel} should exist: {e}"))
}

#[test]
fn sample_workflow_selects_the_binary_engagement_and_runs_the_sample_agent() {
    let wf = Workflow::parse_file(&repo_file(".rupu/workflows/binary-assessment.yaml"))
        .expect("the binary-assessment sample parses");
    assert_eq!(wf.name, "binary-assessment");
    assert_eq!(wf.defaults.engagement_profiles, vec!["binary"]);
    assert_eq!(wf.steps.len(), 1, "single-agent by design");
    let step = &wf.steps[0];
    assert!(
        step.engagement_profiles.is_empty(),
        "the step inherits the workflow default rather than narrowing it"
    );

    let agent = AgentSpec::parse_file(&repo_file(".rupu/agents/binary-analyst.md"))
        .expect("the binary-analyst sample parses");
    assert_eq!(step.agent.as_deref(), Some(agent.name.as_str()));
    assert_eq!(agent.engagement_profiles, wf.defaults.engagement_profiles);
}
