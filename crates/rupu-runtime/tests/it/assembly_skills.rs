//! A run's skills (spec 2026-10-12 §2.4, §3): the assembler resolves the
//! agent's and the launch's skills, renders them into the system prompt and
//! offers `skills.load`; a run never starts without a skill it was given.

use rupu_runtime::assembly::{AssembleError, Origin};
use rupu_transcript::Event;
use serial_test::serial;

use crate::assembly::{agent, launch};
use crate::support::{config, events, Home, MockScript, DONE};

fn run_start_prompt(evs: &[Event]) -> String {
    evs.iter()
        .find_map(|e| match e {
            Event::RunStart { system_prompt, .. } => system_prompt.clone(),
            _ => None,
        })
        .expect("run_start with a system prompt")
}

fn offered(evs: &[Event]) -> Vec<rupu_transcript::ToolGrantEntry> {
    evs.iter()
        .find_map(|e| match e {
            Event::ToolGrant { entries, .. } => Some(entries.clone()),
            _ => None,
        })
        .expect("a tool_grant")
}

/// An agent naming `missing`, built outside the loader (which would refuse
/// it at load), as a skill deleted between load and launch reaches the
/// assembler.
fn agent_naming_a_missing_skill(provider: &str) -> rupu_agent::AgentSpec {
    rupu_agent::AgentSpec::parse(&format!(
        "---\nname: probe\nprovider: {provider}\nmodel: claude-sonnet-4-6\nskills: [missing]\n---\nx\n"
    ))
    .unwrap()
}

#[tokio::test]
#[serial]
async fn agent_skills_reach_the_prompt_and_the_grant() {
    let home = Home::new();
    home.skill("report-style", "House style for reports.", "Write plainly.");
    home.skill("code-review", "Review a diff.", "Look for bugs.");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review]\npreloadSkills: [report-style]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_sk");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let evs = events(&path);
    let prompt = run_start_prompt(&evs);
    assert!(prompt.starts_with("you are probe."), "{prompt}");
    assert!(prompt.contains("## Skill: report-style"), "{prompt}");
    assert!(prompt.contains("Write plainly."), "{prompt}");
    assert!(prompt.contains("- code-review: Review a diff."), "{prompt}");
    assert!(
        !prompt.contains("- report-style:"),
        "a preloaded skill is not indexed: {prompt}"
    );
    let offered = offered(&evs);
    let load = offered
        .iter()
        .find(|e| e.tool == "skills.load")
        .expect("skills.load offered");
    assert_eq!(load.reasons, vec!["ambient:skills".to_string()]);
}

#[tokio::test]
#[serial]
async fn an_agent_without_skills_gets_no_loader() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [\"*\"]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_none");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let evs = events(&path);
    assert!(!run_start_prompt(&evs).contains("## Skill"));
    assert!(!evs.iter().any(|e| matches!(e, Event::SkillIndex { .. })));
    assert!(offered(&evs).iter().all(|e| e.tool != "skills.load"));
}

#[tokio::test]
#[serial]
async fn an_unknown_skill_fails_assembly() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let l = launch(
        &home,
        agent_naming_a_missing_skill("anthropic"),
        Origin::Standalone,
        "run_bad",
    );
    match assembler.assemble(l).await {
        Err(e @ AssembleError::Skills(_)) => {
            assert_eq!(e.to_string(), "skills: unknown skill \"missing\"")
        }
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("assembled a run with an unknown skill"),
    }
}

/// Skills resolve before the provider is built: a run with an unknown skill
/// AND an undeclared provider fails on its skills.
#[tokio::test]
#[serial]
async fn skills_fail_before_the_provider_is_built() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let l = launch(
        &home,
        agent_naming_a_missing_skill("nope"),
        Origin::Standalone,
        "run_first",
    );
    match assembler.assemble(l).await {
        Err(AssembleError::Skills(_)) => {}
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("assembled a run with an unknown skill"),
    }
}

/// A run whose skills fail is refused before its loop starts, like one
/// whose grant fails: its (empty) transcript exists, and the sites that
/// record it as the run's failure get a preflight error.
#[tokio::test]
#[serial]
async fn failed_skills_leave_the_transcript_and_are_a_run_error() {
    let home = Home::new();
    let l = launch(
        &home,
        agent_naming_a_missing_skill("anthropic"),
        Origin::Standalone,
        "run_typo",
    );
    let transcript = l.transcript_path.clone();
    let err = match home.assembler(config()).assemble(l).await {
        Ok(_) => panic!("an unknown skill must not assemble"),
        Err(e) => e,
    };
    assert!(transcript.exists(), "the failed run's transcript exists");
    match err.into_run_error() {
        rupu_agent::RunError::Preflight(m) => {
            assert_eq!(m, "skills: unknown skill \"missing\"")
        }
        other => panic!("{other}"),
    }
}

/// A workflow step's factory records an assembly failure as the unstarted
/// run's transcript: a skills failure closes it with the skills message.
#[tokio::test]
#[serial]
async fn failed_skills_record_the_unstarted_run() {
    let home = Home::new();
    let l = launch(
        &home,
        agent_naming_a_missing_skill("anthropic"),
        Origin::Standalone,
        "run_unstarted",
    );
    let transcript = l.transcript_path.clone();
    let prompt = l.prompt.clone();
    let err = match home.assembler(config()).assemble(l).await {
        Ok(_) => panic!("an unknown skill must not assemble"),
        Err(e) => e,
    };
    let run_err = err.record_unstarted(rupu_agent::UnstartedRun {
        transcript_path: &transcript,
        run_id: "run_unstarted",
        workspace_id: "ws_test",
        agent: "probe",
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        mode: rupu_tools::PermissionMode::Bypass,
        codename: None,
        customer: None,
        system_prompt: Some("x"),
        prompt: &prompt,
    });
    assert!(
        matches!(&run_err, rupu_agent::RunError::Preflight(m) if m == "skills: unknown skill \"missing\""),
        "{run_err}"
    );
    let closed = events(&transcript)
        .into_iter()
        .find_map(|e| match e {
            Event::RunComplete { error, .. } => error,
            _ => None,
        })
        .expect("a run_complete with an error");
    assert_eq!(closed, "skills: unknown skill \"missing\"");
}

#[tokio::test]
#[serial]
async fn launch_overrides_add_skills() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(config());
    let spec = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6");
    let mut l = launch(&home, spec, Origin::Standalone, "run_step");
    l.overrides.skills = vec!["code-review".into()];
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let evs = events(&path);
    assert!(run_start_prompt(&evs).contains("- code-review: Review a diff."));
    assert!(offered(&evs).iter().any(|e| e.tool == "skills.load"));
}

/// The fs read tools reach a granted skill's files: `read_roots` is every
/// granted skill's canonical root.
#[tokio::test]
#[serial]
async fn read_roots_are_the_granted_skills_roots() {
    let home = Home::new();
    let dir = home.skill("code-review", "Review a diff.", "x");
    home.skill("unrelated", "Not granted.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_roots");
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    assert_eq!(
        run.opts.tool_context.workspace.read_roots,
        vec![dir.canonicalize().unwrap()]
    );
    let skills = run.opts.tool_context.services.skills.as_ref().unwrap();
    assert_eq!(skills.names(), vec!["code-review"]);
}
