//! A run's loaded skills survive compaction (spec 2026-10-12 §6): the
//! rebuilt first message re-attaches each at its current version, once,
//! within a cap; a continued run starts with the skills its predecessor
//! loaded.

use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, LoadedSkill};
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::{ContentBlock, Message, Role, StopReason, Usage};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SUMMARY: &str = "SUMMARY-MARKER: condensed earlier work";

fn usage(input: u32) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: 5,
        ..Default::default()
    }
}

fn install(global: &Path, name: &str, body: &str) -> PathBuf {
    let d = global.join("skills").join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d.\n---\n{body}\n"),
    )
    .unwrap();
    d
}

fn resolved(global: &Path, grants: &[&str]) -> Arc<rupu_skills::ResolvedSkills> {
    let catalog =
        rupu_skills::SkillCatalog::discover(&rupu_skills::LayerDirs::for_layers(global, None))
            .unwrap();
    let grants: Vec<String> = grants.iter().map(|s| s.to_string()).collect();
    Arc::new(
        rupu_skills::resolve_run_skills(
            &catalog,
            &[rupu_skills::SkillRequest {
                reason: rupu_skills::SkillReason::Agent,
                grants: &grants,
                preload: &[],
            }],
        )
        .unwrap(),
    )
}

fn load_turn(id: &str, name: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantBlocksWithUsage {
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: "skills.load".into(),
            input: serde_json::json!({ "name": name }),
        }],
        stop: StopReason::ToolUse,
        usage: usage(10),
    }
}

/// A `read_file` turn billed at `input` tokens: over 500 it triggers a
/// compaction (the fixture's threshold).
fn read_turn(id: &str, input: u32) -> ScriptedTurn {
    ScriptedTurn::AssistantBlocksWithUsage {
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: "read_file".into(),
            input: serde_json::json!({ "path": "notes.txt" }),
        }],
        stop: StopReason::ToolUse,
        usage: usage(input),
    }
}

fn summary_turn() -> ScriptedTurn {
    ScriptedTurn::AssistantTextWithUsage {
        text: SUMMARY.into(),
        stop: StopReason::EndTurn,
        usage: usage(100),
    }
}

fn done_turn() -> ScriptedTurn {
    ScriptedTurn::AssistantTextWithUsage {
        text: "all done".into(),
        stop: StopReason::EndTurn,
        usage: usage(60),
    }
}

fn dense(role: Role, label: &str) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: format!("{label}: {}", "x".repeat(1000)),
        }],
    }
}

/// A run over `skills` in `ws` whose compaction threshold is 500 tokens of a
/// 1,000-token input limit, so the re-attach cap is 1,000 characters.
fn opts(
    provider: MockProvider,
    ws: &Path,
    transcript: PathBuf,
    skills: Arc<rupu_skills::ResolvedSkills>,
) -> AgentRunOpts {
    std::fs::write(ws.join("notes.txt"), "invented notes\n").unwrap();
    let mut tc = ToolContext::in_workspace(ws);
    tc.identity = Arc::new(rupu_tools::RunIdentity {
        agent: "reader".into(),
        provider: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_skills_compaction".into(),
        ..Default::default()
    });
    tc.workspace.read_roots = skills.read_roots();
    tc.services.skills = Some(skills);
    let mut prompt = rupu_agent::UserTurn::new("go");
    prompt.initial_messages = vec![
        dense(Role::User, "task"),
        dense(Role::Assistant, "assistant 0"),
        dense(Role::User, "user 0"),
        dense(Role::Assistant, "assistant 1"),
    ];
    rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "You read notes.".into(),
            prompt,
            provider: Box::new(provider),
            limits: ModelLimits::unknown().with_input(1000).with_percent(50),
            recovery: Default::default(),
            permission: rupu_tools::PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: tc,
            pins: Default::default(),
            concerns: None,
            max_turns: 20,
            stream: rupu_agent::StreamOpts {
                no_stream: true,
                suppress_stdout: true,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: Vec::new(),
            transcript_path: transcript,
        },
        Some(&["read_file".to_string()]),
        &[],
    )
    .expect("grant")
}

fn events(path: &Path) -> Vec<Event> {
    JsonlReader::iter(path).unwrap().flatten().collect()
}

/// The text of each compaction's rebuilt first message.
fn compacted_first_messages(evs: &[Event]) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            Event::Compaction { messages, .. } => {
                let messages: Vec<Message> = serde_json::from_value(messages.clone()).unwrap();
                Some(
                    messages[0]
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
            _ => None,
        })
        .collect()
}

/// `(name, via)` of every `skill_load`, and the kind of every compaction, in
/// transcript order.
fn timeline(evs: &[Event]) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            Event::SkillLoad { name, via, .. } => Some(format!("{via}:{name}")),
            Event::Compaction { .. } => Some("compaction".into()),
            _ => None,
        })
        .collect()
}

fn notices(evs: &[Event], kind: &str) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            Event::Notice { kind: k, message } if k == kind => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// Review Focus 3: two compactions in one run carry the loaded skill once
/// each, never twice.
#[tokio::test]
async fn a_loaded_skill_survives_two_compactions_once() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    install(global.path(), "pdf", "PDF STEPS");
    let transcript = ws.path().join("run.jsonl");
    let provider = MockProvider::new(vec![
        load_turn("c1", "pdf"),
        read_turn("c2", 900),
        summary_turn(),
        read_turn("c3", 10),
        read_turn("c4", 900),
        summary_turn(),
        done_turn(),
    ]);
    run_agent(opts(
        provider,
        ws.path(),
        transcript.clone(),
        resolved(global.path(), &["pdf"]),
    ))
    .await
    .unwrap();
    let evs = events(&transcript);
    let firsts = compacted_first_messages(&evs);
    assert_eq!(firsts.len(), 2, "{:?}", timeline(&evs));
    for first in &firsts {
        assert_eq!(
            first.matches(rupu_skills::render::REATTACH_MARKER).count(),
            1,
            "{first}"
        );
        assert_eq!(first.matches("PDF STEPS").count(), 1, "{first}");
    }
    assert_eq!(
        timeline(&evs),
        vec![
            "model:pdf",
            "compaction",
            "reattach:pdf",
            "compaction",
            "reattach:pdf"
        ]
    );
    assert!(notices(&evs, "skill_changed").is_empty());
}

/// A skill edited after it was loaded comes back at its current version,
/// with a notice; one deleted is listed as missing (it stays in the loaded
/// set, so a reinstall re-attaches it at the next compaction).
#[tokio::test]
async fn reattach_reads_the_current_version_and_lists_a_missing_skill() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let pdf = install(global.path(), "pdf", "OLD STEPS");
    let gone = install(global.path(), "gone", "GONE STEPS");
    let skills = resolved(global.path(), &["pdf", "gone"]);
    let transcript = ws.path().join("run.jsonl");
    let provider = MockProvider::new(vec![
        load_turn("c1", "pdf"),
        load_turn("c2", "gone"),
        read_turn("c3", 900),
        summary_turn(),
        done_turn(),
    ]);
    let mut o = opts(provider, ws.path(), transcript.clone(), skills);
    // The turn that triggers the compaction reports its usage first: edit
    // one skill and delete the other between their loads and the re-attach.
    o.hooks.on_usage = Some(Arc::new(move |u: &rupu_agent::UsageTurn| {
        if u.input_tokens == 900 {
            std::fs::write(
                pdf.join("SKILL.md"),
                "---\nname: pdf\ndescription: d.\n---\nNEW STEPS\n",
            )
            .unwrap();
            std::fs::remove_dir_all(&gone).unwrap();
        }
    }));
    run_agent(o).await.unwrap();
    let evs = events(&transcript);
    let firsts = compacted_first_messages(&evs);
    assert_eq!(firsts.len(), 1, "{:?}", timeline(&evs));
    assert!(firsts[0].contains("NEW STEPS"), "{}", firsts[0]);
    assert!(!firsts[0].contains("GONE STEPS"), "{}", firsts[0]);
    assert!(
        firsts[0].contains("\nNo longer installed: gone\n"),
        "{}",
        firsts[0]
    );
    assert_eq!(
        timeline(&evs),
        vec!["model:pdf", "model:gone", "compaction", "reattach:pdf"]
    );
    assert_eq!(
        notices(&evs, "skill_changed"),
        vec![
            "skill pdf changed on disk since it was loaded; its current version was re-attached"
                .to_string()
        ]
    );
    assert_eq!(
        notices(&evs, "skill_missing"),
        vec!["skill gone is no longer installed and was not re-attached".to_string()]
    );
}

/// Re-attached bodies are capped (a quarter of the input limit, in
/// characters): the most recently loaded skill comes back, an older one
/// that no longer fits is named for the model to reload.
#[tokio::test]
async fn the_cap_omits_older_skills() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    install(global.path(), "older", &"A".repeat(700));
    install(global.path(), "newer", &"B".repeat(700));
    let transcript = ws.path().join("run.jsonl");
    let provider = MockProvider::new(vec![
        load_turn("c1", "older"),
        load_turn("c2", "newer"),
        read_turn("c3", 900),
        summary_turn(),
        done_turn(),
    ]);
    run_agent(opts(
        provider,
        ws.path(),
        transcript.clone(),
        resolved(global.path(), &["older", "newer"]),
    ))
    .await
    .unwrap();
    let evs = events(&transcript);
    let firsts = compacted_first_messages(&evs);
    assert_eq!(firsts.len(), 1, "{:?}", timeline(&evs));
    assert!(firsts[0].contains(&"B".repeat(700)), "{}", firsts[0]);
    assert!(!firsts[0].contains(&"A".repeat(700)), "{}", firsts[0]);
    assert!(
        firsts[0].contains(
            "\nNot re-attached for space (reload with `skills.load` when you need them): older\n"
        ),
        "{}",
        firsts[0]
    );
    assert_eq!(
        timeline(&evs),
        vec!["model:older", "model:newer", "compaction", "reattach:newer"]
    );
    assert_eq!(
        notices(&evs, "skill_reattach_truncated"),
        vec!["not re-attached for space: older".to_string()]
    );
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A run that loaded `pdf` and died after that turn.
async fn interrupted_after_a_load(global: &Path, ws: &Path) -> PathBuf {
    let transcript = ws.join("first.jsonl");
    let provider = MockProvider::new(vec![load_turn("c1", "pdf"), done_turn()]);
    run_agent(opts(
        provider,
        ws,
        transcript.clone(),
        resolved(global, &["pdf"]),
    ))
    .await
    .unwrap();
    let all = lines(&transcript);
    let end = all.iter().position(|v| v["type"] == "turn_end").unwrap();
    let body: String = all[..=end].iter().map(|v| format!("{v}\n")).collect();
    std::fs::write(&transcript, body).unwrap();
    transcript
}

/// Review Focus 4: a continued run keeps the skills its predecessor loaded,
/// records them at its start and re-attaches them at its next compaction.
#[tokio::test]
async fn a_continued_run_keeps_and_reattaches_its_predecessors_skills() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let pdf = install(global.path(), "pdf", "PDF STEPS");
    let first = interrupted_after_a_load(global.path(), ws.path()).await;
    let hash = rupu_skills::load_skill(&pdf, rupu_skills::Layer::Global)
        .unwrap()
        .content_hash()
        .unwrap()
        .hex();
    let (messages, seed_source, loaded) =
        match rupu_agent::continuation::prepare_continuation(&first).unwrap() {
            rupu_agent::continuation::Continuation::Resume {
                messages,
                seed_source,
                loaded_skills,
                ..
            } => (messages, seed_source, loaded_skills),
            other => panic!("expected Resume, got {other:?}"),
        };
    assert_eq!(
        loaded,
        vec![LoadedSkill {
            name: "pdf".into(),
            content_hash: hash.clone()
        }]
    );

    let second = ws.path().join("second.jsonl");
    let provider = MockProvider::new(vec![read_turn("c2", 900), summary_turn(), done_turn()]);
    let mut o = opts(
        provider,
        ws.path(),
        second.clone(),
        resolved(global.path(), &["pdf"]),
    );
    rupu_agent::continuation::apply_continuation(&mut o.prompt, messages, seed_source, loaded);
    assert_eq!(o.prompt.loaded_skills.len(), 1);
    run_agent(o).await.unwrap();
    let evs = events(&second);
    assert_eq!(
        timeline(&evs),
        vec!["continuation:pdf", "compaction", "reattach:pdf"]
    );
    let recorded: Vec<String> = evs
        .iter()
        .filter_map(|e| match e {
            Event::SkillLoad {
                via, content_hash, ..
            } if via == "continuation" => Some(content_hash.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(recorded, vec![hash]);
    assert!(compacted_first_messages(&evs)[0].contains("PDF STEPS"));
}

/// A continued run whose skills no longer include one its predecessor
/// loaded drops it, with a notice, and records no load for it.
#[tokio::test]
async fn a_continued_run_drops_a_skill_it_is_no_longer_given() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    install(global.path(), "pdf", "PDF STEPS");
    install(global.path(), "other", "OTHER");
    let first = interrupted_after_a_load(global.path(), ws.path()).await;
    let rupu_agent::continuation::Continuation::Resume {
        messages,
        seed_source,
        loaded_skills,
        ..
    } = rupu_agent::continuation::prepare_continuation(&first).unwrap()
    else {
        panic!("expected Resume")
    };
    let second = ws.path().join("second.jsonl");
    let provider = MockProvider::new(vec![done_turn()]);
    let mut o = opts(
        provider,
        ws.path(),
        second.clone(),
        resolved(global.path(), &["other"]),
    );
    rupu_agent::continuation::apply_continuation(
        &mut o.prompt,
        messages,
        seed_source,
        loaded_skills,
    );
    run_agent(o).await.unwrap();
    let evs = events(&second);
    assert!(timeline(&evs).is_empty(), "{:?}", timeline(&evs));
    assert_eq!(
        notices(&evs, "skill_missing"),
        vec![
            "skill pdf was loaded by the run this one continues but is not granted to this run; \
             it is not re-attached"
                .to_string()
        ]
    );
}

fn catalog(global: &Path) -> rupu_skills::SkillCatalog {
    rupu_skills::SkillCatalog::discover(&rupu_skills::LayerDirs::for_layers(global, None)).unwrap()
}

/// An operator's invocation: the version the session injected.
fn operator_load(dir: &Path) -> rupu_agent::OperatorLoad {
    let skill = rupu_skills::load_skill(dir, rupu_skills::Layer::Global).unwrap();
    rupu_agent::OperatorLoad {
        name: skill.name.as_str().to_owned(),
        layer: "global".into(),
        content_hash: skill.content_hash().unwrap().hex(),
        base_dir: skill.root.display().to_string(),
    }
}

/// A skill the operator invoked is recorded as loaded after the run's
/// `skill_index`, and a later compaction re-attaches it like any other.
#[tokio::test]
async fn an_operator_load_is_recorded_and_reattached() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let pdf = install(global.path(), "pdf", "PDF STEPS");
    let frozen = rupu_skills::run::resolve_operator(&catalog(global.path()), &[], "pdf").unwrap();
    let (skills, missing) = rupu_skills::run::resolve_frozen(&catalog(global.path()), &[frozen]);
    assert!(missing.is_empty());
    let transcript = ws.path().join("run.jsonl");
    let provider = MockProvider::new(vec![read_turn("c1", 900), summary_turn(), done_turn()]);
    let mut o = opts(provider, ws.path(), transcript.clone(), Arc::new(skills));
    let load = operator_load(&pdf);
    o.prompt.operator_loads = vec![load.clone()];
    run_agent(o).await.unwrap();
    let evs = events(&transcript);
    assert_eq!(
        timeline(&evs),
        vec!["operator:pdf", "compaction", "reattach:pdf"]
    );
    let index_at = evs
        .iter()
        .position(|e| matches!(e, Event::SkillIndex { .. }))
        .expect("skill_index");
    let operator_at = evs
        .iter()
        .position(|e| matches!(e, Event::SkillLoad { via, .. } if via == "operator"))
        .unwrap();
    assert!(index_at < operator_at);
    let Event::SkillLoad {
        name,
        layer,
        content_hash,
        base_dir,
        ..
    } = &evs[operator_at]
    else {
        unreachable!()
    };
    assert_eq!(
        (name, layer, content_hash, base_dir),
        (&load.name, &load.layer, &load.content_hash, &load.base_dir)
    );
    assert!(compacted_first_messages(&evs)[0].contains("PDF STEPS"));
    assert!(notices(&evs, "skill_changed").is_empty());
}

/// An operator load replaces a continued conversation's entry of the same
/// name: the set keeps one entry per skill, at the injected version.
#[tokio::test]
async fn an_operator_load_replaces_a_carried_entry_of_the_same_name() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let pdf = install(global.path(), "pdf", "PDF STEPS");
    let transcript = ws.path().join("run.jsonl");
    let provider = MockProvider::new(vec![read_turn("c1", 900), summary_turn(), done_turn()]);
    let mut o = opts(
        provider,
        ws.path(),
        transcript.clone(),
        resolved(global.path(), &["pdf"]),
    );
    o.prompt.loaded_skills = vec![LoadedSkill {
        name: "pdf".into(),
        content_hash: "0".repeat(64),
    }];
    o.prompt.operator_loads = vec![operator_load(&pdf)];
    run_agent(o).await.unwrap();
    let evs = events(&transcript);
    assert_eq!(
        timeline(&evs),
        vec![
            "continuation:pdf",
            "operator:pdf",
            "compaction",
            "reattach:pdf"
        ]
    );
    // Re-attached against the operator's version, so nothing changed.
    assert!(notices(&evs, "skill_changed").is_empty());
}

/// The shared helper keeps the most recently loaded skill that fits the cap
/// and names the older one for the model to reload.
#[test]
fn reattach_block_keeps_the_most_recent_within_the_cap() {
    let global = tempfile::tempdir().unwrap();
    install(global.path(), "older", &"A".repeat(700));
    let newer = install(global.path(), "newer", &"B".repeat(700));
    let skills = resolved(global.path(), &["older", "newer"]);
    let newer_hash = rupu_skills::load_skill(&newer, rupu_skills::Layer::Global)
        .unwrap()
        .content_hash()
        .unwrap()
        .hex();
    let loaded = vec![
        LoadedSkill {
            name: "older".into(),
            content_hash: "1".repeat(64),
        },
        LoadedSkill {
            name: "newer".into(),
            content_hash: "2".repeat(64),
        },
    ];
    let r = rupu_agent::reattach_block(&loaded, &skills, 1000).expect("a block");
    assert!(
        r.block.starts_with(rupu_skills::render::REATTACH_MARKER),
        "{}",
        r.block
    );
    assert!(r.block.contains(&"B".repeat(700)), "{}", r.block);
    assert!(!r.block.contains(&"A".repeat(700)), "{}", r.block);
    assert_eq!(r.omitted, vec!["older".to_string()]);
    assert!(r.missing.is_empty());
    assert_eq!(r.changed, vec!["newer".to_string()]);
    assert_eq!(
        r.reattached,
        vec![LoadedSkill {
            name: "newer".into(),
            content_hash: newer_hash.clone()
        }]
    );
    assert_eq!(
        r.loaded,
        vec![
            LoadedSkill {
                name: "older".into(),
                content_hash: "1".repeat(64)
            },
            LoadedSkill {
                name: "newer".into(),
                content_hash: newer_hash
            }
        ]
    );
    assert!(rupu_agent::reattach_block(&[], &skills, 1000).is_none());
}

/// A skill no longer installed stays in the loaded set (a reinstall brings
/// it back at the next re-attach) and is listed in the block.
#[test]
fn reattach_block_keeps_a_missing_skill_loaded() {
    let global = tempfile::tempdir().unwrap();
    let gone = install(global.path(), "gone", "GONE STEPS");
    let skills = resolved(global.path(), &["gone"]);
    std::fs::remove_dir_all(&gone).unwrap();
    let loaded = vec![LoadedSkill {
        name: "gone".into(),
        content_hash: "3".repeat(64),
    }];
    let r = rupu_agent::reattach_block(&loaded, &skills, 32 * 1024).expect("a block");
    assert_eq!(r.missing, vec!["gone".to_string()]);
    assert!(r.reattached.is_empty());
    assert_eq!(r.loaded, loaded);
    assert!(
        r.block.contains("\nNo longer installed: gone\n"),
        "{}",
        r.block
    );
}

/// A frozen set's `skill_index` rows carry the stamp the session recorded,
/// not the skill's stamp now.
#[tokio::test]
async fn skill_index_rows_carry_the_baseline_stamp() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let pdf = install(global.path(), "pdf", "PDF STEPS");
    let frozen = resolved(global.path(), &["pdf"]).freeze();
    let baseline = frozen[0].stamp.clone();
    std::fs::write(
        pdf.join("SKILL.md"),
        "---\nname: pdf\ndescription: d.\n---\nLONGER PDF STEPS NOW\n",
    )
    .unwrap();
    let (skills, missing) = rupu_skills::run::resolve_frozen(&catalog(global.path()), &frozen);
    assert!(missing.is_empty());
    let live = skills.get("pdf").unwrap().skill.stamp.hex();
    assert_ne!(live, baseline);
    let transcript = ws.path().join("run.jsonl");
    let provider = MockProvider::new(vec![done_turn()]);
    run_agent(opts(
        provider,
        ws.path(),
        transcript.clone(),
        Arc::new(skills),
    ))
    .await
    .unwrap();
    let stamps: Vec<String> = events(&transcript)
        .iter()
        .filter_map(|e| match e {
            Event::SkillIndex { skills } => Some(skills.iter().map(|s| s.stamp.clone())),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(stamps, vec![baseline]);
}

/// A session turn's frozen skills that can no longer be given (spec
/// 2026-10-12 §5.1) are reported at run start, after the `skill_index`, and
/// the run goes on without them; a turn left with no skills reports them
/// too.
#[tokio::test]
async fn missing_frozen_skills_are_reported_at_run_start() {
    let global = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    install(global.path(), "pdf", "PDF STEPS");
    let missing = vec![
        rupu_skills::MissingSkill {
            name: "gone".into(),
            layer: rupu_skills::Layer::Global,
            reason: rupu_skills::MissingReason::Removed,
        },
        rupu_skills::MissingSkill {
            name: "notes".into(),
            layer: rupu_skills::Layer::Project,
            reason: rupu_skills::MissingReason::BadStamp,
        },
    ];
    let expected = vec![
        "skill gone (global) is not available to this run: removed".to_string(),
        "skill notes (project) is not available to this run: corrupt record: bad stamp".to_string(),
    ];
    for with_skills in [true, false] {
        let transcript = ws.path().join(format!("run_{with_skills}.jsonl"));
        let provider = MockProvider::new(vec![done_turn()]);
        let grants: &[&str] = if with_skills { &["pdf"] } else { &[] };
        let mut o = opts(
            provider,
            ws.path(),
            transcript.clone(),
            resolved(global.path(), grants),
        );
        if !with_skills {
            o.tool_context.services.skills = None;
        }
        o.prompt.missing_skills = missing.clone();
        run_agent(o).await.unwrap();
        let evs = events(&transcript);
        assert_eq!(notices(&evs, "skill_missing"), expected);
        let first_missing = evs
            .iter()
            .position(|e| matches!(e, Event::Notice { kind, .. } if kind == "skill_missing"))
            .unwrap();
        let first_turn = evs
            .iter()
            .position(|e| matches!(e, Event::TurnStart { .. }))
            .expect("a turn");
        assert!(first_missing < first_turn);
        if let Some(index_at) = evs
            .iter()
            .position(|e| matches!(e, Event::SkillIndex { .. }))
        {
            assert!(with_skills && index_at < first_missing);
        }
    }
}
