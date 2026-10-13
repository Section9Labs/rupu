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

/// The assembler's message for an agent naming the uninstalled `missing`,
/// in a run with no project layer.
fn unknown_missing(home: &Home) -> String {
    format!(
        "skills: unknown skill \"missing\" (looked in {}; install it with `rupu skill install`)",
        home.global().join("skills").display()
    )
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
                shipped: false,
            },
            SkillIndexEntry {
                name: "report-style".into(),
                layer: "global".into(),
                stamp: style.stamp.hex(),
                preload: true,
                indexed: false,
                reason: "agent".into(),
                shipped: false,
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
        Err(e @ AssembleError::Skills { .. }) => assert_eq!(e.to_string(), unknown_missing(&home)),
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
        Err(AssembleError::Skills { .. }) => {}
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
        rupu_agent::RunError::Preflight(m) => assert_eq!(m, unknown_missing(&home)),
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
        launch_skills: &[],
        launch_preload_skills: &[],
    });
    assert!(
        matches!(&run_err, rupu_agent::RunError::Preflight(m) if *m == unknown_missing(&home)),
        "{run_err}"
    );
    let closed = events(&transcript)
        .into_iter()
        .find_map(|e| match e {
            Event::RunComplete { error, .. } => error,
            _ => None,
        })
        .expect("a run_complete with an error");
    assert_eq!(closed, unknown_missing(&home));
}

/// A site that resolves a launch's skills before assembling it (`rupu run`,
/// before a clone) records a failure as `assemble` does: the run's
/// transcript exists, empty. A launch whose skills resolve writes nothing.
#[tokio::test]
#[serial]
async fn skills_resolved_for_a_launch_record_a_failure_as_assemble_does() {
    let home = Home::new();
    home.skill("pdf", "Read PDFs.", "Use pdftotext.");
    let assembler = home.assembler(config());
    let transcript = home.global().join("transcripts/run_early.jsonl");

    let err = match assembler
        .resolve_skills_for_launch(
            &agent_naming_a_missing_skill("anthropic"),
            &rupu_runtime::assembly::Overrides::default(),
            &transcript,
        )
        .await
    {
        Ok(_) => panic!("an unknown skill must not resolve"),
        Err(e) => e,
    };
    assert_eq!(err.to_string(), unknown_missing(&home));
    assert_eq!(std::fs::read_to_string(&transcript).unwrap(), "");

    std::fs::remove_file(&transcript).unwrap();
    let overrides = rupu_runtime::assembly::Overrides {
        skills: vec!["pdf".into()],
        ..Default::default()
    };
    let skills = assembler
        .resolve_skills_for_launch(
            &agent(&home, "provider: anthropic"),
            &overrides,
            &transcript,
        )
        .await
        .unwrap()
        .expect("the launch's skill");
    assert_eq!(skills.names(), ["pdf"]);
    assert!(!transcript.exists(), "a resolved launch writes nothing");
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
        ("code-review", "launch")
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

/// The skill_load of the transcript's preloaded skills.
fn preload_hashes(evs: &[Event]) -> Vec<(String, String)> {
    skill_loads(evs)
        .into_iter()
        .filter(|l| l.4 == "preload")
        .map(|l| (l.0, l.2))
        .collect()
}

/// A preloaded skill edited between assembly and the run: the recorded
/// version is the one whose body the system prompt carries (taken when the
/// run resolved its skills), never the edited one.
#[tokio::test]
#[serial]
async fn a_preload_records_the_version_its_prompt_body_has() {
    let home = Home::new();
    let dir = home.skill("report-style", "House style.", "Old style.");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\npreloadSkills: [report-style]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_pre_edit");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    let before = installed(&home, "report-style").content_hash().unwrap();
    rewrite(&dir, "report-style", "", "New style.");
    run.run().await.result.unwrap();
    let evs = events(&path);
    let prompt = run_start_prompt(&evs);
    assert!(prompt.contains("Old style."), "{prompt}");
    assert!(!prompt.contains("New style."), "{prompt}");
    assert_eq!(
        preload_hashes(&evs),
        vec![("report-style".to_string(), before.hex())]
    );
}

/// An unknown skill in a run with a project layer names both directories.
#[tokio::test]
#[serial]
async fn an_unknown_skill_names_both_layers_it_looked_in() {
    let home = Home::new();
    let project = home.root().join("proj");
    std::fs::create_dir_all(project.join(".rupu")).unwrap();
    let mut ctx = home.context(config());
    ctx.project_root = Some(project.clone());
    let assembler = rupu_runtime::assembly::RunAssembler::new(ctx);
    let l = launch(
        &home,
        agent_naming_a_missing_skill("anthropic"),
        Origin::Standalone,
        "run_both",
    );
    let err = match assembler.assemble(l).await {
        Err(e) => e,
        Ok(_) => panic!("assembled a run with an unknown skill"),
    };
    assert_eq!(
        err.to_string(),
        format!(
            "skills: unknown skill \"missing\" (looked in {} and {}; install it with `rupu skill install`)",
            home.global().join("skills").display(),
            project.join(".rupu/skills").display()
        )
    );
}

/// Only an unknown skill gets the install hint.
#[tokio::test]
#[serial]
async fn an_ambiguous_skill_gets_no_install_hint() {
    let home = Home::new();
    home.skill("x", "Global x.", "g");
    let project = home.root().join("proj");
    let local = project.join(".rupu/skills/x");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(
        local.join("SKILL.md"),
        "---\nname: x\ndescription: p\n---\np\n",
    )
    .unwrap();
    let mut ctx = home.context(config());
    ctx.project_root = Some(project);
    let assembler = rupu_runtime::assembly::RunAssembler::new(ctx);
    let spec = rupu_agent::AgentSpec::parse(
        "---\nname: probe\nprovider: anthropic\nmodel: claude-sonnet-4-6\nskills: [x]\n---\nx\n",
    )
    .unwrap();
    let err = match assembler
        .assemble(launch(&home, spec, Origin::Standalone, "run_amb"))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("assembled an ambiguous grant"),
    };
    assert_eq!(
        err.to_string(),
        "skills: skill \"x\" is installed in both the global and the project layer; \
         grant global:x or project:x"
    );
}

/// A run whose every skill is `disable-model-invocation` and not preloaded
/// keeps its skills (index row, read roots) but is not offered the loader,
/// not even under `*`.
#[tokio::test]
#[serial]
async fn a_run_with_no_model_loadable_skill_gets_no_loader() {
    let home = Home::new();
    let dir = home.skill("deploy", "Deploy.", "x");
    rewrite(
        &dir,
        "deploy",
        "disable-model-invocation: true\n",
        "Ship it.",
    );
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [\"*\"]\nskills: [deploy]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_noload");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    assert!(run.opts.tool_context.services.skills.is_some());
    assert_eq!(
        run.opts.tool_context.workspace.read_roots,
        vec![dir.canonicalize().unwrap()]
    );
    run.run().await.result.unwrap();
    let evs = events(&path);
    assert!(offered(&evs).iter().all(|e| e.tool != "skills.load"));
    assert_eq!(skill_index(&evs).len(), 1);
}

/// Spec §3.1: the agent's body, then `## Run target`, then the skills
/// section; and the same agent and skills assemble a byte-identical prompt.
#[tokio::test]
#[serial]
async fn the_skills_section_follows_the_run_target_and_is_byte_stable() {
    let home = Home::new();
    home.skill("report-style", "House style.", "Write plainly.");
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review]\npreloadSkills: [report-style]",
    );
    let prompt_of = |run_id: &str| {
        let mut l = launch(&home, spec.clone(), Origin::Standalone, run_id);
        l.overrides.system_prompt_suffix = Some("repo: acme/widgets".into());
        l
    };
    let _m = MockScript::set(DONE);
    let first = assembler.assemble(prompt_of("run_a")).await.unwrap();
    let second = assembler.assemble(prompt_of("run_b")).await.unwrap();
    let prompt = first.opts.system_prompt.clone();
    assert_eq!(prompt, second.opts.system_prompt);
    let style = installed(&home, "report-style");
    assert_eq!(
        prompt,
        format!(
            "you are probe.\n\n\n## Run target\n\nrepo: acme/widgets\n\n## Skill: report-style\n\n\
             Base directory: {}\n\nWrite plainly.\n\n## Skills\n\n\
             Each skill below holds instructions, and sometimes scripts and reference files,\n\
             for one kind of task. When a task matches a skill's description, call\n\
             `skills.load` with its name before you start, and follow it. Paths inside a\n\
             skill are relative to the base directory that `skills.load` returns.\n\n\
             - code-review: Review a diff.",
            style.root.display()
        )
    );
}

/// A project skill granted by its exact name reaches the run: indexed,
/// recorded with its layer, its root a read root.
#[tokio::test]
#[serial]
async fn a_project_skill_granted_exactly_reaches_the_run() {
    let home = Home::new();
    let project = home.root().join("proj");
    let local = project.join(".rupu/skills/house-rules");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(
        local.join("SKILL.md"),
        "---\nname: house-rules\ndescription: This repo's rules.\n---\nFollow them.\n",
    )
    .unwrap();
    let mut ctx = home.context(config());
    ctx.project_root = Some(project);
    let assembler = std::sync::Arc::new(rupu_runtime::assembly::RunAssembler::new(ctx));
    let spec = rupu_agent::AgentSpec::parse(
        "---\nname: probe\nprovider: anthropic\nmodel: claude-sonnet-4-6\nskills: [\"project:house-rules\"]\n---\nyou are probe.\n",
    )
    .unwrap();
    let l = launch(&home, spec, Origin::Standalone, "run_project");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    assert_eq!(
        run.opts.tool_context.workspace.read_roots,
        vec![local.canonicalize().unwrap()]
    );
    run.run().await.result.unwrap();
    let evs = events(&path);
    assert!(run_start_prompt(&evs).contains("- house-rules: This repo's rules."));
    let index = skill_index(&evs);
    assert_eq!(
        (index[0].name.as_str(), index[0].layer.as_str()),
        ("house-rules", "project")
    );
}

/// R5: a project whose `.rupu` is the global dir has no project layer, so
/// a global skill binds once by its bare name instead of being ambiguous.
#[tokio::test]
#[serial]
async fn a_project_rupu_that_is_the_global_dir_is_no_second_layer() {
    let home = Home::new();
    let user = home.root().join("user");
    let global = user.join(".rupu");
    let d = global.join("skills/pdf");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        "---\nname: pdf\ndescription: PDF.\n---\nx\n",
    )
    .unwrap();
    let mut ctx = home.context(config());
    ctx.global = global;
    ctx.project_root = Some(user);
    let assembler = rupu_runtime::assembly::RunAssembler::new(ctx);
    let spec = rupu_agent::AgentSpec::parse(
        "---\nname: probe\nprovider: anthropic\nmodel: claude-sonnet-4-6\nskills: [pdf]\n---\nx\n",
    )
    .unwrap();
    let _m = MockScript::set(DONE);
    let run = assembler
        .assemble(launch(&home, spec, Origin::Standalone, "run_r5"))
        .await
        .unwrap();
    let skills = run.opts.tool_context.services.skills.as_ref().unwrap();
    assert_eq!(skills.names(), vec!["pdf"]);
    assert_eq!(skills.entries()[0].skill.layer, rupu_skills::Layer::Global);
}

/// A `skills.load` that fails (a name the run was not given) records no
/// load: the loaded set and the transcript hold only what entered the
/// conversation.
#[tokio::test]
#[serial]
async fn a_failed_skills_load_records_no_load() {
    let home = Home::new();
    home.skill("pdf", "PDF things.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [pdf]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_badload");
    let path = l.transcript_path.clone();
    let _m = MockScript::set(&script(vec![tool_use(
        "c1",
        "skills.load",
        serde_json::json!({ "name": "nope" }),
    )]));
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let evs = events(&path);
    let error = evs
        .iter()
        .find_map(|e| match e {
            Event::ToolResult { error, .. } => error.clone(),
            _ => None,
        })
        .expect("the load failed");
    assert!(error.contains("available: pdf"), "{error}");
    assert!(skill_loads(&evs).is_empty());
}

/// A workflow step's run gets its agent's skills like any run.
#[tokio::test]
#[serial]
async fn a_workflow_step_run_gets_its_agents_skills() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review]",
    );
    let origin = Origin::WorkflowStep {
        workflow_run_id: String::new(),
        workflow_name: "review".into(),
        step_id: "assess".into(),
        unit: None,
        step_actions: Vec::new(),
        scope_override: None,
        messaging: None,
    };
    let l = launch(&home, spec, origin, "run_wf_step");
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
    assert_eq!(skill_index(&evs)[0].reason, "agent");
}

/// A run granted a skill whose SKILL.md is over the size limit fails
/// closed, with the reason.
#[tokio::test]
#[serial]
async fn an_oversized_skill_fails_assembly_with_its_size() {
    let home = Home::new();
    let dir = home.skill("huge", "d.", "x");
    let text = format!(
        "---\nname: huge\ndescription: d.\n---\n{}\n",
        "x".repeat(1 << 20)
    );
    std::fs::write(dir.join("SKILL.md"), &text).unwrap();
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [huge]",
    );
    let l = launch(&home, spec, Origin::Standalone, "run_huge");
    match home.assembler(config()).assemble(l).await {
        Err(e @ AssembleError::Skills { .. }) => assert_eq!(
            e.to_string(),
            format!(
                "skills: skill \"huge\" (global) is invalid: SKILL.md is {} bytes; \
                 the limit is 1048576 bytes",
                text.len()
            )
        ),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("assembled a run with an oversized skill"),
    }
}

/// The launch's own skills are recorded in the run's coverage manifest as
/// written, so `rupu coverage rerun` passes them again; the agent's own are
/// not (the rerun's agent grants them).
#[tokio::test]
#[serial]
async fn the_coverage_manifest_records_the_launchs_skills() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    home.skill("house-style", "House style.", "y");
    home.skill("pdf", "PDFs.", "z");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [pdf]\nconcerns:\n  - include: stride",
    );
    let mut l = launch(&home, spec, Origin::Standalone, "run_manifest_skills");
    l.overrides.skills = vec!["code-review".into()];
    l.overrides.preload_skills = vec!["house-style".into()];
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.expect("assembles");
    let paths = rupu_tools::ledger::paths(&run.opts.tool_context);
    run.run().await.result.unwrap();
    let manifest = rupu_coverage::find_manifest(&paths, "run_manifest_skills")
        .unwrap()
        .expect("the run's manifest");
    assert_eq!(manifest.skills, vec!["code-review".to_string()]);
    assert_eq!(manifest.preload_skills, vec!["house-style".to_string()]);
}

/// A session's turns send the skills section its first turn rendered
/// (`Overrides::skill_section`), not one rendered from the skills as they
/// are now; the run still gets its skills and the loader.
#[tokio::test]
#[serial]
async fn a_frozen_section_replaces_the_rendered_one() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review]",
    );
    let skills = assembler
        .resolve_skills(&spec, &rupu_runtime::assembly::Overrides::default())
        .await
        .unwrap()
        .expect("the agent's skill");
    let mut l = launch(&home, spec, Origin::Standalone, "run_frozen");
    l.services.skills = Some(skills);
    l.overrides.skill_section = Some("## Skills\n\nFROZEN".into());
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    assert_eq!(
        run.opts.system_prompt,
        "you are probe.\n\n\n## Skills\n\nFROZEN"
    );
    run.run().await.result.unwrap();
    let evs = events(&path);
    let sent = run_start_prompt(&evs);
    assert!(
        sent.starts_with("you are probe.\n\n\n## Skills\n\nFROZEN")
            && !sent.contains("- code-review:"),
        "{sent}"
    );
    assert!(offered(&evs).iter().any(|e| e.tool == "skills.load"));
    assert_eq!(skill_index(&evs)[0].name, "code-review");
}

/// An empty frozen section appends nothing, not even the separator.
#[tokio::test]
#[serial]
async fn an_empty_frozen_section_appends_nothing() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review]",
    );
    let mut l = launch(&home, spec, Origin::Standalone, "run_frozen_empty");
    l.overrides.skill_section = Some(String::new());
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    assert_eq!(run.opts.system_prompt, "you are probe.\n");
}

/// A session's own `--skill` entries are recorded with reason `session`.
#[tokio::test]
#[serial]
async fn resolve_agent_skills_records_a_sessions_entries_as_session() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    home.skill("report-style", "House style.", "x");
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [report-style]",
    );
    let overrides = rupu_runtime::assembly::Overrides {
        skills: vec!["code-review".into()],
        ..Default::default()
    };
    let dirs = rupu_skills::LayerDirs::for_layers(&home.global(), None);
    let skills = rupu_runtime::assembly::resolve_agent_skills(
        &dirs,
        &spec,
        &overrides,
        rupu_skills::SkillReason::Session,
    )
    .await
    .unwrap()
    .expect("the session's skill");
    let mut l = launch(&home, spec, Origin::Standalone, "run_session_reason");
    l.services.skills = Some(skills);
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let index = skill_index(&events(&path));
    assert_eq!(
        index
            .iter()
            .map(|e| (e.name.as_str(), e.reason.as_str()))
            .collect::<Vec<_>>(),
        vec![("code-review", "session"), ("report-style", "agent")]
    );
}

/// A session turn thaws its frozen set against the session's layers: what
/// is still installed is given, what is gone comes back with why.
#[tokio::test]
#[serial]
async fn resolve_frozen_skills_thaws_against_the_sessions_layers() {
    let home = Home::new();
    home.skill("code-review", "Review a diff.", "x");
    home.skill("pdf", "Read a PDF.", "x");
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [code-review, pdf]",
    );
    let dirs = rupu_skills::LayerDirs::for_layers(&home.global(), None);
    let frozen = rupu_runtime::assembly::resolve_agent_skills(
        &dirs,
        &spec,
        &Default::default(),
        rupu_skills::SkillReason::Session,
    )
    .await
    .unwrap()
    .expect("the agent's skills")
    .freeze();
    std::fs::remove_dir_all(home.global().join("skills/pdf")).unwrap();
    let (skills, missing) = rupu_runtime::assembly::resolve_frozen_skills(&dirs, &frozen)
        .await
        .unwrap();
    assert_eq!(skills.expect("code-review").names(), vec!["code-review"]);
    assert_eq!(
        missing,
        vec![rupu_skills::MissingSkill {
            name: "pdf".into(),
            layer: rupu_skills::Layer::Global,
            reason: rupu_skills::MissingReason::Removed,
        }]
    );
    let (none, missing) = rupu_runtime::assembly::resolve_frozen_skills(&dirs, &[])
        .await
        .unwrap();
    assert!(none.is_none() && missing.is_empty());
}

fn shipped_pdf(dir: &std::path::Path) -> rupu_skills::OverlaySkill {
    let d = dir.join("pdf");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        "---\nname: pdf\ndescription: Shipped pdf.\n---\nshipped body\n",
    )
    .unwrap();
    rupu_skills::OverlaySkill {
        layer: rupu_skills::Layer::Global,
        name: "pdf".into(),
        dir: d,
    }
}

#[tokio::test]
#[serial]
async fn a_shipped_overlay_skill_resolves_and_is_recorded_shipped() {
    let home = Home::new();
    let shipped = tempfile::tempdir().unwrap();
    let overlay = shipped_pdf(shipped.path());
    let assembler = home.assembler(config());
    home.agent(
        "probe",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nskills: [pdf]",
    );
    // The agent loads only because the overlay stands in for the install.
    let spec = rupu_agent::load_agent_in(
        &home.global(),
        None,
        "probe",
        std::slice::from_ref(&overlay),
    )
    .unwrap();

    let resolved = assembler
        .resolve_skills(
            &spec,
            &rupu_runtime::assembly::Overrides {
                skill_overlay: vec![overlay.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .expect("a skill set");
    let g = &resolved.entries()[0];
    assert!(g.skill.shipped);
    assert_eq!(g.skill.dir, overlay.dir);

    let mut l = launch(&home, spec, Origin::Standalone, "run_shipped");
    l.overrides.skill_overlay = vec![overlay];
    let path = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let index = skill_index(&events(&path));
    assert_eq!(index.len(), 1);
    assert!(index[0].shipped);
}

#[tokio::test]
async fn no_requested_skills_reads_no_layer() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("skills"), "a file, not a directory").unwrap();
    let dirs = rupu_skills::LayerDirs::for_layers(d.path(), None);
    let requests = vec![
        (rupu_skills::SkillReason::Agent, vec![], vec![]),
        (rupu_skills::SkillReason::Launch, vec![], vec![]),
    ];
    let got = rupu_runtime::assembly::resolve_skill_requests(&dirs, "a", requests)
        .await
        .unwrap();
    assert!(got.is_none());
}
