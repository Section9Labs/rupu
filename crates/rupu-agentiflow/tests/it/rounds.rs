//! Each child an agentiflow lead launches records the lead round that
//! launched it on its `started` line in the flow's children ledger.

use std::sync::{Arc, Mutex};

use rupu_agent::{MockProvider, ScriptedTurn, StopReason as ModelStop};
use rupu_agentiflow::{
    agentiflow_dir, decision_excerpt, run_agentiflow, AgentiflowDef, LeadInputs, LeadProvider,
    RunAgentiflowOpts,
};
use rupu_providers::LlmProvider;
use rupu_runtime::assembly::{AssemblyContext, RunAssembler};

fn dispatch(id: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "dispatch".into(),
        tool_input: serde_json::json!({"kind": "agent", "name": "recon", "prompt": "scan"}),
        stop: ModelStop::ToolUse,
    }
}

fn done() -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: "Done.".into(),
        stop: ModelStop::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

#[test]
fn lead_launches_are_stamped_with_the_current_round() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(global.join("agents")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        global.join("agents/recon.md"),
        "---\nname: recon\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nYou survey.\n",
    )
    .unwrap();

    // Round 0 dispatches one unit, round 1 another.
    let mut rounds = vec![
        MockProvider::new(vec![dispatch("c1"), done()]),
        MockProvider::new(vec![dispatch("c2"), done()]),
    ];
    rounds.reverse();
    let rounds = Arc::new(Mutex::new(rounds));
    let started = chrono::Utc::now();
    run_agentiflow(RunAgentiflowOpts {
        def: AgentiflowDef::parse_str(
            "name: rounds\n\
             lead: lead\n\
             engagement_profiles: [network]\n\
             goals:\n  \
               - id: any-finding\n    \
                 objective: \"Record at least one finding.\"\n    \
                 target: { findings: {}, count_gte: 1 }\n\
             scope: { authorized: true }\n\
             pool: { agents: [lead, recon] }\n\
             round: { lead_max_turns: 4, ceiling: { rounds: 2 } }\n",
        )
        .unwrap(),
        workspace: workspace.clone(),
        global: global.clone(),
        active: rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".to_string()])
            .unwrap(),
        started,
        now: Box::new(move || started),
        assembler: RunAssembler::new(AssemblyContext::minimal(global.clone())),
        workspace_id: "ws_test".into(),
        lead: LeadInputs {
            agent: rupu_agent::AgentSpec::parse(
                "---\nname: lead\ntools: [dispatch]\n---\nYou are the lead.\n",
            )
            .unwrap(),
            provider: LeadProvider::Injected {
                provider: "mock".into(),
                model: "mock-1".into(),
                make: Box::new(move || {
                    Box::new(rounds.lock().unwrap().pop().expect("a lead for this round"))
                        as Box<dyn LlmProvider>
                }),
            },
        },
        run_id: "af_rounds".into(),
        process_backend: Some(Arc::new(rupu_launch::testing::ScriptedProcesses::done(
            "ok",
        ))),
        generation: None,
        pricing: Default::default(),
    })
    .unwrap();

    let dir = agentiflow_dir(&global).join("af_rounds");
    let mut rounds: Vec<Option<u32>> = rupu_launch::ledger::read(&dir)
        .iter()
        .map(|c| c.started.round)
        .collect();
    rounds.sort();
    assert_eq!(rounds, vec![Some(0), Some(1)]);
}

#[test]
fn decision_excerpt_caps_at_600_on_a_word_boundary() {
    assert_eq!(decision_excerpt("  short one \n"), "short one");
    let long = "alpha ".repeat(150);
    let cut = decision_excerpt(&long);
    assert!(cut.chars().count() <= 601, "{}", cut.chars().count());
    assert!(cut.ends_with('\u{2026}'));
    let body = cut.trim_end_matches('\u{2026}');
    assert!(
        body.split(' ').all(|w| w == "alpha"),
        "split a word: {body:?}"
    );
    // One unbroken run longer than the cap still cuts, on a char boundary.
    let wall = "é".repeat(900);
    let cut = decision_excerpt(&wall);
    assert_eq!(cut.chars().count(), 601);
}

/// Run a one-goal flow on `leads` (one mock provider per round, in order) and
/// return its `round` events. `prep` gets the run's dir before the run starts.
fn round_events(
    id: &str,
    leads: Vec<MockProvider>,
    prep: impl FnOnce(&std::path::Path),
) -> Vec<serde_json::Value> {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(global.join("agents")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let dir = agentiflow_dir(&global).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    prep(&dir);
    let n = leads.len();
    let mut leads = leads;
    leads.reverse();
    let leads = Arc::new(Mutex::new(leads));
    let started = chrono::Utc::now();
    let tick = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let clock = Arc::clone(&tick);
    run_agentiflow(RunAgentiflowOpts {
        def: AgentiflowDef::parse_str(&format!(
            "name: rounds\n\
             lead: lead\n\
             engagement_profiles: [network]\n\
             goals:\n  \
               - id: any-finding\n    \
                 objective: \"Record at least one finding.\"\n    \
                 target: {{ findings: {{}}, count_gte: 1 }}\n\
             scope: {{ authorized: true }}\n\
             pool: {{ agents: [lead] }}\n\
             round: {{ lead_max_turns: 4, ceiling: {{ rounds: {n} }} }}\n"
        ))
        .unwrap(),
        workspace: workspace.clone(),
        global: global.clone(),
        active: rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".to_string()])
            .unwrap(),
        started,
        now: Box::new(move || {
            started
                + chrono::Duration::seconds(clock.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
        }),
        assembler: RunAssembler::new(AssemblyContext::minimal(global.clone())),
        workspace_id: "ws_test".into(),
        lead: LeadInputs {
            agent: rupu_agent::AgentSpec::parse(
                "---\nname: lead\ntools: [dispatch]\n---\nYou are the lead.\n",
            )
            .unwrap(),
            provider: LeadProvider::Injected {
                provider: "mock".into(),
                model: "mock-1".into(),
                make: Box::new(move || {
                    Box::new(leads.lock().unwrap().pop().expect("a lead for this round"))
                        as Box<dyn LlmProvider>
                }),
            },
        },
        run_id: id.into(),
        process_backend: Some(Arc::new(rupu_launch::testing::ScriptedProcesses::done(
            "ok",
        ))),
        generation: None,
        pricing: Default::default(),
    })
    .unwrap();
    std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|e| e["kind"] == "round")
        .collect()
}

fn says(text: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: text.into(),
        stop: ModelStop::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

#[test]
fn round_event_records_decision_start_goals() {
    let rounds = round_events(
        "af_decision",
        vec![MockProvider::new(vec![says("Map the surface first.")])],
        |_| {},
    );
    let round = &rounds[0];
    assert_eq!(round["decision"], "Map the surface first.");
    let ts = chrono::DateTime::parse_from_rfc3339(round["ts"].as_str().unwrap()).unwrap();
    let began =
        chrono::DateTime::parse_from_rfc3339(round["started_at"].as_str().unwrap()).unwrap();
    assert!(began < ts, "started_at {began} should precede ts {ts}");
    assert_eq!(
        round["goals"],
        serde_json::json!([{"id": "any-finding", "met": false, "current": 0}])
    );
    assert!(round.get("warnings").is_none(), "{round}");
}

#[test]
fn a_round_that_says_nothing_carries_no_decision() {
    // Round 1's model call fails before any reply: the history still ends with
    // round 0's answer, which must not be reported as round 1's decision.
    let rounds = round_events(
        "af_nodecision",
        vec![
            MockProvider::new(vec![says("Map the surface first.")]),
            MockProvider::new(vec![]),
        ],
        |_| {},
    );
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[0]["decision"], "Map the surface first.");
    assert_eq!(rounds[1]["outcome"], "error", "{}", rounds[1]);
    assert!(rounds[1].get("decision").is_none(), "{}", rounds[1]);
}

#[test]
fn round_event_carries_the_digests_warnings() {
    // A file where the control queue's directory belongs makes the drain fail,
    // which the digest reports as a warning.
    let rounds = round_events(
        "af_warn",
        vec![MockProvider::new(vec![says("Go.")])],
        |dir| std::fs::write(dir.join("steering"), b"not a directory").unwrap(),
    );
    let warnings = rounds[0]["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("operator queue unreadable")),
        "{warnings:?}"
    );
}
