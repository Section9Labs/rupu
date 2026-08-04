//! The shipped benchmark workflows must parse.
//!
//! `rupu workflow list` renders an unparseable workflow as a normal row and
//! still exits 0 — the only signal is a `—` in the STEPS column. That is far
//! too easy to miss, so parsing is asserted here instead.

use std::path::Path;

fn workflows_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.rupu/workflows")
}

#[test]
fn shipped_bench_workflows_parse() {
    let mut checked = 0;
    for name in ["cybergym", "cybermark"] {
        let path = workflows_dir().join(format!("{name}.yaml"));
        if !path.exists() {
            // The sibling plan may not have landed yet.
            continue;
        }
        rupu_orchestrator::workflow::Workflow::parse_file(&path)
            .unwrap_or_else(|e| panic!("{name}.yaml failed to parse: {e}"));
        checked += 1;
    }
    assert!(checked > 0, "expected at least one bench workflow to exist");
}

#[test]
fn cybergym_has_the_expected_step_shape() {
    let path = workflows_dir().join("cybergym.yaml");
    if !path.exists() {
        return;
    }
    let wf = rupu_orchestrator::workflow::Workflow::parse_file(&path).expect("parses");
    let ids: Vec<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "preflight",
            "plan",
            "gen",
            "solve",
            "verify",
            "collect",
            "render",
            "analyze"
        ]
    );

    // Scoring must come from a deterministic step, never an agent.
    let verify = wf.steps.iter().find(|s| s.id == "verify").expect("verify");
    assert!(verify.run.is_some(), "verify must be a run: step");
    assert!(verify.agent.is_none(), "verify must never be agent-driven");

    // Only the two intended steps dispatch a model.
    let agent_steps: Vec<&str> = wf
        .steps
        .iter()
        .filter(|s| s.agent.is_some())
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(agent_steps, vec!["solve", "analyze"]);
}
