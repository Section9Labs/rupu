//! Sessions carry skills (spec 2026-10-12 §5.1): the set and its prompt
//! section are fixed when the session starts, and the skills the model
//! loaded carry from turn to turn. Turns run through `run_turn` against the
//! `RUPU_MOCK_PROVIDER_SCRIPT` seam.

use super::tests::{test_session_record, with_mock_home};
use super::*;
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::ContentBlock;
use serde_json::json;

fn install_skill(global: &Path, name: &str, body: &str) -> PathBuf {
    let dir = global.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: The {name} skill.\n---\n{body}\n"),
    )
    .unwrap();
    dir
}

fn agent(keys: &str) -> rupu_agent::AgentSpec {
    rupu_agent::AgentSpec::parse(&format!(
        "---\nname: issue-reader\nprovider: anthropic\nmodel: claude-sonnet-4-6\n{keys}\n---\n\
         You are a test agent.\n"
    ))
    .unwrap()
}

/// An idle session under `<tmp>/global` with no project layer, its skills
/// frozen from `agent` plus `skills` / `preload` the way `session start`
/// freezes them.
async fn skill_session(
    tmp: &tempfile::TempDir,
    session_id: &str,
    agent: &rupu_agent::AgentSpec,
    skills: &[&str],
    preload: &[&str],
) -> (PathBuf, SessionRecord) {
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("notes.txt"), "invented notes\n").unwrap();
    let mut record = test_session_record();
    record.session_id = session_id.into();
    record.status = SessionStatus::Idle;
    record.workspace_path = workspace;
    record.project_root = None;
    record.repo_ref = None;
    record.issue_ref = None;
    record.target = None;
    record.workspace_strategy = None;
    record.transcripts_dir = global.join("sessions").join(session_id).join("transcripts");
    record.active_run_id = None;
    record.active_transcript_path = None;
    record.active_pid = None;
    record.worker_pid = None;
    record.last_run_id = None;
    record.last_transcript_path = None;
    record.runs = Vec::new();
    record.total_turns = 0;
    let owned = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (index, prompt) =
        freeze_session_skills(&global, None, agent, owned(skills), owned(preload))
            .await
            .expect("the session's skills resolve");
    record.skill_index = index;
    record.skill_prompt = prompt;
    write_session(&global, SessionScope::Active, &record).unwrap();
    (global, record)
}

/// Runs one turn and returns its transcript.
async fn turn(global: &Path, session_id: &str, run_id: &str, script: &str) -> Vec<TranscriptEvent> {
    with_mock_home(
        global,
        script,
        run_turn(RunTurnArgs {
            session_id: session_id.into(),
            run_id: run_id.into(),
            prompt: format!("prompt for {run_id}"),
        }),
    )
    .await
    .expect("the turn runs");
    let (session, _) = read_session(global, session_id).unwrap();
    assert_eq!(
        session.status,
        SessionStatus::Idle,
        "{:?}",
        session.last_error
    );
    JsonlReader::iter(session.transcripts_dir.join(format!("{run_id}.jsonl")))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

const ACK: &str = r#"[{ "AssistantText": { "text": "ack", "stop": "end_turn" } }]"#;

/// The model calls `skills.load` for `name`, then answers.
fn load(name: &str) -> String {
    json!([
        { "AssistantToolUse": {
            "text": null, "tool_id": "c1", "tool_name": "skills.load",
            "tool_input": { "name": name }, "stop": "tool_use"
        } },
        { "AssistantText": { "text": "ack", "stop": "end_turn" } }
    ])
    .to_string()
}

fn system_prompt(evs: &[TranscriptEvent]) -> String {
    evs.iter()
        .find_map(|e| match e {
            TranscriptEvent::RunStart { system_prompt, .. } => system_prompt.clone(),
            _ => None,
        })
        .expect("a run_start with a system prompt")
}

/// `(name, reason)` of the turn's `skill_index`.
fn index(evs: &[TranscriptEvent]) -> Vec<(String, String)> {
    evs.iter()
        .find_map(|e| match e {
            TranscriptEvent::SkillIndex { skills } => Some(
                skills
                    .iter()
                    .map(|s| (s.name.clone(), s.reason.clone()))
                    .collect(),
            ),
            _ => None,
        })
        .expect("a skill_index")
}

fn notices(evs: &[TranscriptEvent], kind: &str) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            TranscriptEvent::Notice { kind: k, message } if k == kind => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// Review Focus 1 and 4: every turn's system prompt is the one the session
/// started with, byte for byte, though a preloaded skill was edited and a
/// new skill installed in between; a load judges the edit against the
/// session's baseline.
#[tokio::test]
async fn a_session_freezes_its_skills_at_start() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let global = tmp.path().join("global");
    install_skill(&global, "report-style", "STYLE V1");
    install_skill(&global, "code-review", "REVIEW STEPS");
    let spec = agent("skills: [\"*\"]\npreloadSkills: [report-style]");
    let (global, record) = skill_session(&tmp, "ses_skills_freeze01", &spec, &[], &[]).await;

    let first = turn(&global, &record.session_id, "run_freeze_1", ACK).await;
    install_skill(&global, "report-style", "STYLE V2");
    install_skill(&global, "late", "LATE STEPS");
    let second = turn(
        &global,
        &record.session_id,
        "run_freeze_2",
        &load("report-style"),
    )
    .await;

    let prompt = system_prompt(&first);
    assert_eq!(system_prompt(&second), prompt);
    assert!(prompt.contains("## Skill: report-style"), "{prompt}");
    assert!(prompt.contains("STYLE V1") && !prompt.contains("STYLE V2"));
    assert!(!prompt.contains("late"), "{prompt}");
    let agent_rows = pairs(&[("code-review", "agent"), ("report-style", "agent")]);
    assert_eq!(index(&first), agent_rows);
    assert_eq!(index(&second), agent_rows);
    assert_eq!(
        notices(&second, "skill_changed"),
        vec![
            "skill report-style changed on disk since this run started; the model was given \
             its current version"
                .to_string()
        ]
    );
}

#[tokio::test]
async fn start_flags_add_skills_with_reason_session() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, record) = skill_session(
        &tmp,
        "ses_skills_flags01",
        &agent(""),
        &["code-review"],
        &[],
    )
    .await;
    let evs = turn(&global, &record.session_id, "run_flags_1", ACK).await;
    assert_eq!(index(&evs), pairs(&[("code-review", "session")]));
    assert!(system_prompt(&evs).contains("- code-review: The code-review skill."));
}

/// Review Focus 2 (in-turn compaction): a skill loaded in turn 1 is carried
/// on the session and re-attached when turn 2 compacts.
#[tokio::test]
async fn loaded_skills_persist_across_turns_and_reattach() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let skill_dir = install_skill(&tmp.path().join("global"), "code-review", "REVIEW STEPS");
    let (global, mut record) = skill_session(
        &tmp,
        "ses_skills_loaded01",
        &agent("skills: [code-review]"),
        &[],
        &[],
    )
    .await;
    let dense = "x".repeat(1000);
    let mut history = vec![Message::user(&format!("task: {dense}"))];
    for i in 0..3 {
        history.push(Message::assistant(&format!("assistant {i}: {dense}")));
        history.push(Message::user(&format!("user {i}: {dense}")));
    }
    record.message_history = history;
    write_session(&global, SessionScope::Active, &record).unwrap();

    turn(
        &global,
        &record.session_id,
        "run_loaded_1",
        &load("code-review"),
    )
    .await;
    let (mut after_first, _) = read_session(&global, &record.session_id).unwrap();
    let skill = rupu_skills::load_skill(&skill_dir, rupu_skills::Layer::Global).unwrap();
    assert_eq!(
        after_first.loaded_skills,
        vec![rupu_agent::LoadedSkill {
            name: "code-review".into(),
            content_hash: skill.content_hash().unwrap().hex(),
        }]
    );

    after_first.model_limits = Some(ModelLimits::fixed(1000, 100).with_percent(50));
    write_session(&global, SessionScope::Active, &after_first).unwrap();
    let script = json!([
        { "AssistantBlocksWithUsage": {
            "content": [{ "type": "tool_use", "id": "c2", "name": "read_file",
                          "input": { "path": "notes.txt" } }],
            "stop": "tool_use",
            "usage": { "input_tokens": 900, "output_tokens": 5 }
        } },
        { "AssistantText": { "text": "Summary of prior work.", "stop": "end_turn" } },
        { "AssistantText": { "text": "done", "stop": "end_turn" } }
    ])
    .to_string();
    let second = turn(&global, &record.session_id, "run_loaded_2", &script).await;
    let first_message: Vec<String> = second
        .iter()
        .find_map(|e| match e {
            TranscriptEvent::Compaction { messages, .. } => {
                let messages: Vec<Message> = serde_json::from_value(messages.clone()).unwrap();
                Some(
                    messages[0]
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect(),
                )
            }
            _ => None,
        })
        .expect("turn 2 compacted");
    let reattached: Vec<&String> = first_message
        .iter()
        .filter(|t| t.starts_with(rupu_skills::render::REATTACH_MARKER))
        .collect();
    assert_eq!(reattached.len(), 1, "{first_message:?}");
    assert!(reattached[0].contains("## Skill: code-review"));
    assert!(reattached[0].contains("REVIEW STEPS"));
    let (after_second, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(after_second.loaded_skills, after_first.loaded_skills);
}

#[tokio::test]
async fn a_frozen_skill_removed_mid_session_is_dropped_with_a_notice() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let global = tmp.path().join("global");
    install_skill(&global, "code-review", "REVIEW STEPS");
    let pdf = install_skill(&global, "pdf", "PDF STEPS");
    let (global, record) = skill_session(
        &tmp,
        "ses_skills_removed01",
        &agent("skills: [code-review, pdf]"),
        &[],
        &[],
    )
    .await;
    turn(&global, &record.session_id, "run_removed_1", ACK).await;
    fs::remove_dir_all(&pdf).unwrap();
    let evs = turn(&global, &record.session_id, "run_removed_2", &load("pdf")).await;
    assert_eq!(
        notices(&evs, "skill_missing"),
        vec!["skill pdf (global) is not available to this run: removed".to_string()]
    );
    assert_eq!(index(&evs), pairs(&[("code-review", "agent")]));
    let result = evs
        .iter()
        .find_map(|e| match e {
            TranscriptEvent::ToolResult {
                call_id,
                output,
                error,
                ..
            } if call_id == "c1" => Some(format!("{output} {}", error.clone().unwrap_or_default())),
            _ => None,
        })
        .expect("the load's result");
    assert!(result.contains("no skill named \"pdf\""), "{result}");
    let (after, _) = read_session(&global, &record.session_id).unwrap();
    assert_eq!(
        after.skill_index, record.skill_index,
        "the frozen set is kept"
    );
}

/// Review Focus 5: a record written before sessions carried skills runs
/// its turn as before and is written back without skill keys.
#[tokio::test]
async fn a_record_without_skill_fields_runs_unchanged() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (global, record) = skill_session(&tmp, "ses_skills_legacy01", &agent(""), &[], &[]).await;
    let path = session_record_path(&global, SessionScope::Active, &record.session_id);
    let keys = |path: &Path| -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        v.as_object().unwrap().keys().cloned().collect()
    };
    let skill_keys = ["skill_index", "skill_prompt", "loaded_skills"];
    assert!(keys(&path)
        .iter()
        .all(|k| !skill_keys.contains(&k.as_str())));

    let evs = turn(&global, &record.session_id, "run_legacy_1", ACK).await;
    assert!(!evs
        .iter()
        .any(|e| matches!(e, TranscriptEvent::SkillIndex { .. })));
    assert!(notices(&evs, "skill_missing").is_empty());
    let prompt = system_prompt(&evs);
    assert!(prompt.starts_with("You are a test agent."), "{prompt}");
    assert!(!prompt.contains("## Skill"), "{prompt}");
    assert!(keys(&path)
        .iter()
        .all(|k| !skill_keys.contains(&k.as_str())));
}
