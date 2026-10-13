//! A run's skills (spec 2026-10-12 §2.4, §3): the assembler resolves the
//! agent's and the launch's skills, renders them into the system prompt and
//! offers `skills.load`; a run never starts without a skill it was given.

use rupu_runtime::assembly::{AssembleError, Origin};
use rupu_transcript::{Event, SkillIndexEntry};
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

fn installed(home: &Home, name: &str) -> rupu_skills::Skill {
    rupu_skills::load_skill(
        &home.global().join("skills").join(name),
        rupu_skills::Layer::Global,
    )
    .unwrap()
}

fn skill_index(evs: &[Event]) -> Vec<SkillIndexEntry> {
    let mut found = evs.iter().filter_map(|e| match e {
        Event::SkillIndex { skills } => Some(skills.clone()),
        _ => None,
    });
    let index = found.next().expect("a skill_index");
    assert!(found.next().is_none(), "one skill_index per run");
    index
}

/// `(name, layer, content_hash, base_dir, via)` of every `skill_load`.
fn skill_loads(evs: &[Event]) -> Vec<(String, String, String, String, String)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::SkillLoad {
                name,
                layer,
                content_hash,
                base_dir,
                via,
            } => Some((
                name.clone(),
                layer.clone(),
                content_hash.clone(),
                base_dir.clone(),
                via.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// `message` of every notice of `kind`.
fn notices(evs: &[Event], kind: &str) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            Event::Notice { kind: k, message } if k == kind => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// Rewrite an installed skill's `SKILL.md`.
fn rewrite(dir: &std::path::Path, name: &str, frontmatter: &str, body: &str) {
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d.\n{frontmatter}---\n{body}\n"),
    )
    .unwrap();
}

fn tool_use(id: &str, tool: &str, input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "AssistantToolUse": {
        "text": null, "tool_id": id, "tool_name": tool, "tool_input": input, "stop": "tool_use"
    } })
}

fn script(turns: Vec<serde_json::Value>) -> String {
    let mut turns = turns;
    turns.push(serde_json::json!({ "AssistantText": { "text": "done", "stop": "end_turn" } }));
    serde_json::Value::Array(turns).to_string()
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

    // The index follows the grant; the preloaded skill is recorded as
    // loaded, since its body is in the system prompt.
    let kinds: Vec<String> = evs
        .iter()
        .map(|e| {
            serde_json::to_value(e).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let at = |k: &str| kinds.iter().position(|x| x == k).unwrap();
    assert!(at("tool_grant") < at("skill_index"), "{kinds:?}");
    assert!(at("skill_index") < at("skill_load"), "{kinds:?}");
    let review = installed(&home, "code-review");
    let style = installed(&home, "report-style");
    assert_eq!(
        skill_index(&evs),
        vec![
            SkillIndexEntry {
                name: "code-review".into(),
                layer: "global".into(),
                stamp: review.stamp.hex(),
                preload: false,
                indexed: true,
                reason: "agent".into(),
            },
            SkillIndexEntry {
                name: "report-style".into(),
                layer: "global".into(),
                stamp: style.stamp.hex(),
                preload: true,
                indexed: false,
                reason: "agent".into(),
            },
        ]
    );
    assert_eq!(
        skill_loads(&evs),
        vec![(
            "report-style".into(),
            "global".into(),
            style.content_hash().unwrap().hex(),
            style.root.display().to_string(),
            "preload".into(),
        )]
    );
    assert!(notices(&evs, "skill_not_indexed").is_empty());
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
    let index = skill_index(&evs);
    assert_eq!(index.len(), 1);
    assert_eq!(
        (index[0].name.as_str(), index[0].reason.as_str()),
        ("code-review", "step")
    );
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

/// The whole path a skill's instructions take: the model loads it, reads a
/// reference under its base directory and runs its script with `bash`.
#[tokio::test]
#[serial]
async fn a_model_loads_a_skill_reads_its_reference_and_runs_its_script() {
    let home = Home::new();
    let dir = home.skill("pdf", "PDF things.", "Run scripts/hello.sh.");
    let root = dir.canonicalize().unwrap();
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [read_file, bash]\nskills: [pdf]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_use");
    assert_eq!(l.mode, rupu_tools::PermissionMode::Bypass, "bash must run");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(&script(vec![
        tool_use("c1", "skills.load", serde_json::json!({ "name": "pdf" })),
        tool_use(
            "c2",
            "read_file",
            serde_json::json!({ "path": root.join("references/REF.md") }),
        ),
        tool_use(
            "c3",
            "bash",
            serde_json::json!({
                "command": format!("/bin/sh {}", root.join("scripts/hello.sh").display())
            }),
        ),
    ]));
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let evs = events(&path);
    let pdf = installed(&home, "pdf");
    assert_eq!(
        skill_loads(&evs),
        vec![(
            "pdf".into(),
            "global".into(),
            pdf.content_hash().unwrap().hex(),
            root.display().to_string(),
            "model".into(),
        )]
    );
    let results: Vec<(String, Option<String>)> = evs
        .iter()
        .filter_map(|e| match e {
            Event::ToolResult { output, error, .. } => Some((output.clone(), error.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(results.iter().all(|(_, e)| e.is_none()), "{results:?}");
    assert!(
        results[0].0.contains("Run scripts/hello.sh."),
        "{results:?}"
    );
    assert!(results[1].0.contains("reference text"), "{results:?}");
    assert!(results[2].0.contains("hello-from-skill"), "{results:?}");
    assert!(notices(&evs, "skill_changed").is_empty());
    assert!(notices(&evs, "skill_tools_missing").is_empty());
}

/// A skill that names tools the run was not granted still loads, with a
/// notice naming them (spec §3.5).
#[tokio::test]
#[serial]
async fn a_skill_naming_ungranted_tools_writes_a_notice() {
    let home = Home::new();
    let dir = home.skill("jq-tricks", "Query JSON.", "x");
    rewrite(
        &dir,
        "jq-tricks",
        "allowed-tools: Bash(jq:*) Read Write\n",
        "Use jq.",
    );
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [read_file]\nskills: [jq-tricks]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_missing");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(&script(vec![tool_use(
        "c1",
        "skills.load",
        serde_json::json!({ "name": "jq-tricks" }),
    )]));
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let evs = events(&path);
    assert_eq!(skill_loads(&evs).len(), 1);
    assert_eq!(
        notices(&evs, "skill_tools_missing"),
        vec![
            "skill jq-tricks expects tools this run was not granted: bash, write_file".to_string()
        ]
    );
}

/// A granted `disable-model-invocation` skill the run does not preload is
/// never offered to the model; the run says so at start (spec §3.2).
#[tokio::test]
#[serial]
async fn a_granted_skill_the_model_cannot_load_writes_a_notice() {
    let home = Home::new();
    let dir = home.skill("deploy", "Deploy.", "x");
    rewrite(
        &dir,
        "deploy",
        "disable-model-invocation: true\n",
        "Ship it.",
    );
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [deploy, code-review]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_hidden");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let evs = events(&path);
    let index = skill_index(&evs);
    let deploy = index.iter().find(|e| e.name == "deploy").unwrap();
    assert!(!deploy.indexed && !deploy.preload);
    assert_eq!(
        notices(&evs, "skill_not_indexed"),
        vec![
            "skill deploy is granted but marked disable-model-invocation, so the model is \
             never offered it; preload it or invoke it from a session"
                .to_string()
        ]
    );
    assert!(skill_loads(&evs).is_empty());
}

/// A skill edited after the run started loads at its current version, with
/// a notice (Review Focus 2).
#[tokio::test]
#[serial]
async fn a_skill_edited_since_the_run_started_loads_its_new_version() {
    let home = Home::new();
    let dir = home.skill("pdf", "PDF things.", "Old instructions.");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [pdf]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_edit");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(&script(vec![tool_use(
        "c1",
        "skills.load",
        serde_json::json!({ "name": "pdf" }),
    )]));
    let run = assembler.assemble(l).await.unwrap();
    let before = installed(&home, "pdf");
    rewrite(&dir, "pdf", "", "New and longer instructions.");
    let after = installed(&home, "pdf");
    assert_ne!(before.stamp, after.stamp);
    run.run().await.result.unwrap();
    let evs = events(&path);
    assert_eq!(skill_index(&evs)[0].stamp, before.stamp.hex());
    let loads = skill_loads(&evs);
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0].2, after.content_hash().unwrap().hex());
    let output = evs
        .iter()
        .find_map(|e| match e {
            Event::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .unwrap();
    assert!(output.contains("New and longer instructions."), "{output}");
    assert_eq!(
        notices(&evs, "skill_changed"),
        vec![
            "skill pdf changed on disk since this run started; the model was given its \
             current version"
                .to_string()
        ]
    );
}
